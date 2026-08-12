package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
)

type fakeWorkExecutor struct {
	availableErr error
	executeErr   error
	executed     bool
	input        workRunExecution
}

func (f *fakeWorkExecutor) Available(context.Context) error { return f.availableErr }

func (f *fakeWorkExecutor) Execute(_ context.Context, in workRunExecution, emit func(string) error) error {
	f.executed = true
	f.input = in
	if err := emit(`{"type":"agent_message","text":"bounded result"}`); err != nil {
		return err
	}
	return f.executeErr
}

type workHubFixture struct {
	t         *testing.T
	token     string
	lease     workRunLease
	mu        sync.Mutex
	events    []string
	heartbeat int
}

func (f *workHubFixture) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.t.Helper()
	w.Header().Set("X-Work-Node-Contract", workNodeContract)
	if got := r.Header.Get("X-Internal-Token"); got != f.token {
		f.t.Errorf("internal token = %q", got)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	switch {
	case r.URL.Path == "/api/node/work/v1/runs/claim":
		var body struct {
			ClaimID       string   `json:"claim_id"`
			NodeID        string   `json:"node_id"`
			ExecutorTypes []string `json:"executor_types"`
		}
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body.ClaimID == "" || body.NodeID != "node_air" || len(body.ExecutorTypes) != 1 || body.ExecutorTypes[0] != "codex" {
			f.t.Errorf("unexpected claim: %+v", body)
		}
		_ = json.NewEncoder(w).Encode(f.lease)
	case strings.HasSuffix(r.URL.Path, "/heartbeat"):
		f.mu.Lock()
		f.heartbeat++
		f.mu.Unlock()
		_ = json.NewEncoder(w).Encode(f.lease)
	case strings.HasSuffix(r.URL.Path, "/context"):
		materials := []workRunContextMaterial{}
		for _, source := range f.lease.Run.ContextManifest.Sources {
			materials = append(materials, workRunContextMaterial{
				LinkID: source.LinkID, SourceType: source.SourceType, SourceID: source.SourceID,
				SourceRevision: source.SourceRevision, Title: source.Title, Format: "text/plain", Content: "frozen content",
			})
		}
		_ = json.NewEncoder(w).Encode(workRunContextBundle{
			RunID: f.lease.Run.ID, TaskTitle: f.lease.Run.ContextManifest.TaskTitle,
			TaskVersion: f.lease.Run.ContextManifest.TaskVersion, Materials: materials,
		})
	case strings.HasSuffix(r.URL.Path, "/events"):
		var body struct {
			Kind string `json:"kind"`
		}
		_ = json.NewDecoder(r.Body).Decode(&body)
		f.mu.Lock()
		f.events = append(f.events, body.Kind)
		f.mu.Unlock()
		w.WriteHeader(http.StatusCreated)
		_, _ = w.Write([]byte(`{}`))
	default:
		http.NotFound(w, r)
	}
}

func testWorkLease() workRunLease {
	return workRunLease{
		Run: workRunRow{
			ID: "run_1", TaskID: "task_1", ExecutorType: "codex", ExecutorProfile: "codex/acpx", State: "claimed",
			ContextManifest:    workRunContextManifest{TaskVersion: 3, TaskTitle: "Review the plan", TaskDescription: "Return three risks."},
			PermissionManifest: workRunPermissionManifest{ProjectContext: "read", Repository: "none", Workspace: "none", Network: "denied"},
			Budget:             workRunBudget{DurationSeconds: 60, Steps: 20, CostCents: 200},
		},
		LeaseID: "lease_1", FencingToken: 4, ExpiresAt: time.Now().Add(30 * time.Second),
	}
}

func newTestWorkWorker(server *httptest.Server, executor workRunExecutor, approve bool) *WorkRunsWorker {
	return &WorkRunsWorker{
		HubURL: server.URL, InternalToken: "secret", NodeID: "node_air",
		HTTP: server.Client(), Executor: executor,
		Approve: func(context.Context, approval.Request_) (bool, error) { return approve, nil },
	}
}

func TestWorkRunsWorkerExecutesBoundedRun(t *testing.T) {
	fixture := &workHubFixture{t: t, token: "secret", lease: testWorkLease()}
	server := httptest.NewServer(fixture)
	defer server.Close()
	executor := &fakeWorkExecutor{}
	worker := newTestWorkWorker(server, executor, true)

	if err := worker.claimAndRun(context.Background()); err != nil {
		t.Fatal(err)
	}
	if !executor.executed {
		t.Fatal("executor was not called")
	}
	if !strings.Contains(executor.input.Prompt, "Return three risks") || !strings.Contains(executor.input.Prompt, "Do not access files") {
		t.Fatalf("prompt did not preserve task and boundary: %q", executor.input.Prompt)
	}
	fixture.mu.Lock()
	defer fixture.mu.Unlock()
	if fixture.heartbeat < 1 {
		t.Fatal("initial heartbeat was not sent")
	}
	want := []string{"step.started", "output", "run.completed"}
	if strings.Join(fixture.events, ",") != strings.Join(want, ",") {
		t.Fatalf("events = %v, want %v", fixture.events, want)
	}
}

func TestWorkRunsWorkerMaterializesContextSources(t *testing.T) {
	lease := testWorkLease()
	lease.Run.ContextManifest.Sources = []workRunContextItem{{LinkID: "ctx_1", SourceType: "file", Usable: true, VerificationState: "verified"}}
	fixture := &workHubFixture{t: t, token: "secret", lease: lease}
	server := httptest.NewServer(fixture)
	defer server.Close()
	executor := &fakeWorkExecutor{}
	worker := newTestWorkWorker(server, executor, true)

	if err := worker.claimAndRun(context.Background()); err != nil {
		t.Fatal(err)
	}
	if !executor.executed {
		t.Fatal("executor did not run with bounded context materialization")
	}
	if !strings.Contains(executor.input.Prompt, "frozen content") || !strings.Contains(executor.input.Prompt, "untrusted reference data") {
		t.Fatalf("bounded context missing from prompt: %q", executor.input.Prompt)
	}
	fixture.mu.Lock()
	defer fixture.mu.Unlock()
	if got := strings.Join(fixture.events, ","); got != "step.started,output,run.completed" {
		t.Fatalf("events = %s", got)
	}
}

func TestWorkRunsWorkerApprovalDenialNeverExecutes(t *testing.T) {
	fixture := &workHubFixture{t: t, token: "secret", lease: testWorkLease()}
	server := httptest.NewServer(fixture)
	defer server.Close()
	executor := &fakeWorkExecutor{}
	worker := newTestWorkWorker(server, executor, false)

	if err := worker.claimAndRun(context.Background()); err != nil {
		t.Fatal(err)
	}
	if executor.executed {
		t.Fatal("executor ran after approval denial")
	}
	fixture.mu.Lock()
	defer fixture.mu.Unlock()
	if got := strings.Join(fixture.events, ","); got != "run.failed" {
		t.Fatalf("events = %s", got)
	}
}

func TestWorkRunsWorkerUnavailableExecutorIsTerminalFailure(t *testing.T) {
	fixture := &workHubFixture{t: t, token: "secret", lease: testWorkLease()}
	server := httptest.NewServer(fixture)
	defer server.Close()
	executor := &fakeWorkExecutor{availableErr: errors.New("missing acpx")}
	worker := newTestWorkWorker(server, executor, true)

	if err := worker.claimAndRun(context.Background()); err != nil {
		t.Fatal(err)
	}
	if executor.executed {
		t.Fatal("unavailable executor ran")
	}
	fixture.mu.Lock()
	defer fixture.mu.Unlock()
	if got := strings.Join(fixture.events, ","); got != "run.failed" {
		t.Fatalf("events = %s", got)
	}
}

func TestStartWorkRunsWorkerFromEnvFailsClosed(t *testing.T) {
	t.Setenv("SNTH_WORK_RUNNER_ENABLED", "false")
	if worker := StartWorkRunsWorkerFromEnv(context.Background()); worker != nil {
		t.Fatal("disabled worker started")
	}
	t.Setenv("SNTH_WORK_RUNNER_ENABLED", "true")
	t.Setenv("SNTH_WORK_HUB_URL", "")
	t.Setenv("HUB_URL", "")
	t.Setenv("SNTH_WORK_NODE_TOKEN", "")
	t.Setenv("SNTH_WORK_NODE_ID", "")
	if worker := StartWorkRunsWorkerFromEnv(context.Background()); worker != nil {
		t.Fatal("incompletely configured worker started")
	}
}
