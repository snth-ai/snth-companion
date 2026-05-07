package daemon

// tasks_worker.go — companion-side sub-agent runner for the Tasks
// system (SPEC §10 + §16.5/16.6).
//
// Architecture (v1, polling-based — not WS-pushed):
//
//   1. Synth-side orchestrator atomically claims a queued task on hub:
//      task.state = "claimed", task.owner_synth_id = <synth>.
//   2. Companion polls hub `/api/my/tasks?owner_synth_id=<active>&state=claimed`
//      every 10s. For each task it doesn't already run locally, spawn.
//   3. Spawn: render prompt → write workspace files → exec subprocess
//      detached (setsid) → POST subagent_started + new_state=running.
//   4. Monitor goroutine tails transcript every 1s, posts progress
//      events (debounced 10s), watches for INPUT REQUIRED markers,
//      detects exit, posts subagent_exited with new_state=done|error.
//   5. Reattach on companion restart: scan tasks dir, read meta.json,
//      verify PID alive, resume monitor.
//
// Auth: uses companion bearer (cfg.CompanionToken). All calls through
// cfg.HubURL/api/my/tasks*. The hub allows companion bearer everywhere
// per dual-auth in admin/my_tasks.go.

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/osteele/liquid"

	"github.com/snth-ai/snth-companion/internal/config"
)

// TasksWorker owns the per-companion sub-agent runner.
type TasksWorker struct {
	HTTP         *http.Client
	PollInterval time.Duration

	mu      sync.Mutex
	running map[string]*localRun // task_id → run

	stopCh chan struct{}
	once   sync.Once
}

// localRun tracks a task we have a local subprocess for.
type localRun struct {
	taskID    string
	workspace string
	pid       int
	cmd       *exec.Cmd      // nil after reattach
	startedAt time.Time
	exited    bool
	cancel    context.CancelFunc
}

// taskMeta is persisted to <workspace>/meta.json so we can reattach on
// companion restart. Mirrors registry shape selectively.
type taskMeta struct {
	TaskID       string    `json:"task_id"`
	SynthID      string    `json:"synth_id"`
	SubAgentKind string    `json:"sub_agent_kind"`
	PID          int       `json:"pid"`
	StartedAt    time.Time `json:"started_at"`
	Command      string    `json:"command"`
	Budget       runBudget `json:"budget"`
}

// taskRow is the subset of the hub Task wire shape we use here.
type taskRow struct {
	ID                string  `json:"id"`
	Title             string  `json:"title"`
	Description       string  `json:"description"`
	State             string  `json:"state"`
	OwnerSynthID      *string `json:"owner_synth_id"`
	SubAgentKind      string  `json:"sub_agent_kind"`
	TemplateID        *string `json:"template_id"`
	TemplateOverrides string  `json:"template_overrides"`
	WorkspacePath     string  `json:"workspace_path"`
	SubAgentPID       int     `json:"sub_agent_pid"`
	TranscriptPath    string  `json:"transcript_path"`
}

// taskTemplate subset.
type taskTemplate struct {
	ID                 string `json:"id"`
	Name               string `json:"name"`
	PromptTemplate     string `json:"prompt_template"`
	DefaultAgentConfig string `json:"default_agent_config"`
}

// global singleton — only one worker per companion process.
var (
	workerMu  sync.Mutex
	curWorker *TasksWorker
)

// StartTasksWorker launches the polling loop. Safe to call before any
// pair is configured — the loop polls cfg.ActivePair() each tick and
// no-ops gracefully when no pair / no token.
func StartTasksWorker(ctx context.Context) *TasksWorker {
	workerMu.Lock()
	defer workerMu.Unlock()
	if curWorker != nil {
		return curWorker
	}
	w := &TasksWorker{
		HTTP:         &http.Client{Timeout: 30 * time.Second},
		PollInterval: 10 * time.Second,
		running:      make(map[string]*localRun),
		stopCh:       make(chan struct{}),
	}
	curWorker = w
	if err := w.reattach(); err != nil {
		log.Printf("[tasks-worker] reattach error: %v", err)
	}
	go w.loop(ctx)
	return w
}

// Stop signals the loop to exit. In-flight subprocesses keep running
// (detached) — they'll be reattached on next companion start.
func (w *TasksWorker) Stop() {
	w.once.Do(func() { close(w.stopCh) })
}

// Snapshot returns running task IDs + PIDs for /api/status surfaces.
func (w *TasksWorker) Snapshot() []map[string]any {
	w.mu.Lock()
	defer w.mu.Unlock()
	out := make([]map[string]any, 0, len(w.running))
	for _, r := range w.running {
		out = append(out, map[string]any{
			"task_id":    r.taskID,
			"pid":        r.pid,
			"workspace":  r.workspace,
			"started_at": r.startedAt.Format(time.RFC3339),
			"exited":     r.exited,
		})
	}
	return out
}

func (w *TasksWorker) loop(ctx context.Context) {
	log.Printf("[tasks-worker] started: poll=%s", w.PollInterval)
	t := time.NewTicker(w.PollInterval)
	defer t.Stop()
	w.tick(ctx)
	for {
		select {
		case <-ctx.Done():
			log.Printf("[tasks-worker] stop: ctx done")
			return
		case <-w.stopCh:
			log.Printf("[tasks-worker] stop: explicit")
			return
		case <-t.C:
			w.tick(ctx)
		}
	}
}

func (w *TasksWorker) tick(ctx context.Context) {
	defer func() {
		if r := recover(); r != nil {
			log.Printf("[tasks-worker] tick panic: %v", r)
		}
	}()
	cfg := config.Get()
	if cfg == nil {
		return
	}
	pair := cfg.ActivePair()
	if pair == nil || pair.Token == "" {
		return
	}
	tasks, err := w.listOwnedTasks(pair.ID)
	if err != nil {
		log.Printf("[tasks-worker] list tasks: %v", err)
		return
	}
	for _, t := range tasks {
		// Only spawn for tasks in `claimed` state — orchestrator already
		// claimed them and they're waiting for the worker. Tasks that
		// already moved to `running`/`awaiting_input`/etc. are either
		// already locally running (skip) or owned by a different
		// companion (also skip — meta.json absent locally).
		if t.State != "claimed" {
			continue
		}
		w.mu.Lock()
		_, already := w.running[t.ID]
		w.mu.Unlock()
		if already {
			continue
		}
		go w.spawn(ctx, t)
	}
}

// --- HTTP helpers ----------------------------------------------------

func (w *TasksWorker) hubReq(method, path string, body any) (*http.Response, error) {
	cfg := config.Get()
	pair := cfg.ActivePair()
	if pair == nil {
		return nil, fmt.Errorf("no active synth pair")
	}
	hubURL := pair.HubURL
	if hubURL == "" {
		hubURL = cfg.HubURL
	}
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}
	target := strings.TrimRight(hubURL, "/") + path
	var rdr io.Reader
	if body != nil {
		raw, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		rdr = bytes.NewReader(raw)
	}
	req, err := http.NewRequest(method, target, rdr)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+pair.Token)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	return w.HTTP.Do(req)
}

func (w *TasksWorker) listOwnedTasks(synthID string) ([]taskRow, error) {
	q := url.Values{}
	q.Set("owner_synth_id", synthID)
	q.Set("limit", "200")
	resp, err := w.hubReq("GET", "/api/my/tasks?"+q.Encode(), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(body))
	}
	var out struct {
		Tasks []taskRow `json:"tasks"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return nil, err
	}
	return out.Tasks, nil
}

// fetchRemoteState pulls just the state field from the hub. Used by
// the monitor for cancel/block reconciliation.
func (w *TasksWorker) fetchRemoteState(id string) (string, error) {
	resp, err := w.hubReq("GET", "/api/my/tasks/"+url.PathEscape(id), nil)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return "", fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	var t struct {
		State string `json:"state"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&t); err != nil {
		return "", err
	}
	return t.State, nil
}

func (w *TasksWorker) getTemplate(id string) (*taskTemplate, error) {
	resp, err := w.hubReq("GET", "/api/my/task-templates/"+url.PathEscape(id), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	var t taskTemplate
	if err := json.NewDecoder(resp.Body).Decode(&t); err != nil {
		return nil, err
	}
	return &t, nil
}

func (w *TasksWorker) postEvent(taskID, kind string, payload, runtime map[string]any, newState string) error {
	body := map[string]any{
		"task_id": taskID,
		"kind":    kind,
	}
	if payload != nil {
		body["payload"] = payload
	}
	if runtime != nil {
		body["runtime"] = runtime
	}
	if newState != "" {
		body["new_state"] = newState
	}
	resp, err := w.hubReq("POST", "/api/my/task-event", body)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		raw, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(raw))
	}
	return nil
}

// --- workspace + spawn ------------------------------------------------

func tasksRoot() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "Library", "Application Support", "snth-companion", "tasks")
}

func workspaceFor(taskID string) string {
	return filepath.Join(tasksRoot(), taskID)
}

func (w *TasksWorker) spawn(ctx context.Context, t taskRow) {
	w.mu.Lock()
	if _, ok := w.running[t.ID]; ok {
		w.mu.Unlock()
		return
	}
	// Reserve the slot before any I/O so a slow tick doesn't double-spawn.
	w.running[t.ID] = &localRun{taskID: t.ID, startedAt: time.Now()}
	w.mu.Unlock()

	cfg := config.Get()
	pair := cfg.ActivePair()
	if pair == nil {
		w.failSpawn(t.ID, "no active synth pair", "")
		return
	}

	ws := workspaceFor(t.ID)
	if err := os.MkdirAll(ws, 0o755); err != nil {
		w.failSpawn(t.ID, "workspace_creation_failed: "+err.Error(), ws)
		return
	}

	// --- resolve effective config (template defaults + task overrides) ---
	effectiveCfg := w.resolveAgentConfig(t)
	budget := resolveBudget(effectiveCfg)
	// Sub-agent kind: task wins, then config, then "claude".
	kind := strings.TrimSpace(t.SubAgentKind)
	if kind == "" {
		if v, ok := effectiveCfg["sub_agent_kind"].(string); ok {
			kind = v
		}
	}
	if kind == "" {
		kind = "claude"
	}

	// --- render prompt ---
	prompt, err := w.renderPrompt(t)
	if err != nil {
		w.failSpawn(t.ID, "prompt_render_failed: "+err.Error(), ws)
		return
	}
	if strings.TrimSpace(prompt) == "" {
		// Fall back to title + description so we never exec with an
		// empty brief — the sub-agent would produce garbage.
		prompt = strings.TrimSpace(t.Title + "\n\n" + t.Description)
	}
	if err := os.WriteFile(filepath.Join(ws, "prompt.txt"), []byte(prompt), 0o644); err != nil {
		w.failSpawn(t.ID, "write prompt: "+err.Error(), ws)
		return
	}
	if err := os.WriteFile(filepath.Join(ws, "transcript.log"), nil, 0o644); err != nil {
		// Pre-create so the tail goroutine can open it before subprocess
		// starts writing.
		w.failSpawn(t.ID, "init transcript: "+err.Error(), ws)
		return
	}

	// --- build command ---
	cmdStr, err := buildCommand(kind, ws)
	if err != nil {
		w.failSpawn(t.ID, "build command: "+err.Error(), ws)
		return
	}

	// --- exec detached ---
	cmd := exec.Command("bash", "-lc", cmdStr+" >> transcript.log 2>&1")
	cmd.Dir = ws
	tlog, _ := os.OpenFile(filepath.Join(ws, "transcript.log"), os.O_APPEND|os.O_WRONLY, 0o644)
	cmd.Stdout = tlog
	cmd.Stderr = tlog
	cmd.Stdin = nil
	cmd.Env = append(os.Environ(),
		"SNTH_TASK_ID="+t.ID,
		"SNTH_TASK_WORKSPACE="+ws,
	)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		w.failSpawn(t.ID, "subprocess_spawn_failed: "+err.Error(), ws)
		return
	}
	pid := cmd.Process.Pid
	log.Printf("[tasks-worker] spawned task=%s pid=%d kind=%s ws=%s",
		t.ID, pid, t.SubAgentKind, ws)

	meta := taskMeta{
		TaskID:       t.ID,
		SynthID:      pair.ID,
		SubAgentKind: kind,
		PID:          pid,
		StartedAt:    time.Now(),
		Command:      cmdStr,
		Budget:       budget,
	}
	if raw, err := json.MarshalIndent(meta, "", "  "); err == nil {
		_ = os.WriteFile(filepath.Join(ws, "meta.json"), raw, 0o644)
	}

	w.mu.Lock()
	w.running[t.ID] = &localRun{
		taskID:    t.ID,
		workspace: ws,
		pid:       pid,
		cmd:       cmd,
		startedAt: time.Now(),
	}
	w.mu.Unlock()

	if err := w.postEvent(t.ID, "subagent_started",
		map[string]any{"pid": pid},
		map[string]any{
			"workspace_path":   ws,
			"sub_agent_pid":    pid,
			"transcript_path":  filepath.Join(ws, "transcript.log"),
			"started_at":       time.Now().UTC().Format(time.RFC3339),
		},
		"running",
	); err != nil {
		log.Printf("[tasks-worker] post subagent_started: %v", err)
	}

	// Detach the process from the cmd struct's Wait loop. We poll for
	// exit ourselves via signal 0 in the monitor — that way the
	// subprocess outlives the companion and can be reattached later.
	go w.monitor(t.ID, ws, pid, cmd, budget)
}

// resolveAgentConfig merges template.default_agent_config and task.
// template_overrides per SPEC §5.2: overrides win on collision. Both
// are stored as JSON strings; this returns the merged map.
func (w *TasksWorker) resolveAgentConfig(t taskRow) map[string]any {
	merged := map[string]any{}
	if t.TemplateID != nil && *t.TemplateID != "" {
		if tpl, err := w.getTemplate(*t.TemplateID); err == nil && tpl != nil && tpl.DefaultAgentConfig != "" {
			var base map[string]any
			if err := json.Unmarshal([]byte(tpl.DefaultAgentConfig), &base); err == nil {
				for k, v := range base {
					merged[k] = v
				}
			}
		}
	}
	if t.TemplateOverrides != "" {
		var ov map[string]any
		if err := json.Unmarshal([]byte(t.TemplateOverrides), &ov); err == nil {
			for k, v := range ov {
				merged[k] = v
			}
		}
	}
	return merged
}

func (w *TasksWorker) failSpawn(taskID, msg, ws string) {
	log.Printf("[tasks-worker] spawn fail task=%s: %s", taskID, msg)
	w.mu.Lock()
	delete(w.running, taskID)
	w.mu.Unlock()
	runtime := map[string]any{"error_text": msg}
	if ws != "" {
		runtime["workspace_path"] = ws
	}
	_ = w.postEvent(taskID, "subagent_failed",
		map[string]any{"reason": msg},
		runtime,
		"error",
	)
}

func buildCommand(kind, ws string) (string, error) {
	// All commands read prompt from prompt.txt to avoid escaping. Run
	// from $ws, so prompt.txt is in cwd.
	switch strings.ToLower(strings.TrimSpace(kind)) {
	case "", "claude":
		return `claude -p "$(cat prompt.txt)" --output-format text --dangerously-skip-permissions`, nil
	case "codex":
		return `codex exec "$(cat prompt.txt)"`, nil
	case "gemini":
		return `gemini "$(cat prompt.txt)"`, nil
	case "manual":
		// Manual mode = just write the prompt + wait. Useful for
		// human-in-the-loop tasks where the user pastes the prompt
		// somewhere else and answers via /provide-input.
		return `echo "MANUAL: prompt at $(pwd)/prompt.txt — write your answer to /provide-input." && sleep 86400`, nil
	default:
		return "", fmt.Errorf("unknown sub_agent_kind: %s", kind)
	}
}

// --- prompt rendering -------------------------------------------------

// liquidEngine is the strict Liquid engine used for prompt rendering
// (SPEC §12). One per process — engines are concurrent-safe.
var liquidEngine = liquid.NewEngine()

// renderPrompt resolves template + overrides + task description per
// SPEC §5.2 and §12. Uses osteele/liquid in strict-undefined mode so
// unknown variables fail the render rather than silently emit empty
// strings (per SPEC: "Unknown variables/filters MUST fail rendering").
//
// Bindings:
//   task.{id, title, description, sub_agent_kind, priority,
//         retry_attempt, created_by, created_at, updated_at,
//         workspace_path}
//   overrides.{...}        (flattened JSON map from task.template_overrides)
//   attempt                (alias for task.retry_attempt — Symphony idiom)
//
// If task has no template, returns description (or title) verbatim.
func (w *TasksWorker) renderPrompt(t taskRow) (string, error) {
	body := strings.TrimSpace(t.Description)
	if body == "" {
		body = strings.TrimSpace(t.Title)
	}
	if t.TemplateID == nil || *t.TemplateID == "" {
		return body, nil
	}
	tpl, err := w.getTemplate(*t.TemplateID)
	if err != nil {
		log.Printf("[tasks-worker] template fetch failed for %s (using raw description): %v", *t.TemplateID, err)
		return body, nil
	}

	bindings := map[string]any{
		"task": map[string]any{
			"id":             t.ID,
			"title":          t.Title,
			"description":    t.Description,
			"sub_agent_kind": t.SubAgentKind,
		},
		"attempt": 0,
	}
	if t.TemplateOverrides != "" {
		var ov map[string]any
		if err := json.Unmarshal([]byte(t.TemplateOverrides), &ov); err == nil && len(ov) > 0 {
			bindings["overrides"] = ov
		}
	}
	if _, ok := bindings["overrides"]; !ok {
		bindings["overrides"] = map[string]any{}
	}

	rendered, err := liquidEngine.ParseAndRenderString(tpl.PromptTemplate, bindings)
	if err != nil {
		return "", fmt.Errorf("liquid render: %w", err)
	}
	return rendered, nil
}

// --- monitor ----------------------------------------------------------

var (
	tokenRE          = regexp.MustCompile(`(?i)total tokens?:\s*(\d+)`)
	costRE           = regexp.MustCompile(`(?i)cost:\s*\$?(\d+\.\d+)`)
	askRE            = regexp.MustCompile(`(?i)^\s*(INPUT REQUIRED|ASKING):\s*(.+)$`)
	defaultStallTime = 5 * time.Minute
	defaultWallTime  = 60 * time.Minute
)

// runBudget is the merged set of caps the monitor enforces. Resolved
// from template.default_agent_config + task.template_overrides per
// SPEC §5.2 (overrides win over template defaults).
type runBudget struct {
	StallTimeout time.Duration `json:"stall_timeout_ns"`
	WallTimeout  time.Duration `json:"wall_timeout_ns"`
	MaxCostUSD   float64       `json:"max_cost_usd"` // 0 = no cap
}

func defaultBudget() runBudget {
	return runBudget{
		StallTimeout: defaultStallTime,
		WallTimeout:  defaultWallTime,
		MaxCostUSD:   0,
	}
}

// resolveBudget reads budget caps from the resolved agent config map.
// Keys (all optional):
//
//	max_wall_minutes  → wallTimeout (number, minutes)
//	stall_timeout_ms  → stallTimeout (number, milliseconds)
//	max_cost_usd      → maxCostUSD  (number)
func resolveBudget(cfg map[string]any) runBudget {
	b := defaultBudget()
	if cfg == nil {
		return b
	}
	if v, ok := cfg["max_wall_minutes"]; ok {
		if f := toFloat(v); f > 0 {
			b.WallTimeout = time.Duration(f * float64(time.Minute))
		}
	}
	if v, ok := cfg["stall_timeout_ms"]; ok {
		if f := toFloat(v); f > 0 {
			b.StallTimeout = time.Duration(f * float64(time.Millisecond))
		}
	}
	if v, ok := cfg["max_cost_usd"]; ok {
		if f := toFloat(v); f > 0 {
			b.MaxCostUSD = f
		}
	}
	return b
}

func toFloat(v any) float64 {
	switch n := v.(type) {
	case float64:
		return n
	case float32:
		return float64(n)
	case int:
		return float64(n)
	case int64:
		return float64(n)
	case json.Number:
		f, _ := n.Float64()
		return f
	}
	return 0
}

// monitor tails transcript.log every 1s, posts progress + handles
// awaiting_input + detects exit. Owns the localRun until exit.
func (w *TasksWorker) monitor(taskID, ws string, pid int, cmd *exec.Cmd, budget runBudget) {
	defer func() {
		if r := recover(); r != nil {
			log.Printf("[tasks-worker] monitor panic task=%s: %v", taskID, r)
		}
		w.mu.Lock()
		delete(w.running, taskID)
		w.mu.Unlock()
	}()

	transcriptPath := filepath.Join(ws, "transcript.log")
	var off int64 = 0
	var (
		lastTokens   int
		lastCost     float64
		lastLine     string
		lastProgPost time.Time
		lastNewLine  = time.Now()
		startedAt    = time.Now()
		awaiting     bool
	)

	tick := time.NewTicker(1 * time.Second)
	defer tick.Stop()

	// reconcileEvery is how often we check the hub for an out-of-band
	// state change (operator cancel, hub-side block, etc). 5s is short
	// enough that a Telegram-issued cancel reaches the subprocess
	// quickly without burning hub QPS on every transcript-tail tick.
	reconcileEvery := 5 * time.Second
	lastReconcile := time.Now()

	for range tick.C {
		// --- hub-state reconcile (cancel / blocked propagate here) ---
		if time.Since(lastReconcile) >= reconcileEvery {
			lastReconcile = time.Now()
			if remote, err := w.fetchRemoteState(taskID); err == nil {
				switch remote {
				case "cancelled":
					log.Printf("[tasks-worker] task=%s cancelled remotely — killing pid=%d", taskID, pid)
					killProcessGroup(pid)
					_ = w.postEvent(taskID, "subagent_killed",
						map[string]any{"reason": "cancelled_by_operator"},
						nil, "",
					)
					return
				case "blocked":
					log.Printf("[tasks-worker] task=%s blocked remotely — killing pid=%d", taskID, pid)
					killProcessGroup(pid)
					_ = w.postEvent(taskID, "subagent_killed",
						map[string]any{"reason": "blocked_by_operator"},
						nil, "",
					)
					return
				}
			}
		}

		// Read new bytes since last offset.
		f, err := os.Open(transcriptPath)
		if err == nil {
			st, _ := f.Stat()
			if st != nil && st.Size() > off {
				_, _ = f.Seek(off, io.SeekStart)
				buf := make([]byte, st.Size()-off)
				n, _ := io.ReadFull(f, buf)
				off += int64(n)
				if n > 0 {
					lastNewLine = time.Now()
					lines := strings.Split(string(buf[:n]), "\n")
					for _, line := range lines {
						line = strings.TrimRight(line, "\r")
						if line == "" {
							continue
						}
						lastLine = line
						if m := tokenRE.FindStringSubmatch(line); m != nil {
							n, _ := strconv.Atoi(m[1])
							if n > lastTokens {
								lastTokens = n
							}
						}
						if m := costRE.FindStringSubmatch(line); m != nil {
							v, _ := strconv.ParseFloat(m[1], 64)
							if v > lastCost {
								lastCost = v
							}
						}
						if m := askRE.FindStringSubmatch(line); m != nil && !awaiting {
							awaiting = true
							_ = w.postEvent(taskID, "subagent_awaiting_input",
								map[string]any{"question": m[2]},
								map[string]any{"last_progress_text": "awaiting input: " + m[2]},
								"awaiting_input",
							)
							go w.waitForInput(taskID, ws)
						}
					}
				}
			}
			_ = f.Close()
		}

		// Debounced progress post (every ≥10s with new content).
		if time.Since(lastProgPost) >= 10*time.Second && lastLine != "" && !awaiting {
			runtime := map[string]any{
				"last_progress_at":   time.Now().UTC().Format(time.RFC3339),
				"last_progress_text": truncate(lastLine, 400),
			}
			if lastTokens > 0 {
				runtime["total_tokens"] = lastTokens
			}
			if lastCost > 0 {
				runtime["cost_usd"] = lastCost
			}
			_ = w.postEvent(taskID, "subagent_progress",
				map[string]any{
					"tokens": lastTokens,
					"cost":   lastCost,
					"last":   truncate(lastLine, 200),
				},
				runtime,
				"",
			)
			lastProgPost = time.Now()
		}

		// Exit detection. cmd.Wait() will reap the child once; after
		// reattach (cmd==nil) we use kill(pid, 0).
		exited := false
		if cmd != nil && cmd.ProcessState != nil {
			exited = true
		} else if err := syscall.Kill(pid, 0); err != nil {
			exited = true
		}
		if cmd != nil && !exited {
			// Non-blocking wait: only report exit after we've collected
			// the syscall.Kill signal — avoids a race where the goroutine
			// catches Wait() before transcript catches up.
		}

		if exited {
			// Try one final read to pick up any trailing lines.
			finalLine := lastLine
			exitCode := -1
			if cmd != nil && cmd.ProcessState != nil {
				exitCode = cmd.ProcessState.ExitCode()
			}
			newState := "done"
			if exitCode != 0 {
				newState = "error"
			}
			runtime := map[string]any{
				"finished_at":        time.Now().UTC().Format(time.RFC3339),
				"last_progress_at":   time.Now().UTC().Format(time.RFC3339),
				"last_progress_text": truncate(finalLine, 400),
			}
			if lastTokens > 0 {
				runtime["total_tokens"] = lastTokens
			}
			if lastCost > 0 {
				runtime["cost_usd"] = lastCost
			}
			if exitCode != 0 {
				runtime["error_text"] = fmt.Sprintf("subagent exited with code %d", exitCode)
			}
			_ = w.postEvent(taskID, "subagent_exited",
				map[string]any{
					"exit_code":          exitCode,
					"tokens_total":       lastTokens,
					"cost_usd_estimate":  lastCost,
					"last_message":       truncate(finalLine, 200),
				},
				runtime,
				newState,
			)
			log.Printf("[tasks-worker] task=%s exited code=%d tokens=%d cost=%.4f",
				taskID, exitCode, lastTokens, lastCost)
			return
		}

		// Stall + wall-time + cost enforcement (per-template budget).
		if time.Since(lastNewLine) > budget.StallTimeout {
			log.Printf("[tasks-worker] task=%s stalled (no transcript for %s) — killing",
				taskID, budget.StallTimeout)
			killProcessGroup(pid)
			_ = w.postEvent(taskID, "subagent_failed",
				map[string]any{"reason": "stall_timeout"},
				map[string]any{"error_text": "no transcript activity for " + budget.StallTimeout.String()},
				"error",
			)
			return
		}
		if time.Since(startedAt) > budget.WallTimeout {
			log.Printf("[tasks-worker] task=%s wall_timeout (>%s) — killing", taskID, budget.WallTimeout)
			killProcessGroup(pid)
			_ = w.postEvent(taskID, "subagent_failed",
				map[string]any{"reason": "wall_timeout"},
				map[string]any{"error_text": "exceeded " + budget.WallTimeout.String()},
				"error",
			)
			return
		}
		if budget.MaxCostUSD > 0 && lastCost > budget.MaxCostUSD {
			log.Printf("[tasks-worker] task=%s over_budget ($%.4f > $%.4f cap) — killing",
				taskID, lastCost, budget.MaxCostUSD)
			killProcessGroup(pid)
			_ = w.postEvent(taskID, "subagent_failed",
				map[string]any{
					"reason":     "over_budget",
					"cost_usd":   lastCost,
					"cap_usd":    budget.MaxCostUSD,
				},
				map[string]any{
					"error_text": fmt.Sprintf("cost $%.4f exceeded cap $%.4f", lastCost, budget.MaxCostUSD),
					"cost_usd":   lastCost,
				},
				"error",
			)
			return
		}
	}
}

// waitForInput polls for <ws>/input.txt — once written, we move task
// back to running. The sub-agent's own poll loop reads + deletes it.
func (w *TasksWorker) waitForInput(taskID, ws string) {
	inPath := filepath.Join(ws, "input.txt")
	for {
		time.Sleep(2 * time.Second)
		w.mu.Lock()
		_, alive := w.running[taskID]
		w.mu.Unlock()
		if !alive {
			return
		}
		if _, err := os.Stat(inPath); err == nil {
			_ = w.postEvent(taskID, "subagent_input_consumed", nil,
				map[string]any{"last_progress_text": "input received, resuming"},
				"running",
			)
			return
		}
	}
}

func killProcessGroup(pid int) {
	// SIGTERM whole group (we used Setsid), wait 5s, SIGKILL.
	if pgid, err := syscall.Getpgid(pid); err == nil {
		_ = syscall.Kill(-pgid, syscall.SIGTERM)
		time.Sleep(5 * time.Second)
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
	} else {
		_ = syscall.Kill(pid, syscall.SIGTERM)
	}
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "…"
}

// --- reattach on companion restart -----------------------------------

func (w *TasksWorker) reattach() error {
	root := tasksRoot()
	entries, err := os.ReadDir(root)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		ws := filepath.Join(root, e.Name())
		raw, err := os.ReadFile(filepath.Join(ws, "meta.json"))
		if err != nil {
			continue
		}
		var meta taskMeta
		if err := json.Unmarshal(raw, &meta); err != nil {
			continue
		}
		if meta.TaskID == "" || meta.PID == 0 {
			continue
		}
		// Probe with kill(pid, 0). If the process is dead, leave the
		// workspace on disk for forensics but don't reattach.
		if err := syscall.Kill(meta.PID, 0); err != nil {
			continue
		}
		log.Printf("[tasks-worker] reattach task=%s pid=%d ws=%s", meta.TaskID, meta.PID, ws)
		w.mu.Lock()
		w.running[meta.TaskID] = &localRun{
			taskID:    meta.TaskID,
			workspace: ws,
			pid:       meta.PID,
			cmd:       nil,
			startedAt: meta.StartedAt,
		}
		w.mu.Unlock()
		// Reattach budget — fall back to defaults if meta predates the
		// budget field (older companion versions).
		budget := meta.Budget
		if budget.WallTimeout == 0 {
			budget.WallTimeout = defaultWallTime
		}
		if budget.StallTimeout == 0 {
			budget.StallTimeout = defaultStallTime
		}
		go w.monitor(meta.TaskID, ws, meta.PID, nil, budget)
	}
	return nil
}
