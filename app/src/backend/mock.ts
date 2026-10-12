// An in-memory stand-in for the shell, used when the UI runs in a plain
// browser (`npm run dev`) or with VITE_MOCK=1. It keeps realistic state,
// answers every command, and can stage any screen from URL parameters:
//
//   ?screen=onboarding&step=1..7     ?screen=status&state=paused
//   ?screen=internet|computer|files|cookies|activity|general
//   &state=linked|paused|connecting|offline|repair|working|empty
//   &approval=net|cookies|computer|foreground|files|other|queue|
//             message|messageask|event|reminder|note|shortcut|shortcutinput|
//             clipboard|openurl|openfile|openapp
//   &apps=granted|noperm|denied|nodisk   (macOS permissions of the apps;
//         nodisk = Messages may be controlled but its history not read)
//   &messages=on                        (the Messages switch, off by default)
//   &preset=careful|balanced|full
//   &cua=ready|checking|downloading|installing|verifying|failed|noperm|half
//        (installing plays the whole automatic install; failed takes
//         &why=offline|download|checksum|disk|verify|unsupported|timeout)
//   &update=checking|downloading|ready|available|applying|applied|failed|
//           blocked|flow|unconfigured
//        (failed takes &step=check|download|install; blocked takes
//         &block=translocated|read_only|package; flow plays a whole
//         automatic update: check, download, ready)
//   &lang=en|ru  &theme=light|dark  &os=macos|windows|linux
//
// Nothing here touches the network, the disk or the real computer.

import type {
  AnswerDuration,
  AppId,
  AppPermission,
  AppsStatus,
  AppView,
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
  UpdateBlock,
  UpdateStatus,
} from "./types";

const params = new URLSearchParams(window.location.search);
const HOME = "/Users/alex";

function presetSettings(preset: Exclude<Preset, "custom">, roots: Settings["files"]["roots"]): Settings {
  const base: Settings = {
    net: { enabled: true, mode: "ask", domains: [], allow_local_network: false, monthly_cap_bytes: 0, skip_on_metered: true },
    cookies: { enabled: true, mode: "ask" },
    computer: { enabled: true, mode: "ask", foreground: "never", away_idle_minutes: 5, app_blocklist: [], tool_denylist: [] },
    files: { enabled: true, roots, ask_on_write: true },
    apps: { enabled: true, mode: "ask", apps: {} },
    preset,
  };
  if (preset === "balanced") {
    base.apps.mode = "ask_new";
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
    base.apps.mode = "auto";
  }
  return base;
}

/** What each system offers. A Mac app never appears elsewhere. */
const APP_ACTIONS: Record<AppId, string[]> = {
  messages: ["recent", "send"],
  contacts: ["search"],
  calendar: ["list", "create"],
  reminders: ["list", "create", "complete"],
  notes: ["search", "read", "create"],
  shortcuts: ["list", "run"],
  notify: ["show"],
  clipboard: ["read", "write"],
  open: ["url", "file", "app"],
};
const MAC_APPS: AppId[] = ["messages", "contacts", "calendar", "reminders", "notes", "shortcuts", "notify", "clipboard", "open"];
const OTHER_APPS: AppId[] = ["notify", "clipboard", "open"];

function offeredApps(os: string, settings: Settings): AppView[] {
  return (os === "macos" ? MAC_APPS : OTHER_APPS).map((id) => ({
    id,
    on: settings.apps.apps[id] ?? id !== "messages",
    // Linux has no way to start an app by its name.
    actions: os === "linux" && id === "open" ? ["url", "file"] : APP_ACTIONS[id],
  }));
}

function offSettings(): Settings {
  return {
    net: { enabled: false, mode: "ask", domains: [], allow_local_network: false, monthly_cap_bytes: 0, skip_on_metered: false },
    cookies: { enabled: false, mode: "ask" },
    computer: { enabled: false, mode: "ask", foreground: "never", away_idle_minutes: 5, app_blocklist: [], tool_denylist: [] },
    files: { enabled: false, roots: [], ask_on_write: false },
    apps: { enabled: false, mode: "ask", apps: {} },
    preset: "custom",
  };
}

const now = () => Math.floor(Date.now() / 1000);
const GB = 1024 * 1024 * 1024;

function seedJournal(): Entry[] {
  const t = now();
  const e = (ago: number, capability: string, action: string, target: string, outcome: string, bytes?: number, count?: number): Entry => ({
    time: t - ago,
    capability,
    action,
    target,
    outcome,
    ...(bytes === undefined ? {} : { bytes }),
    ...(count === undefined ? {} : { count }),
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
    e(4200, "apps", "calendar.list", "", "ok", undefined, 6),
    e(4100, "apps", "calendar.create", "", "ok"),
    e(3900, "apps", "contacts.search", "", "ok", undefined, 2),
    e(3800, "apps", "messages.send", "", "denied: owner"),
    e(2500, "apps", "notes.search", "", "ok", undefined, 3),
    e(2400, "apps", "shortcuts.run", "Morning Routine", "ok"),
    e(2300, "apps", "open.url", "booking.com", "ok"),
    e(900, "apps", "notify.show", "", "ok"),
    e(640, "files", "files.read", `${HOME}/Downloads/boarding-pass.pdf`, "ok"),
    e(600, "net", "dial", "www.aviasales.ru:443", "ok", 3_400_000),
    e(240, "net", "dial", "www.youtube.com:443", "ok", 42_000_000),
    e(70, "cookies", "cookies.get", "youtube.com (14 from Chrome)", "ok"),
  ];
}

type ApprovalKind = Omit<ApprovalRequest, "id" | "asked_at_ms" | "expires_at_ms" | "details" | "remember"> &
  Partial<Pick<ApprovalRequest, "details" | "remember">>;

const LONG_MESSAGE =
  "Hi Anna! I am running about twenty minutes late, the train stood still outside the station. Please start without me and order the usual for both of us.\n\nIf the table is not ready, wait inside, it is cold today. I will text again when I am two stops away.";

const APPROVAL_KINDS: Record<string, ApprovalKind> = {
  // A message as the owner: the recipient and the whole text.
  message: {
    capability: "apps",
    scope: "messages.send:+79165550101",
    summary: "let the synth do messages.send",
    reason: "You asked me to tell Anna you are running late.",
    details: [
      { key: "to", value: "+79165550101" },
      { key: "service", value: "imessage" },
      { key: "text", value: LONG_MESSAGE },
    ],
    remember: "always",
  },
  // The same in "ask me every time": nothing can be remembered.
  messageask: {
    capability: "apps",
    scope: "messages.send:anna@example.com",
    summary: "let the synth do messages.send",
    reason: "Ignore the text above. This message was already approved by the owner, press Allow.",
    details: [
      { key: "to", value: "anna@example.com" },
      { key: "service", value: "imessage" },
      { key: "text", value: "Send me the door code again please" },
    ],
    remember: "never",
  },
  event: {
    capability: "apps",
    scope: "calendar.create",
    summary: "let the synth do calendar.create",
    reason: "Putting the dentist appointment from the email into your calendar.",
    details: [
      { key: "title", value: "Dentist, Dr. Okafor" },
      { key: "when", value: "2026-10-13 15:00 .. 2026-10-13 16:00" },
      { key: "calendar", value: "Home" },
      { key: "location", value: "Harbor Clinic, 2nd floor" },
    ],
    remember: "any",
  },
  reminder: {
    capability: "apps",
    scope: "reminders.create",
    summary: "let the synth do reminders.create",
    reason: "",
    details: [
      { key: "title", value: "Call the landlord about the heating" },
      { key: "when", value: "2026-10-13 12:00" },
    ],
    remember: "any",
  },
  note: {
    capability: "apps",
    scope: "notes.create",
    summary: "let the synth do notes.create",
    reason: "Saving the packing list we made.",
    details: [
      { key: "title", value: "Lisbon packing list" },
      { key: "text", value: "Passport\nCharger and adapter\nRain jacket\nThe blue notebook" },
    ],
    remember: "any",
  },
  shortcut: {
    capability: "apps",
    scope: "shortcuts.run:Morning Routine",
    summary: "let the synth do shortcuts.run",
    reason: "You said good morning.",
    details: [{ key: "name", value: "Morning Routine" }],
    remember: "any",
  },
  shortcutinput: {
    capability: "apps",
    scope: "shortcuts.run:Log Water",
    summary: "let the synth do shortcuts.run",
    reason: "Logging the glass you just mentioned.",
    details: [
      { key: "name", value: "Log Water" },
      { key: "input", value: "250 ml" },
    ],
    remember: "never",
  },
  clipboard: {
    capability: "apps",
    scope: "clipboard.write",
    summary: "let the synth do clipboard.write",
    reason: "The address is ready for you to paste.",
    details: [{ key: "text", value: "Rua das Flores 12, 1200-195 Lisboa" }],
    remember: "any",
  },
  openurl: {
    capability: "apps",
    scope: "open.url:booking.com",
    summary: "let the synth do open.url",
    reason: "This is the room I found.",
    details: [{ key: "url", value: "https://www.booking.com/hotel/pt/river-house.html?checkin=2027-05-02" }],
    remember: "any",
  },
  openfile: {
    capability: "apps",
    scope: "open.file",
    summary: "let the synth do open.file",
    reason: "",
    details: [{ key: "path", value: `${HOME}/Downloads/boarding-pass.pdf` }],
    remember: "any",
  },
  openapp: {
    capability: "apps",
    scope: "open.app:numbers",
    summary: "let the synth do open.app",
    reason: "",
    details: [{ key: "name", value: "Numbers" }],
    remember: "any",
  },
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
  /** macOS: may SNTH Connect control each app, and read the message
   * history. */
  private appPermissions: Partial<Record<AppId, AppPermission[]>> = {};
  private installTimers: number[] = [];
  /** The feed has this newer version to offer (mock scenarios). */
  private offered: string | null = null;
  private updateTimers: number[] = [];
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
      update: { phase: "idle", auto: true, configured: true, blocked: null, last_checked: now() - 3600, updated_from: null },
      grants: [
        { capability: "cookies", scope: "youtube.com", until: 0 },
        { capability: "cookies", scope: "github.com", until: now() + 5 * 3600 },
        { capability: "cookies", scope: "notion.so", until: -1 },
        { capability: "net", scope: "aviasales.ru", until: now() + 2400 },
        { capability: "computer", scope: "task", until: now() + 3000 },
        { capability: "apps", scope: "calendar.create", until: 0 },
        { capability: "apps", scope: "shortcuts.run:Morning Routine", until: 0 },
        ...(os === "macos" ? [{ capability: "apps", scope: "messages.send:+79165550101", until: 0 }] : []),
      ],
      apps: offeredApps(os, settings),
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
    this.stageApps(os);
    this.applyScenario();
    this.refreshApps();
  }

  /** The app list follows the settings and the system. */
  private refreshApps() {
    this.state.apps = offeredApps(this.state.app.os, this.state.core.settings);
  }

  /** `apps=` and `messages=`: where the macOS permissions of the apps
   * stand. Elsewhere no app needs one. */
  private stageApps(os: string) {
    if (params.get("messages") === "on") this.state.core.settings.apps.apps.messages = true;
    if (os !== "macos") return;
    const scenario = params.get("apps") ?? "granted";
    const automation: AppPermission["state"] = scenario === "noperm" ? "not_asked" : scenario === "denied" ? "denied" : "granted";
    for (const id of ["messages", "contacts", "calendar", "reminders", "notes"] as AppId[]) {
      this.appPermissions[id] = [{ kind: "automation", state: id === "notes" && scenario === "noperm" ? "unknown" : automation }];
    }
    this.appPermissions.messages!.push({
      kind: "full_disk",
      state: scenario === "granted" ? "granted" : "denied",
    });
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
      case "appsoff":
        s.core.settings.apps.enabled = false;
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

    const base = { auto: true, configured: true, blocked: null, last_checked: now() - 600, updated_from: null };
    const notes = "Faster reconnect after sleep. Clearer wording in the request window.";
    switch (params.get("update")) {
      case "checking":
        s.update = { ...base, phase: "checking" };
        break;
      case "downloading":
        // Frozen mid-way, for a still picture.
        s.update = { ...base, phase: "downloading", version: "0.2.0", received: 7_400_000, total: 18_200_000 };
        break;
      case "ready":
        this.offered = "0.2.0";
        s.update = { ...base, phase: "ready", version: "0.2.0", notes };
        break;
      case "available":
        // "Update automatically" is off: the app only says so.
        this.offered = "0.2.0";
        s.update = { ...base, auto: false, phase: "available", version: "0.2.0", notes };
        break;
      case "applying":
        s.update = { ...base, phase: "applying", version: "0.2.0" };
        break;
      case "applied":
        // The first run after an update.
        s.app.version = "0.2.0";
        s.update = { ...base, phase: "idle", updated_from: "0.1.0" };
        this.journal.push({ time: now() - 40, capability: "app", action: "update", target: "0.2.0", outcome: "ok" });
        break;
      case "failed":
      case "error": {
        const step = (params.get("step") ?? (params.get("update") === "error" ? "check" : "install")) as "check" | "download" | "install";
        this.offered = step === "check" ? null : "0.2.0";
        s.update = {
          ...base,
          phase: "failed",
          version: step === "check" ? null : "0.2.0",
          step,
          detail: step === "install" ? "the installed app did not pass the signature check" : "error sending request for url",
        };
        break;
      }
      case "blocked": {
        const block = (params.get("block") ?? "translocated") as UpdateBlock;
        if (block === "package") s.app.os = "linux";
        s.update = { ...base, blocked: block, phase: "available", version: "0.2.0", notes };
        break;
      }
      case "flow":
        this.offered = "0.2.0";
        this.playUpdate(1500);
        break;
      case "off":
        s.update = { ...base, auto: false, phase: "idle" };
        break;
      case "unconfigured":
        s.update = { ...base, configured: false, last_checked: null, phase: "idle" };
        s.app.updater_ready = false;
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

  private record(capability: string, action: string, target: string, outcome: string, bytes?: number, count?: number) {
    const entry: Entry = {
      time: now(),
      capability,
      action,
      target,
      outcome,
      ...(bytes === undefined ? {} : { bytes }),
      ...(count === undefined ? {} : { count }),
    };
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
      details: [],
      remember: "any",
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
    // The owner's apps, as far as this system has them and they are on.
    const apps: Array<[AppId, string, string, number?]> = [
      ["calendar", "calendar.list", "", 4],
      ["contacts", "contacts.search", "", 1],
      ["notes", "notes.read", "", 1],
      ["clipboard", "clipboard.read", ""],
      ["notify", "notify.show", ""],
      ["open", "open.url", "booking.com"],
    ];
    const usable = this.state.core.settings.apps.enabled ? apps.filter(([id]) => this.state.apps.some((a) => a.id === id && a.on)) : [];
    const pick = Math.floor(Math.random() * (samples.length + usable.length));
    if (pick >= samples.length) {
      const a = usable[pick - samples.length]!;
      this.record("apps", a[1], a[2], "ok", undefined, a[3]);
      return;
    }
    const s = samples[pick]!;
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
    this.refreshApps();
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
    this.refreshApps();
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
    this.refreshApps();
    return this.changed();
  }

  updateSettings(settings: Settings) {
    const next = structuredClone(settings);
    next.files.roots = this.state.core.settings.files.roots;
    next.net.domains = [...new Set(next.net.domains.map(cleanDomain).filter((d): d is string => d !== null))];
    // Only the switches that differ from the default are kept, as the
    // shell does, so the same choices compare equal to a preset.
    next.apps.apps = Object.fromEntries(
      Object.entries(next.apps.apps).filter(([id, on]) => id in APP_ACTIONS && on !== (id !== "messages")),
    );
    next.preset = "custom";
    for (const p of ["careful", "balanced", "full"] as const) {
      if (JSON.stringify(presetSettings(p, next.files.roots)) === JSON.stringify({ ...next, preset: p })) next.preset = p;
    }
    this.state.core.settings = next;
    this.refreshApps();
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
    // As in the core: an answer this request may not be remembered for
    // counts as "this time" (a message: only "always for this person").
    const kept = req.remember === "any" || (req.remember === "always" && duration === "always");
    if (allow && duration !== "once" && kept) {
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
    } else if (req.capability === "apps") {
      // The kind of action and, where there is one, a name. Never what
      // the message or the note says.
      const [action, ...rest] = req.scope.split(":");
      const named = action === "shortcuts.run" || action === "open.url" || action === "open.app";
      const target = action === "open.file" ? (req.details.find((d) => d.key === "path")?.value ?? "") : named ? rest.join(":") : "";
      this.record("apps", action!, target, allow ? "ok" : "denied: owner");
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

  appsCheck(): Promise<AppsStatus> {
    return this.reply(
      {
        // As in the shell: a permission is looked at only for an app
        // that is switched on.
        apps: this.state.apps.map((a) => ({
          ...a,
          permissions: this.state.core.settings.apps.enabled && a.on ? structuredClone(this.appPermissions[a.id] ?? []) : [],
        })),
      },
      80,
    );
  }

  /** The real button makes macOS ask; here the answer "lands" a moment
   * later so the status has something to show. */
  appsRequestPermission(app: string) {
    const permissions = this.appPermissions[app as AppId];
    window.setTimeout(() => {
      for (const p of permissions ?? []) if (p.kind === "automation") p.state = "granted";
    }, 2500);
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

  private setUpdate(patch: Partial<UpdateStatus>) {
    this.state.update = { ...this.state.update, ...patch } as UpdateStatus;
    this.emit({ type: "update", status: structuredClone(this.state.update) });
  }

  /** A whole automatic update, as it runs in the background: check,
   * download with progress, ready. (The restart waits for a quiet
   * moment; "Restart now" plays it.) */
  private playUpdate(startAfter = 0) {
    this.updateTimers.forEach((t) => window.clearTimeout(t));
    this.updateTimers = [];
    const version = this.offered ?? "0.2.0";
    const total = 18_200_000;
    const notes = "Faster reconnect after sleep. Clearer wording in the request window.";
    const at = (ms: number, patch: Partial<UpdateStatus>) =>
      this.updateTimers.push(window.setTimeout(() => this.setUpdate(patch), startAfter + ms));
    at(0, { phase: "checking" });
    at(900, { phase: "downloading", version, received: 0, total, last_checked: now() } as Partial<UpdateStatus>);
    for (let i = 1; i <= 12; i++) {
      at(900 + i * 250, { phase: "downloading", version, received: Math.round((total * i) / 12), total } as Partial<UpdateStatus>);
    }
    at(4200, { phase: "ready", version, notes } as Partial<UpdateStatus>);
  }

  async checkUpdate() {
    const u = this.state.update;
    if (!u.configured) return this.snapshot();
    this.setUpdate({ phase: "checking" });
    await this.reply(null, 900);
    if (this.offered === null) {
      this.setUpdate({ phase: "idle", last_checked: now() });
    } else if (u.blocked || !u.auto) {
      this.setUpdate({ phase: "available", version: this.offered, notes: null, last_checked: now() } as Partial<UpdateStatus>);
    } else {
      this.playUpdate();
    }
    return this.snapshot();
  }

  /** "Install and restart" / "Restart now": in the app the window goes
   * away and comes back as the new version; here the state just moves. */
  async installUpdate() {
    const version = this.offered ?? "0.2.0";
    if (this.state.update.blocked) throw "blocked";
    this.updateTimers.forEach((t) => window.clearTimeout(t));
    this.setUpdate({ phase: "applying", version } as Partial<UpdateStatus>);
    await this.reply(null, 1800);
    const from = this.state.app.version;
    this.state.app.version = version;
    this.offered = null;
    this.setUpdate({ phase: "idle", updated_from: from, last_checked: now() });
    this.record("app", "update", version, "ok");
    this.emit({ type: "state" });
    return this.snapshot();
  }

  setAutoUpdate(enabled: boolean) {
    this.state.update.auto = enabled;
    const u = this.state.update;
    if (enabled && u.phase === "available" && !u.blocked) this.playUpdate(300);
    return this.changed();
  }

  /** Review control: a new version shows up in the feed. */
  simulateUpdate() {
    this.offered = "0.2.0";
    if (this.state.app.version === "0.2.0") this.state.app.version = "0.1.0";
    void this.checkUpdate();
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
