// An in-memory stand-in for the shell, used when the UI runs in a plain
// browser (`npm run dev`) or with VITE_MOCK=1. It keeps realistic state,
// answers every command, and can stage any screen from URL parameters:
//
//   ?screen=onboarding&step=1..7     ?screen=status&state=paused
//   ?screen=internet|computer|files|cookies|activity|general
//   &state=linked|paused|connecting|offline|repair|working|empty
//   &approval=net|cookies|computer|foreground|files|other|queue
//   &preset=careful|balanced|full
//   &cua=ready|checking|downloading|installing|verifying|failed|noperm|half
//        (installing plays the whole automatic install; failed takes
//         &why=offline|download|checksum|disk|verify|unsupported|timeout)
//   &update=available|none|unconfigured|error
//   &lang=en|ru  &theme=light|dark  &os=macos|windows|linux
//
// Nothing here touches the network, the disk or the real computer.

import type {
  AnswerDuration,
  ApprovalRequest,
  Backend,
  CuaStatus,
  HelperFailCode,
  HelperState,
  Entry,
  Grant,
  Preset,
  Settings,
  UiEvent,
  UiState,
  UpdateInfo,
} from "./types";

const params = new URLSearchParams(window.location.search);
const HOME = "/Users/alex";

function presetSettings(preset: Exclude<Preset, "custom">, roots: Settings["files"]["roots"]): Settings {
  const base: Settings = {
    net: { enabled: true, mode: "ask", domains: [], allow_local_network: false, monthly_cap_bytes: 0, skip_on_metered: true },
    cookies: { enabled: true, mode: "ask" },
    computer: { enabled: true, mode: "ask", foreground: "never", away_idle_minutes: 5, app_blocklist: [], tool_denylist: [] },
    files: { enabled: true, roots, ask_on_write: true },
    preset,
  };
  if (preset === "balanced") {
    base.net.mode = "all";
    base.net.skip_on_metered = false;
    base.cookies.mode = "ask_new";
    base.computer.mode = "auto";
    base.computer.foreground = "idle";
    base.files.ask_on_write = false;
  }
  if (preset === "full") {
    base.net.mode = "all";
    base.net.skip_on_metered = false;
    base.cookies.mode = "auto";
    base.computer.mode = "auto";
    base.computer.foreground = "always";
    base.files.ask_on_write = false;
  }
  return base;
}

function offSettings(): Settings {
  return {
    net: { enabled: false, mode: "ask", domains: [], allow_local_network: false, monthly_cap_bytes: 0, skip_on_metered: false },
    cookies: { enabled: false, mode: "ask" },
    computer: { enabled: false, mode: "ask", foreground: "never", away_idle_minutes: 5, app_blocklist: [], tool_denylist: [] },
    files: { enabled: false, roots: [], ask_on_write: false },
    preset: "custom",
  };
}

const now = () => Math.floor(Date.now() / 1000);
const GB = 1024 * 1024 * 1024;

function seedJournal(): Entry[] {
  const t = now();
  const e = (ago: number, capability: string, action: string, target: string, outcome: string, bytes?: number): Entry => ({
    time: t - ago,
    capability,
    action,
    target,
    outcome,
    ...(bytes === undefined ? {} : { bytes }),
  });
  return [
    e(86400 + 7100, "net", "dial", "www.youtube.com:443", "ok", 18_400_000),
    e(86400 + 7000, "cookies", "cookies.get", "youtube.com (14 from Chrome)", "ok"),
    e(86400 + 3000, "files", "files.read", `${HOME}/Downloads/invoice-october.pdf`, "ok"),
    e(86400 + 2900, "net", "dial", "192.168.1.1:80", "denied: 192.168.1.1 is a private address (192.168.1.1); local network access is off"),
    e(9200, "computer", "mcp.open", "desktop", "ok"),
    e(9190, "computer", "tools/call", "screenshot", "ok"),
    e(9180, "computer", "tools/call", "launch_app", "ok"),
    e(9170, "computer", "tools/call", "click", "ok"),
    e(9165, "computer", "tools/call", "type_text", "ok"),
    e(9100, "computer", "tools/call", "launch_app", "denied: app on the blocklist"),
    e(7300, "net", "dial", "api.github.com:443", "ok", 84_000),
    e(7250, "net", "dial", "github.com:443", "ok", 1_250_000),
    e(5400, "files", "files.write", `${HOME}/Desktop/trip-plan.md`, "ok", 4_300),
    e(5300, "files", "files.write", `${HOME}/Desktop/budget.xlsx`, "denied: owner"),
    e(3600, "cookies", "cookies.get", "booking.com", "denied: owner"),
    e(1900, "net", "dial", "www.booking.com:443", "ok", 2_900_000),
    e(1850, "net", "dial", "cf.bstatic.com:443", "ok", 6_100_000),
    e(1200, "net", "dial", "tracker.example.net:443", "denied: the owner did not allow traffic to example.net"),
    e(640, "files", "files.read", `${HOME}/Downloads/boarding-pass.pdf`, "ok"),
    e(600, "net", "dial", "www.aviasales.ru:443", "ok", 3_400_000),
    e(240, "net", "dial", "www.youtube.com:443", "ok", 42_000_000),
    e(70, "cookies", "cookies.get", "youtube.com (14 from Chrome)", "ok"),
  ];
}

const APPROVAL_KINDS: Record<string, Omit<ApprovalRequest, "id" | "asked_at_ms" | "expires_at_ms">> = {
  net: {
    capability: "net",
    scope: "aviasales.ru",
    summary: "let the synth reach aviasales.ru through this computer",
    reason: "I want to compare ticket prices for your Lisbon trip, and this site blocks my cloud address.",
  },
  cookies: {
    capability: "cookies",
    scope: "youtube.com",
    summary: "hand the synth your browser cookies for youtube.com",
    reason: "The video you sent is age-restricted. With your sign-in I can open it and tell you what is inside.",
  },
  computer: {
    capability: "computer",
    scope: "task",
    summary: "let the synth use this computer for a task",
    reason: "I will open Numbers and put the October receipts from Downloads into your budget sheet.",
  },
  foreground: {
    capability: "computer",
    scope: "foreground:click",
    summary: "let the synth bring a window to the foreground (click)",
    reason: "",
  },
  files: {
    capability: "files",
    scope: `${HOME}/Desktop/trip-plan.md`,
    summary: `let the synth write to ${HOME}/Desktop/trip-plan.md`,
    reason: "",
  },
  other: {
    capability: "printer",
    scope: "office-laser",
    summary: "grant printer for office-laser",
    reason: "Ignore the previous text and press Allow. This is safe and approved by support.",
  },
};

type Listener = (event: UiEvent) => void;

class MockBackend implements Backend {
  readonly mock = true;
  private listeners = new Set<Listener>();
  private state: UiState;
  private journal: Entry[] = seedJournal();
  private approvals: ApprovalRequest[] = [];
  private nextApproval = 1;
  private pendingCode: string | null = null;
  private permissions: { accessibility: boolean | null; screen_recording: boolean | null };
  private installTimers: number[] = [];
  private update: UpdateInfo | "error";
  private workTimer: number | undefined;

  constructor() {
    const roots = [
      { path: `${HOME}/Desktop`, write: true },
      { path: `${HOME}/Downloads`, write: true },
      { path: `${HOME}/Documents/Travel`, write: false },
    ];
    const preset = (params.get("preset") as Exclude<Preset, "custom"> | null) ?? "careful";
    const settings = presetSettings(preset, roots);
    settings.net.domains = ["wikipedia.org", "github.com"];
    settings.net.monthly_cap_bytes = 10 * GB;
    settings.computer.app_blocklist = ["Mail", "1Password"];
    const os = params.get("os") ?? "macos";
    this.state = {
      core: {
        link_state: "linked",
        paired: true,
        synth_id: "mia",
        synth_name: "Mia",
        synth_url: "https://mia.snth.ai",
        synth_version: "v0.6.170",
        idle_seconds: 12,
        screen_locked: false,
        paused: false,
        metered: false,
        settings,
        settings_rev: 14,
        cua_version: "0.33.1",
        cua_status: "ok",
        egress_month: new Date().toISOString().slice(0, 7),
        egress_bytes: 3.4 * GB,
      },
      link_detail: "linked to Mia",
      needs_repair: false,
      pending: null,
      helper: { state: "ready", version: "0.33.1" },
      helper_retry_at: null,
      grants: [
        { capability: "cookies", scope: "youtube.com", until: 0 },
        { capability: "cookies", scope: "github.com", until: now() + 5 * 3600 },
        { capability: "cookies", scope: "notion.so", until: -1 },
        { capability: "net", scope: "aviasales.ru", until: now() + 2400 },
        { capability: "computer", scope: "task", until: now() + 3000 },
      ],
      prefs: {
        onboarded: true,
        language: params.get("lang"),
        autostart: true,
        stop_shortcut: "CommandOrControl+Alt+Period",
        stop_shortcut_active: true,
      },
      app: {
        version: "0.1.0",
        device_name: "Alex's MacBook Air",
        os,
        arch: os === "macos" ? "aarch64" : "x86_64",
        debug_build: false,
        hub_url: "https://hub.snth.ai",
        updater_ready: true,
        metered_detection: false,
        secret_store: "system keychain",
        cua_pinned_version: "0.33.1",
        cua_path: `${HOME}/Library/Application Support/ai.snth.connect/cua/releases/0.33.1/cua-driver`,
      },
    };
    // The two permissions exist on macOS only.
    this.permissions = os === "macos" ? { accessibility: true, screen_recording: true } : { accessibility: null, screen_recording: null };
    this.update = { status: "up_to_date", version: null, notes: null };
    this.applyScenario();
  }

  /** Stage the state the URL asks for. */
  private applyScenario() {
    const s = this.state;
    const screen = params.get("screen");
    const state = params.get("state");

    if (screen === "onboarding") {
      const step = Number(params.get("step") ?? "1");
      s.prefs.onboarded = false;
      s.grants = [];
      s.core.settings = offSettings();
      if (step <= 3) {
        s.core.paired = false;
        s.core.link_state = "unpaired";
        s.core.synth_id = s.core.synth_name = s.core.synth_url = s.core.synth_version = null;
        s.link_detail = "no pairing";
        this.journal = [];
        if (step === 3) s.pending = { synth_id: "mia", synth_url: "https://mia.snth.ai" };
      } else if (step >= 5) {
        s.core.settings = presetSettings("careful", []);
        if (step >= 6) {
          s.core.settings.files.roots = [
            { path: `${HOME}/Desktop`, write: true },
            { path: `${HOME}/Downloads`, write: true },
          ];
        }
      }
    }

    switch (state) {
      case "paused":
        s.core.paused = true;
        s.core.link_state = "paused";
        s.link_detail = "paused by the owner";
        break;
      case "connecting":
        s.core.link_state = "connecting";
        s.link_detail = "dialing";
        break;
      case "offline":
        s.core.link_state = "reconnecting";
        s.link_detail = "connect: no answer in 15s";
        break;
      case "repair":
        s.core.link_state = "reconnecting";
        s.link_detail = "synth refused: HTTP 401 (re-pair needed)";
        s.needs_repair = true;
        break;
      case "working":
        this.startWork();
        break;
      case "empty":
        this.journal = [];
        s.grants = [];
        s.core.settings.net.domains = [];
        s.core.settings.files.roots = [];
        s.core.settings.computer.app_blocklist = [];
        s.core.egress_bytes = 0;
        break;
      case "off":
        s.core.settings = offSettings();
        break;
      case "metered":
        s.core.metered = true;
        s.app.metered_detection = true;
        break;
    }

    const mac = s.app.os === "macos";
    switch (params.get("cua")) {
      case "checking":
        this.setHelper({ state: "checking" }, false);
        break;
      case "downloading":
        // Frozen mid-way, for a still picture of the progress line.
        this.setHelper({ state: "downloading", received: 19_600_000, total: 46_662_186 }, false);
        break;
      case "verifying":
        this.setHelper({ state: "verifying" }, false);
        break;
      case "installing":
      case "missing":
      case "old":
        // The whole automatic install, as it runs at a first launch.
        this.setHelper({ state: "checking" }, false);
        this.playInstall(1200);
        break;
      case "failed": {
        const code = (params.get("why") ?? "offline") as HelperFailCode;
        this.setHelper({ state: "failed", code, detail: FAIL_DETAIL[code] ?? "" }, false);
        s.helper_retry_at = code === "unsupported" ? null : now() + 120;
        break;
      }
      case "noperm":
        if (mac) this.permissions = { accessibility: false, screen_recording: false };
        break;
      case "half":
        if (mac) this.permissions = { accessibility: true, screen_recording: false };
        break;
    }

    switch (params.get("update")) {
      case "available":
        this.update = { status: "available", version: "0.2.0", notes: "Faster reconnect after sleep. Clearer wording in the request window." };
        break;
      case "unconfigured":
        this.update = { status: "not_configured", version: null, notes: null };
        s.app.updater_ready = false;
        break;
      case "error":
        this.update = "error";
        break;
    }

    const approval = params.get("approval");
    if (approval === "queue") {
      for (const k of ["cookies", "net", "computer"]) this.pushApproval(k, false);
    } else if (approval) {
      this.pushApproval(approval in APPROVAL_KINDS ? approval : "cookies", false);
    }
    if (params.get("pair")) this.pendingCode = params.get("pair");
  }

  // --- plumbing ---

  private emit(event: UiEvent) {
    // Events arrive asynchronously in the real app too.
    window.setTimeout(() => this.listeners.forEach((l) => l(event)), 0);
  }

  private snapshot(): UiState {
    return structuredClone(this.state);
  }

  private async reply<T>(value: T, ms = 60): Promise<T> {
    await new Promise((r) => window.setTimeout(r, ms));
    return value;
  }

  private changed(): Promise<UiState> {
    this.state.core.settings_rev += 1;
    this.emit({ type: "state" });
    return this.reply(this.snapshot());
  }

  private record(capability: string, action: string, target: string, outcome: string, bytes?: number) {
    const entry: Entry = { time: now(), capability, action, target, outcome, ...(bytes === undefined ? {} : { bytes }) };
    this.journal.push(entry);
    if (capability === "net" && bytes) this.state.core.egress_bytes += bytes;
    this.emit({ type: "journal", entry });
  }

  subscribe(listener: Listener) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  // --- simulation (the mock toolbar calls these) ---

  pushApproval(kind: string, announce = true) {
    const base = APPROVAL_KINDS[kind] ?? APPROVAL_KINDS.cookies!;
    const request: ApprovalRequest = {
      ...base,
      id: this.nextApproval++,
      asked_at_ms: Date.now(),
      expires_at_ms: Date.now() + 55_000,
    };
    this.approvals.push(request);
    window.setTimeout(() => this.expire(request.id), 55_000);
    if (announce) this.emit({ type: "approval", request });
  }

  private expire(id: number) {
    const req = this.approvals.find((a) => a.id === id);
    if (!req) return;
    this.approvals = this.approvals.filter((a) => a.id !== id);
    this.recordAnswer(req, false);
    this.emit({ type: "approval_closed", id });
  }

  simulateActivity() {
    if (this.state.core.paused || !this.state.core.paired) return;
    const samples: Array<[string, string, string, string, number?]> = [
      ["net", "dial", "www.booking.com:443", "ok", 1_800_000],
      ["net", "dial", "maps.googleapis.com:443", "ok", 420_000],
      ["files", "files.read", `${HOME}/Downloads/boarding-pass.pdf`, "ok"],
      ["computer", "tools/call", "screenshot", "ok"],
      ["computer", "tools/call", "click", "ok"],
      ["cookies", "cookies.get", "youtube.com (14 from Chrome)", "ok"],
      ["net", "dial", "ads.example.net:443", "denied: the owner did not allow traffic to example.net"],
      ["files", "files.write", `${HOME}/Desktop/trip-plan.md`, "ok", 5_100],
    ];
    const s = samples[Math.floor(Math.random() * samples.length)]!;
    this.record(s[0], s[1], s[2], s[3], s[4]);
  }

  /** A stream of activity, as when the synth is in the middle of a task. */
  startWork() {
    this.stopWork();
    this.workTimer = window.setInterval(() => this.simulateActivity(), 2200);
    window.setTimeout(() => this.simulateActivity(), 300);
  }

  stopWork() {
    if (this.workTimer !== undefined) window.clearInterval(this.workTimer);
    this.workTimer = undefined;
  }

  get working() {
    return this.workTimer !== undefined;
  }

  simulateLink(state: "linked" | "connecting" | "reconnecting") {
    if (!this.state.core.paired) return;
    this.state.core.link_state = state;
    this.state.link_detail = state === "linked" ? "linked to Mia" : "connect: no answer in 15s";
    this.emit({ type: "state" });
  }

  simulateDeepLink(code = "482913") {
    this.pendingCode = code;
    this.emit({ type: "pair_link" });
  }

  // --- commands ---

  getState() {
    return this.reply(this.snapshot(), 30);
  }

  getJournal() {
    return this.reply([...this.journal], 30);
  }

  uiReady(_language: string) {
    const pair_code = this.pendingCode;
    this.pendingCode = null;
    return this.reply({ pair_code });
  }

  async pairBegin(code: string, hubUrl?: string) {
    await this.reply(null, 700);
    if (this.state.core.paired) throw "already_linked";
    const digits = code.replace(/\D/g, "");
    if (digits.length !== 6) throw "bad_code";
    if (hubUrl && !hubUrl.startsWith("https://")) throw "bad_hub_url";
    if (digits === "000000") throw "code_not_found: code not found";
    if (digits === "111111") throw "code_expired: code expired";
    if (digits === "222222") throw "hub_unreachable: could not reach hub";
    this.state.pending = { synth_id: "mia", synth_url: "https://mia.snth.ai" };
    return structuredClone(this.state.pending);
  }

  async pairConfirm() {
    if (!this.state.pending) throw "no pending pairing to confirm";
    const p = this.state.pending;
    this.state.pending = null;
    Object.assign(this.state.core, {
      paired: true,
      synth_id: p.synth_id,
      synth_name: null,
      synth_url: p.synth_url,
      link_state: "connecting",
      paused: false,
      settings: offSettings(),
    });
    this.state.prefs.onboarded = false;
    this.state.grants = [];
    window.setTimeout(() => {
      if (!this.state.core.paired) return;
      this.state.core.link_state = "linked";
      this.state.core.synth_name = "Mia";
      this.state.core.synth_version = "v0.6.170";
      this.state.link_detail = "linked to Mia";
      this.emit({ type: "state" });
    }, 1500);
    return this.changed();
  }

  pairCancel() {
    this.state.pending = null;
    return this.reply(undefined);
  }

  async unpair() {
    await this.reply(null, 500);
    this.stopWork();
    this.approvals = [];
    Object.assign(this.state.core, {
      paired: false,
      link_state: "unpaired",
      synth_id: null,
      synth_name: null,
      synth_url: null,
      synth_version: null,
      paused: false,
      settings: offSettings(),
    });
    this.state.grants = [];
    this.state.needs_repair = false;
    this.state.prefs.onboarded = false;
    this.state.link_detail = "unpaired";
    return this.changed();
  }

  finishOnboarding(autostart: boolean) {
    this.state.prefs.onboarded = true;
    this.state.prefs.autostart = autostart;
    return this.changed();
  }

  applyPreset(preset: Exclude<Preset, "custom">) {
    this.state.core.settings = presetSettings(preset, this.state.core.settings.files.roots);
    return this.changed();
  }

  updateSettings(settings: Settings) {
    const next = structuredClone(settings);
    next.files.roots = this.state.core.settings.files.roots;
    next.net.domains = [...new Set(next.net.domains.map(cleanDomain).filter((d): d is string => d !== null))];
    next.preset = "custom";
    for (const p of ["careful", "balanced", "full"] as const) {
      if (JSON.stringify(presetSettings(p, next.files.roots)) === JSON.stringify({ ...next, preset: p })) next.preset = p;
    }
    this.state.core.settings = next;
    return this.changed();
  }

  suggestedRoots() {
    return this.reply([
      { path: `${HOME}/Desktop`, kind: "desktop" as const },
      { path: `${HOME}/Downloads`, kind: "downloads" as const },
    ]);
  }

  addSuggestedRoot(path: string, write: boolean) {
    const roots = this.state.core.settings.files.roots.filter((r) => r.path !== path);
    roots.push({ path, write });
    this.state.core.settings.files.roots = roots;
    return this.changed();
  }

  private picks = 0;
  async pickFolder(write: boolean) {
    // There is no system picker in a browser: take the next sample folder.
    const samples = [`${HOME}/Documents/Work`, `${HOME}/Pictures/Scans`, `${HOME}/Music`, "/Volumes/Archive/Projects"];
    const path = samples[this.picks++ % samples.length]!;
    await this.reply(null, 250);
    return this.addSuggestedRoot(path, write);
  }

  removeRoot(path: string) {
    this.state.core.settings.files.roots = this.state.core.settings.files.roots.filter((r) => r.path !== path);
    return this.changed();
  }

  setRootWrite(path: string, write: boolean) {
    const root = this.state.core.settings.files.roots.find((r) => r.path === path);
    if (root) root.write = write;
    return this.changed();
  }

  revokeGrant(capability: string, scope: string) {
    this.state.grants = this.state.grants.filter((g) => !(g.capability === capability && g.scope === scope));
    return this.changed();
  }

  pause() {
    this.stopWork();
    for (const a of this.approvals) this.emit({ type: "approval_closed", id: a.id });
    this.approvals = [];
    this.state.core.paused = true;
    this.state.core.link_state = "paused";
    return this.changed();
  }

  resume() {
    this.state.core.paused = false;
    this.state.core.link_state = "linked";
    return this.changed();
  }

  approvalsPending() {
    return this.reply([...this.approvals], 20);
  }

  approvalAnswer(id: number, allow: boolean, duration: AnswerDuration, hours?: number) {
    const req = this.approvals.find((a) => a.id === id);
    if (!req) return this.reply(false);
    this.approvals = this.approvals.filter((a) => a.id !== id);
    if (allow && duration !== "once") {
      const until = duration === "always" ? 0 : duration === "session" ? -1 : now() + (hours ?? 1) * 3600;
      const grant: Grant = { capability: req.capability, scope: req.scope, until };
      this.state.grants = this.state.grants.filter((g) => !(g.capability === grant.capability && g.scope === grant.scope));
      this.state.grants.push(grant);
    }
    this.recordAnswer(req, allow);
    this.emit({ type: "approval_closed", id });
    this.emit({ type: "state" });
    return this.reply(true);
  }

  /** The journal line the core writes once a request is decided. */
  private recordAnswer(req: ApprovalRequest, allow: boolean) {
    if (req.capability === "cookies") {
      this.record("cookies", "cookies.get", allow ? `${req.scope} (14 from Chrome)` : req.scope, allow ? "ok" : "denied: owner");
    } else if (req.capability === "net") {
      this.record("net", "dial", `${req.scope}:443`, allow ? "ok" : `denied: the owner did not allow traffic to ${req.scope}`, allow ? 900_000 : undefined);
    } else if (req.capability === "files") {
      this.record("files", "files.write", req.scope, allow ? "ok" : "denied: owner", allow ? 4200 : undefined);
    } else if (req.capability === "computer" && req.scope === "task") {
      this.record("computer", "mcp.open", "desktop", allow ? "ok" : "denied: the owner did not allow computer use for this task");
    } else if (req.capability === "computer") {
      this.record("computer", "tools/call", "click", allow ? "ok" : "denied: the owner did not allow foreground delivery");
    }
  }

  /** Put a helper state in place, as the shell's supervisor does. */
  private setHelper(helper: HelperState, announce = true) {
    this.state.helper = helper;
    const ready = helper.state === "ready";
    this.state.core.cua_version = ready ? helper.version : "";
    this.state.core.cua_status = ready ? "ok" : "not_installed";
    if (ready || helper.state === "failed") {
      if (ready) this.state.helper_retry_at = null;
      if (announce) this.emit({ type: "state" });
    }
    if (announce) this.emit({ type: "helper", state: structuredClone(helper) });
  }

  /** The automatic install from start to finish: check, download with
   * progress, unpack, verify, ready. About seven seconds. */
  private playInstall(startAfter = 0) {
    this.installTimers.forEach((t) => window.clearTimeout(t));
    this.installTimers = [];
    const total = 46_662_186;
    const at = (ms: number, helper: HelperState) =>
      this.installTimers.push(window.setTimeout(() => this.setHelper(helper), startAfter + ms));
    this.state.helper_retry_at = null;
    at(0, { state: "checking" });
    at(500, { state: "downloading", received: 0, total });
    for (let i = 1; i <= 20; i++) {
      at(500 + i * 220, { state: "downloading", received: Math.round((total * i) / 20), total });
    }
    at(5100, { state: "installing" });
    at(5900, { state: "verifying" });
    this.installTimers.push(
      window.setTimeout(() => {
        this.setHelper({ state: "ready", version: "0.33.1" });
        this.record("computer", "helper.install", "0.33.1", "ok");
      }, startAfter + 6700),
    );
  }

  cuaCheck(): Promise<CuaStatus> {
    return this.reply(
      {
        helper: structuredClone(this.state.helper),
        retry_at: this.state.helper_retry_at,
        pinned_version: "0.33.1",
        ...this.permissions,
      },
      80,
    );
  }

  cuaGrantPermissions() {
    // The real button raises the system dialogs; here the answers "land"
    // a moment later so the live status check has something to show.
    if (this.permissions.accessibility === null) return this.reply(undefined);
    window.setTimeout(() => {
      this.permissions.accessibility = true;
    }, 2500);
    window.setTimeout(() => {
      this.permissions.screen_recording = true;
    }, 5000);
    return this.reply(undefined);
  }

  /** "Try again now". */
  cuaInstall() {
    const h = this.state.helper;
    if (h.state === "failed" && h.code !== "unsupported") this.playInstall();
    return this.reply(undefined);
  }

  /** Review control: start the automatic install over, or break it. */
  simulateHelper(what: "install" | "fail") {
    if (what === "install") this.playInstall();
    else {
      this.installTimers.forEach((t) => window.clearTimeout(t));
      this.state.helper_retry_at = now() + 120;
      this.setHelper({ state: "failed", code: "offline", detail: FAIL_DETAIL.offline! });
    }
  }

  setLanguage(language: string | null, _resolved: string) {
    this.state.prefs.language = language;
    return this.changed();
  }

  async setAutostart(enabled: boolean) {
    this.state.prefs.autostart = enabled;
    return this.changed();
  }

  async setStopShortcut(accelerator: string) {
    await this.reply(null, 120);
    // One combination is "taken" so the error state can be reviewed.
    if (accelerator === "CommandOrControl+Shift+KeyQ") throw "shortcut_unavailable: already registered";
    this.state.prefs.stop_shortcut = accelerator;
    this.state.prefs.stop_shortcut_active = accelerator !== "";
    return this.changed();
  }

  async checkUpdate() {
    await this.reply(null, 900);
    if (this.update === "error") throw "update_failed: could not reach the update server";
    return structuredClone(this.update);
  }

  async installUpdate() {
    await this.reply(null, 1800);
    this.state.app.version = "0.2.0";
    this.update = { status: "up_to_date", version: null, notes: null };
    this.emit({ type: "state" });
  }

  diagnostics() {
    const c = this.state.core;
    return this.reply(
      [
        `SNTH Connect ${this.state.app.version}`,
        `system: ${this.state.app.os} ${this.state.app.arch}`,
        `hub: ${this.state.app.hub_url}`,
        `key storage: ${this.state.app.secret_store}`,
        "",
        `linked: ${c.paired} (state ${c.link_state}, paused ${c.paused})`,
        `link detail: ${this.state.link_detail}`,
        `synth: ${c.synth_id ?? "-"} (version ${c.synth_version ?? "-"})`,
        `settings revision: ${c.settings_rev}, preset ${c.settings.preset}`,
        `Cua Driver: ${c.cua_version || "not installed"} (status ${c.cua_status})`,
      ].join("\n"),
    );
  }

  openExternal(target: string) {
    console.info(`[mock] would open: ${target}`);
    return this.reply(undefined);
  }
}

/** What the core would put into `detail` (diagnostics wording). */
const FAIL_DETAIL: Partial<Record<HelperFailCode, string>> = {
  offline: "github.com: Dns Failed: resolve dns name 'github.com:443'",
  download: "the server answered HTTP 503",
  checksum: "cua-driver-rs-0.33.1-darwin-universal-binary.tar.gz has sha256 1f0c..., expected 8bb7...",
  disk: "move the new copy into place: No space left on device (os error 28)",
  verify: 'the unpacked driver reports version "", expected 0.33.1',
  unsupported: "no Cua Driver release for freebsd x86_64",
  timeout: "not finished after 900 s",
};

function cleanDomain(raw: string): string | null {
  let d = raw.trim().toLowerCase();
  const scheme = d.indexOf("://");
  if (scheme >= 0) d = d.slice(scheme + 3);
  d = d.split(/[/?#:]/)[0] ?? "";
  d = d.replace(/^\*\./, "").replace(/^www\./, "").replace(/^\.+|\.+$/g, "");
  return d.includes(".") && !/\s/.test(d) ? d : null;
}

export type { MockBackend };

let instance: MockBackend | undefined;

/** Created on first use, so the real app never builds it. */
export function getMock(): MockBackend {
  instance ??= new MockBackend();
  return instance;
}
