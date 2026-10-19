// An in-memory stand-in for the shell, used when the UI runs in a plain
// browser (`npm run dev`) or with VITE_MOCK=1. It keeps realistic state,
// answers every command, and can stage any screen from URL parameters:
//
//   ?screen=onboarding&step=1..7     ?screen=status&state=paused
//   ?screen=internet|computer|files|logins|activity|general
//   &logins=ready|empty|asking|offline|outdated|novault|failed
//        (what the synth's vault answers on the Logins page)
//   &ext=mixed|none|installed|linked|closed|nobrowser|busy
//        (the SNTH Vault extension in the browsers: mixed = one linked,
//         one connected but not linked, one never heard from)
//   &safari=off|unknown|on|moved|installed|away|linked|closed|allsites|both|none
//        (macOS: the Safari extension that ships inside the app. off = not
//         switched on in Safari yet (the default); on = switched on, not heard
//         from; moved = the app runs from the disk image; installed / away =
//         heard from, not linked; linked / closed = linked; allsites = Safari
//         lets it read every site; both = the stand-alone SNTH Vault app is
//         installed too; none = this copy of the app has no extension.
//         "Open Safari settings" plays the rest: on, then connected.)
//   &link=ok|synth_offline|synth_outdated|other_synth|busy|failed
//        (what "Link this browser" does)
//   &state=linked|paused|connecting|offline|repair|working|empty
//   &approval=net|cookies|cookiesext|vault|computer|foreground|files|other|queue|
//             message|messageask|event|reminder|note|shortcut|shortcutinput|
//             clipboard|openurl|openfile|openapp
//   &apps=granted|noperm|denied|nodisk   (macOS permissions of the apps;
//         nodisk = Messages may be controlled but its history not read)
//   &messages=on                        (the Messages switch, off by default)
//   &preset=careful|balanced|full
//   &cursor=on|off  &color=auto|sky|violet|rose|green|amber|red|graphite
//   &speed=fast|normal|relaxed  &idlehide=5|15|60|0
//   &motion=system|always|never  &shot=0|1024|1568|2048  &pip=on|off
//        (Computer control, "How it looks": the pointer, its colour, its
//         speed, when it hides, its animation; under Advanced the largest
//         screenshot and the preview window, which exists with &os=macos
//         only. &shot and &pip=on open Advanced.)
//   &which=all|except|only   (Computer control, "Which apps": every app,
//         every app except some, only some; except is the default here)
//   &picker=ok|slow|failed|missing|bare
//        (what "Add apps" finds: the apps of this computer; slow = still
//         reading; failed = the helper did not answer; missing = no helper
//         yet; bare = a computer with no password manager, so a new list
//         of exceptions starts empty)
//   &task=<label>   (with &state=working: the name the synth gave its
//         computer task, shown on Status; default "Mia: flights")
//   &health=ok|noperm|half|nopointer|off|paused|missing|nodaemon|bounded|
//           notbounded|slow|unknown
//        (General, Diagnostics, "Check the helper": what the check finds;
//         unknown = the helper reports a check this app has no words for)
//   &wayland=1   (with &os=linux: a desktop that draws the pointer itself,
//         so only the on/off switch is offered)
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
  AppChoice,
  AppId,
  AppRef,
  ComputerApps,
  HealthItem,
  HealthReport,
  Look,
  LookSupport,
  BrowserRow,
  LoginsState,
  VaultView,
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

/** The helper's own look: what every preset has. */
function defaultLook(): Look {
  return { cursor: true, color: "auto", speed: "normal", idle_hide_seconds: 15, reduced_motion: "system", screenshot_max: 0, preview_window: false };
}

function oneOf<T extends string | number>(raw: string | null, offered: readonly T[], fallback: T): T {
  const found = offered.find((o) => String(o) === raw);
  return found === undefined ? fallback : found;
}

/** The look staged from the URL (`&color=rose&speed=fast` ...). */
function stagedLook(): Look {
  const base = defaultLook();
  return {
    cursor: params.get("cursor") !== "off",
    color: oneOf(params.get("color"), ["auto", "sky", "violet", "rose", "green", "amber", "red", "graphite"] as const, base.color),
    speed: oneOf(params.get("speed"), ["fast", "normal", "relaxed"] as const, base.speed),
    idle_hide_seconds: oneOf(params.get("idlehide"), [5, 15, 60, 0] as const, base.idle_hide_seconds),
    reduced_motion: oneOf(params.get("motion"), ["system", "always", "never"] as const, base.reduced_motion),
    screenshot_max: oneOf(params.get("shot"), [0, 1024, 1568, 2048] as const, base.screenshot_max),
    preview_window: params.get("pip") === "on",
  };
}

/** What the helper follows on each system (`cua_look::support`). */
function lookSupport(os: string): LookSupport {
  const shellPointer = os === "linux" && params.get("wayland") === "1";
  return { cursor: true, color: !shellPointer, motion: !shellPointer, preview_window: os === "macos" };
}

/** The apps the helper would list on each system, for the picker. */
function installedApps(os: string): AppChoice[] {
  const mac: [string, string, boolean, boolean][] = [
    ["Safari", "com.apple.Safari", true, false],
    ["Finder", "com.apple.finder", true, false],
    ["Notes", "com.apple.Notes", true, false],
    ["1Password", "com.1password.1password", true, true],
    ["Calendar", "com.apple.iCal", false, false],
    ["Google Chrome", "com.google.Chrome", false, false],
    ["Keychain Access", "com.apple.keychainaccess", false, true],
    ["Mail", "com.apple.mail", false, false],
    ["Numbers", "com.apple.iWork.Numbers", false, false],
    ["Passwords", "com.apple.Passwords", false, true],
    ["Preview", "com.apple.Preview", false, false],
    ["System Settings", "com.apple.systempreferences", false, true],
    ["Telegram", "ru.keepcoder.Telegram", false, false],
    ["Terminal", "com.apple.Terminal", false, false],
  ];
  const win: [string, string, boolean, boolean][] = [
    ["Microsoft Edge", "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", true, false],
    ["File Explorer", "C:\\Windows\\explorer.exe", true, false],
    ["Notepad", "C:\\Windows\\System32\\notepad.exe", true, false],
    ["1Password", "C:\\Users\\alex\\AppData\\Local\\1Password\\app\\8\\1Password.exe", false, true],
    ["Excel", "C:\\Program Files\\Microsoft Office\\root\\Office16\\EXCEL.EXE", false, false],
    ["Settings", "C:\\Windows\\ImmersiveControlPanel\\SystemSettings.exe", false, true],
    ["Telegram", "C:\\Users\\alex\\AppData\\Roaming\\Telegram Desktop\\Telegram.exe", false, false],
    ["Windows Security", "C:\\Windows\\System32\\SecurityHealthHost.exe", false, true],
  ];
  const linux: [string, string, boolean, boolean][] = [
    ["Firefox", "/usr/lib/firefox/firefox", true, false],
    ["Files", "/usr/bin/nautilus", true, false],
    ["KeePassXC", "/usr/bin/keepassxc", false, true],
    ["Passwords and Keys", "/usr/bin/seahorse", false, true],
    ["Text Editor", "/usr/bin/gnome-text-editor", false, false],
  ];
  const rows = os === "macos" ? mac : os === "windows" ? win : linux;
  return rows.map(([name, id, running, sensitive]) => ({
    id,
    name,
    running,
    // macOS: the id is the bundle id, and the app has a place as well.
    path: os === "macos" ? `/Applications/${name}.app` : null,
    sensitive,
  }));
}

/** What "Check the helper" finds, staged. */
function healthReport(stage: string, state: UiState): HealthReport {
  const mac = state.app.os === "macos";
  const item = (id: string, verdict: HealthItem["verdict"], code = "", value = ""): HealthItem => ({ id, verdict, code, value });
  const scope = state.core.settings.computer.apps;
  const header = [`SNTH Connect ${state.app.version}: helper check`, `system: ${state.app.os} ${state.app.arch}`];
  const done = (overall: HealthReport["overall"], items: HealthItem[], lines: string[]): HealthReport => ({
    overall,
    items,
    details: [...header, `overall: ${overall}`, ...lines].join("\n"),
  });
  if (stage === "missing") {
    return done("not_run", [item("installed", "fail", "not_installed", "0.34.1")], ["[fail] installed: no helper found (expected 0.34.1)"]);
  }
  const installed = item("installed", "ok", "", "0.34.1");
  const installedLine = "[ok] installed: cua-driver 0.34.1 (expected 0.34.1)";
  if (stage === "off" || stage === "paused" || !state.core.settings.computer.enabled || state.core.paused) {
    const code = stage === "paused" || state.core.paused ? "paused" : "off";
    return done("not_run", [installed, item("daemon", "skip", code)], [installedLine, `[skip] daemon: not started (${code === "paused" ? "the app is paused" : "computer control is switched off"})`]);
  }
  if (stage === "nodaemon") {
    return done(
      "problems",
      [installed, item("daemon", "fail", "no_start")],
      [installedLine, "[fail] daemon: unavailable: start private Cua daemon: the helper daemon was not listening after 20 s"],
    );
  }
  const access = stage !== "noperm" && stage !== "half";
  const screen = stage !== "noperm";
  const items: HealthItem[] = [installed, item("daemon", "ok"), item("platform_supported", "ok")];
  const lines = [
    installedLine,
    "daemon: the app's own, on a private endpoint",
    "[ok] daemon: answers (driver 0.34.1, report ok, schema 1)",
    "[pass] binary_version: cua-driver 0.34.1",
    `[pass] platform_supported: ${mac ? "macOS 26.1 (arm64)" : state.app.os === "windows" ? "Windows 11 (x86_64)" : "Linux (x86_64)"}`,
    "[pass] session_active: MCP session is active.",
  ];
  if (mac) {
    items.push(item("bundle_identity", "ok"), item("tcc_accessibility", access ? "ok" : "fail"), item("tcc_screen_recording", screen ? "ok" : "fail"));
    lines.push(
      "[pass] bundle_identity: Embedded host ai.snth.connect.",
      "       executable_path: <app-data>/cua/releases/0.34.1/cua-driver",
      `[${access ? "pass" : "fail"}] tcc_accessibility: Accessibility is ${access ? "granted" : "NOT granted for this process"}.`,
      `[${screen ? "pass" : "fail"}] tcc_screen_recording: Screen Recording is ${screen ? "granted" : "NOT granted for this process"}.`,
    );
  }
  items.push(item("ax_capability", access ? "ok" : "fail"));
  lines.push(`[${access ? "pass" : "fail"}] ax_capability: ${access ? "AX is trusted and reachable." : "AX is not trusted; UI inspection and event posting will fail."}`);
  if (mac) {
    items.push(item("screen_capture_capability", "skip", "not_probed"));
    lines.push("[skip] screen_capture_capability: Direct ScreenCaptureKit readiness was not probed because health_report is read-only.");
  } else {
    items.push(item("screen_capture_capability", "ok"));
    lines.push("[pass] screen_capture_capability: capture works.");
  }
  if (stage === "unknown") {
    items.push(item("gpu_compositor", "fail"));
    lines.push("[fail] gpu_compositor: compositor handshake failed.");
  }
  if (!state.core.settings.computer.look.cursor) {
    items.push(item("pointer", "skip", "switched_off"));
    lines.push("[skip] pointer: switched off by the owner");
  } else if (stage === "nopointer") {
    items.push(item("pointer", "fail"));
    lines.push("[fail] pointer: agent cursor overlay is unavailable (facility_unavailable)");
  } else {
    items.push(item("pointer", "ok"));
    lines.push("[ok] pointer: enabled true, theme cua.default");
  }
  const count = String(scope.policy === "only" ? scope.only.length : scope.except.length);
  if (stage === "notbounded") {
    items.push(item("apps", "fail", "not_bounded", count));
    lines.push(`[fail] apps: only ${count}, but the helper did not refuse the list of every window (it answered)`);
  } else if (stage === "bounded" || scope.policy === "only") {
    items.push(item("apps", "ok", "bounded", stage === "bounded" && scope.policy !== "only" ? "2" : count));
    lines.push(`[ok] apps: only ${count} (the helper runs in bounded mode and refuses the rest itself)`);
  } else if (scope.policy === "except") {
    items.push(item("apps", "ok", "except", count));
    lines.push(`[ok] apps: every app except ${count} (held by the app, call by call)`);
  } else {
    items.push(item("apps", "ok", "all"));
    lines.push("[ok] apps: every app");
  }
  return done(items.some((i) => i.verdict === "fail") ? "problems" : "ok", items, lines);
}

function presetSettings(preset: Exclude<Preset, "custom">, roots: Settings["files"]["roots"]): Settings {
  const base: Settings = {
    net: { enabled: true, mode: "ask", domains: [], allow_local_network: false, monthly_cap_bytes: 0, skip_on_metered: true },
    cookies: { enabled: true, mode: "ask" },
    computer: { enabled: true, mode: "ask", foreground: "never", away_idle_minutes: 5, apps: { policy: "all", except: [], only: [] }, tool_denylist: [], look: defaultLook(), experiment: "default" },
    files: { enabled: true, roots, ask_on_write: true },
    apps: { enabled: true, mode: "ask", apps: {} },
    // Off in every preset.
    shell: { enabled: false, mode: "ask" },
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
    computer: { enabled: false, mode: "ask", foreground: "never", away_idle_minutes: 5, apps: { policy: "all", except: [], only: [] }, tool_denylist: [], look: defaultLook(), experiment: "default" },
    files: { enabled: false, roots: [], ask_on_write: false },
    apps: { enabled: false, mode: "ask", apps: {} },
    shell: { enabled: false, mode: "ask" },
    preset: "custom",
  };
}

const now = () => Math.floor(Date.now() / 1000);
const GB = 1024 * 1024 * 1024;

function seedJournal(): Entry[] {
  const t = now();
  const cmd = (ago: number, target: string, outcome: string, exit_code: number | undefined, duration_ms: number, bytes: number): Entry => ({
    time: t - ago,
    capability: "shell",
    action: "shell.exec",
    target,
    outcome,
    cwd: `${HOME}/Downloads`,
    ...(exit_code === undefined ? {} : { exit_code }),
    duration_ms,
    bytes,
  });
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
    e(86400 + 6000, "vault", "vault.pair", "Chrome", "ok"),
    e(86400 + 5000, "vault", "vault.forget", "old-shop.example", "ok"),
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
    e(3300, "cookies", "cookies.get", "github.com (9 from chrome-extension)", "ok"),
    e(3200, "vault", "vault.pair", "Microsoft Edge", "denied: owner"),
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
    cmd(1600, "ffmpeg -y -ss 00:00:05 -i trip.mp4 -t 30 -c copy trip-short.mp4", "ok", 0, 2400, 18_200),
    cmd(1500, "ffmpeg -i missing.mp4 out.mp4", "exit 1", 1, 180, 900),
    cmd(1400, "Remove-Item -Recurse old-photos", "denied: owner", undefined, 0, 0),
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
  // The same when a linked browser has to ask too: step 1 of 2.
  cookiesext: {
    capability: "cookies",
    scope: "notion.so",
    summary: "hand the synth your browser cookies for notion.so",
    reason: "You asked me to tidy up your reading list. I need to be signed in as you to see it.",
    details: [{ key: "browser_step", value: "Chrome" }],
  },
  // Not the synth: the owner's own browser asks to be linked.
  vault: {
    capability: "vault",
    scope: "Microsoft Edge",
    summary: "link Microsoft Edge to the synth's login vault",
    reason: "",
    remember: "never",
  },
  // A command on the command line: the whole command and its folder.
  shell: {
    capability: "shell",
    scope: "ffmpeg -y -ss 00:00:05 -i trip.mp4 -t 30 -c copy trip-short.mp4",
    summary: `run a command in ${HOME}/Downloads`,
    reason: "You asked for the first 30 seconds of the trip video, without the shaky start.",
    details: [
      { key: "command", value: "ffmpeg -y -ss 00:00:05 -i trip.mp4 -t 30 -c copy trip-short.mp4" },
      { key: "cwd", value: `${HOME}/Downloads` },
      { key: "shell", value: "zsh" },
    ],
    remember: "always",
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

const HOUR = 3600;
const DAY = 86400;

/** `logins=`: what the synth's vault answers. */
function stagedVault(): VaultView {
  switch (params.get("logins")) {
    case "empty":
      return { state: "ready", sites: [], devices: 0, fetched_at: now() };
    case "asking":
      return { state: "unknown" };
    case "offline":
      return { state: "offline" };
    case "outdated":
      return { state: "outdated" };
    case "novault":
      return { state: "unavailable" };
    case "failed":
      return { state: "failed" };
    default:
      return {
        state: "ready",
        devices: 2,
        fetched_at: now(),
        sites: [
          { site: "github.com", has_totp: true, updated: now() - 12 * DAY, last_used: now() - 3 * HOUR, use_count: 9 },
          { site: "booking.com", has_totp: false, updated: now() - 40 * DAY, last_used: now() - 6 * DAY, use_count: 2 },
          { site: "mail.example.org", has_totp: false, updated: now() - 2 * DAY, last_used: 0, use_count: 0 },
        ],
      };
  }
}

const SAFARI_INSTALL = "5afa5afa5afa5afa5afa5afa5afa5afa";

/** `safari=`: the Safari extension that ships inside the app (macOS). */
function stagedSafari(os: string): BrowserRow | null {
  const stage = params.get("safari") ?? "off";
  if (os !== "macos" || stage === "none") return null;
  const heard = ["installed", "away", "linked", "closed", "allsites", "both"].includes(stage);
  const linked = ["linked", "closed", "allsites"].includes(stage);
  const connected = ["installed", "linked", "allsites", "both"].includes(stage);
  return {
    browser: "safari",
    name: "Safari",
    install: heard ? SAFARI_INSTALL : null,
    state: linked ? "linked" : heard ? "installed" : "missing",
    connected,
    ext_version: heard ? "0.3.0" : "",
    sites: stage === "linked" ? ["github.com"] : [],
    all_sites: stage === "allsites",
    last_seen: heard ? now() - 90 : 0,
    safari: {
      enabled: heard || stage === "on" ? "on" : stage === "moved" ? "translocated" : stage === "unknown" ? "unknown" : "off",
      standalone: stage === "both",
    },
  };
}

/** `ext=`: the SNTH Vault extension in the browsers of this computer,
 * and Safari's row after them. */
function stagedBrowsers(os: string): Pick<LoginsState, "browsers" | "link_ready"> {
  const staged = stagedChromium(os);
  const safari = stagedSafari(os);
  return safari ? { ...staged, browsers: [...staged.browsers, safari] } : staged;
}

function stagedChromium(os: string): Pick<LoginsState, "browsers" | "link_ready"> {
  const row = (browser: BrowserRow["browser"], name: string, state: BrowserRow["state"], connected: boolean, sites: string[] = []): BrowserRow => ({
    browser,
    name,
    install: state === "missing" ? null : `${browser}00000000000000000000000000`.slice(0, 32),
    state,
    connected,
    ext_version: state === "missing" ? "" : "0.2.0",
    sites,
    all_sites: false,
    last_seen: state === "missing" ? 0 : now() - 120,
    safari: null,
  });
  const edge = os === "linux" ? [] : ["edge"];
  switch (params.get("ext")) {
    case "none":
      return { link_ready: true, browsers: [row("chrome", "Chrome", "missing", false), ...edge.map(() => row("edge", "Microsoft Edge", "missing", false))] };
    case "installed":
      return { link_ready: true, browsers: [row("chrome", "Chrome", "installed", true)] };
    case "linked":
      return { link_ready: true, browsers: [row("chrome", "Chrome", "linked", true, ["github.com", "youtube.com"])] };
    case "closed":
      return { link_ready: true, browsers: [row("chrome", "Chrome", "linked", false), row("brave", "Brave", "installed", false)] };
    case "nobrowser":
      return { link_ready: true, browsers: [] };
    case "busy":
      return { link_ready: false, browsers: [row("chrome", "Chrome", "missing", false)] };
    default:
      return {
        link_ready: true,
        browsers: [
          row("chrome", "Chrome", "linked", true, ["youtube.com"]),
          ...edge.map(() => row("edge", "Microsoft Edge", "installed", true)),
          row("brave", "Brave", "missing", false),
        ],
      };
  }
}

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
  /** The owner was sent to Safari's settings: the next checks play them
   * switching the extension on, then the extension saying hello. */
  private safariSteps = 0;
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
    const os = params.get("os") ?? "macos";
    const known = installedApps(os);
    const pickApp = (name: string): AppRef[] => known.filter((a) => a.name === name).map((a) => ({ id: a.id, name: a.name }));
    settings.computer.apps = {
      policy: oneOf(params.get("which"), ["all", "except", "only"] as const, "except"),
      // One picked from the list, one word typed by hand in an older version.
      except: [...pickApp("1Password"), ...pickApp(os === "macos" ? "Keychain Access" : "Settings"), { id: "mail", name: "mail", loose: true }],
      only: [...pickApp(os === "macos" ? "Safari" : "Microsoft Edge"), ...pickApp(os === "macos" ? "Notes" : "Notepad")],
    };
    settings.computer.look = stagedLook();
    // The preview window exists on macOS only.
    if (os !== "macos") settings.computer.look.preview_window = false;
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
        cua_version: "0.34.1",
        cua_status: "ok",
        computer_task: null,
        egress_month: new Date().toISOString().slice(0, 7),
        egress_bytes: 3.4 * GB,
      },
      link_detail: "linked to Mia",
      needs_repair: false,
      pending: null,
      helper: { state: "ready", version: "0.34.1" },
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
      look_support: lookSupport(os),
      // The vault is asked when the Logins page opens (`loginsRefresh`).
      logins: { ...stagedBrowsers(os), vault: { state: "unknown" } },
      prefs: {
        onboarded: true,
        language: params.get("lang"),
        autostart: true,
        stop_shortcut: "CommandOrControl+Alt+Period",
        stop_shortcut_active: true,
        developer: params.get("developer") === "1",
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
        cua_pinned_version: "0.34.1",
        cua_path: `${HOME}/Library/Application Support/ai.snth.connect/cua/releases/0.34.1/cua-driver`,
        os_shell: os === "windows" ? "pwsh" : os === "macos" ? "zsh" : "bash",
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
        // The synth named its task; the label is its own text, cut as the
        // helper cuts it under the pointer.
        s.core.computer_task = (params.get("task") ?? "Mia: flights").slice(0, 28);
        this.startWork();
        break;
      case "empty":
        this.journal = [];
        s.grants = [];
        s.core.settings.net.domains = [];
        s.core.settings.files.roots = [];
        s.core.settings.computer.apps = { policy: "all", except: [], only: [] };
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
    // A preset is a policy: the look and the experiment the owner chose stay.
    // The command line is off in every preset; its mode stays.
    const { look, experiment } = this.state.core.settings.computer;
    const shellMode = this.state.core.settings.shell.mode;
    this.state.core.settings = presetSettings(preset, this.state.core.settings.files.roots);
    this.state.core.settings.computer.look = look;
    this.state.core.settings.computer.experiment = experiment;
    this.state.core.settings.shell.mode = shellMode;
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
    // The command line only with developer mode on, as the shell does.
    if (!this.state.prefs.developer) next.shell.enabled = false;
    next.preset = "custom";
    for (const p of ["careful", "balanced", "full"] as const) {
      // How it looks and the experiment are not part of a preset.
      const probe = presetSettings(p, next.files.roots);
      probe.computer.look = next.computer.look;
      probe.computer.experiment = next.computer.experiment;
      probe.shell.mode = next.shell.mode;
      if (JSON.stringify(probe) === JSON.stringify({ ...next, preset: p })) next.preset = p;
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
    // A site taken back here is taken back in the browsers too.
    if (capability === "cookies") {
      for (const b of this.state.logins.browsers) b.sites = b.sites.filter((s) => s !== scope);
    }
    return this.changed();
  }

  // --- logins: the synth's vault and the browsers ---

  /** The synth answers after a moment (or not at all, with `logins=asking`). */
  async loginsRefresh() {
    if (params.get("logins") === "asking") return new Promise<UiState>(() => undefined);
    const offline = this.state.core.link_state !== "linked" && this.state.core.link_state !== "paused";
    const staged = stagedVault();
    // What was removed in this session stays removed.
    const previous = this.state.logins.vault;
    this.state.logins.vault = offline ? { state: "offline" } : previous.state === "ready" && staged.state === "ready" ? previous : staged;
    return this.reply(this.snapshot(), 450);
  }

  /** Safari is asked again. After "Open Safari settings" the story moves
   * one step per question: switched on, then the extension connects. */
  safariCheck() {
    const row = this.state.logins.browsers.find((b) => b.browser === "safari");
    if (row && row.safari && this.safariSteps > 0 && row.state === "missing" && row.safari.enabled !== "translocated") {
      this.safariSteps += 1;
      if (this.safariSteps === 3) row.safari.enabled = "on";
      if (this.safariSteps >= 5) {
        row.state = "installed";
        row.install = SAFARI_INSTALL;
        row.connected = true;
        row.ext_version = "0.3.0";
        row.last_seen = now();
      }
    }
    return this.reply(this.snapshot(), 120);
  }

  vaultForget(site: string) {
    const vault = this.state.logins.vault;
    if (vault.state === "ready") vault.sites = vault.sites.filter((s) => s.site !== site);
    this.record("vault", "vault.forget", site, "ok");
    this.emit({ type: "state" });
    return this.reply(this.snapshot(), 300);
  }

  async browserLink(install: string) {
    const outcome = params.get("link") ?? "ok";
    await new Promise((r) => window.setTimeout(r, 700));
    const row = this.state.logins.browsers.find((b) => b.install === install);
    if (!row) throw "failed";
    if (outcome !== "ok") {
      this.record("vault", "vault.pair", row.name, `failed: ${outcome}`);
      throw outcome;
    }
    row.state = "linked";
    this.record("vault", "vault.pair", row.name, "ok");
    this.emit({ type: "state" });
    return this.snapshot();
  }

  browserUnlink(install: string) {
    const row = this.state.logins.browsers.find((b) => b.install === install);
    if (row) {
      row.state = "installed";
      row.sites = [];
      this.record("vault", "vault.unpair", row.name, "ok");
    }
    this.emit({ type: "state" });
    return this.reply(this.snapshot(), 200);
  }

  browserForget(install: string) {
    this.state.logins.browsers = this.state.logins.browsers.filter((b) => b.install !== install);
    this.emit({ type: "state" });
    return this.reply(this.snapshot());
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
    if (req.capability === "vault") {
      // The browser that asked is linked once the owner says yes.
      const row = this.state.logins.browsers.find((b) => b.name === req.scope && b.state === "installed");
      if (allow && row) row.state = "linked";
      this.record("vault", "vault.pair", req.scope, allow ? "ok" : "denied: owner");
    } else if (req.capability === "cookies") {
      const viaBrowser = req.details.some((d) => d.key === "browser_step");
      this.record("cookies", "cookies.get", allow ? `${req.scope} (${viaBrowser ? "9 from chrome-extension" : "14 from Chrome"})` : req.scope, allow ? "ok" : "denied: owner");
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
    } else if (req.capability === "shell") {
      // The command, its folder, how it ended. Never its output.
      const cwd = req.details.find((d) => d.key === "cwd")?.value;
      const entry: Entry = { time: now(), capability: "shell", action: "shell.exec", target: req.scope, outcome: allow ? "ok" : "denied: owner", ...(cwd ? { cwd } : {}) };
      if (allow) Object.assign(entry, { exit_code: 0, duration_ms: 2300, bytes: 18_000 });
      this.journal.push(entry);
      this.emit({ type: "journal", entry });
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
        this.setHelper({ state: "ready", version: "0.34.1" });
        this.record("computer", "helper.install", "0.34.1", "ok");
      }, startAfter + 6700),
    );
  }

  cuaCheck(): Promise<CuaStatus> {
    return this.reply(
      {
        helper: structuredClone(this.state.helper),
        retry_at: this.state.helper_retry_at,
        pinned_version: "0.34.1",
        ...this.permissions,
      },
      80,
    );
  }

  computerApps(): Promise<ComputerApps> {
    const stage = params.get("picker") ?? "ok";
    if (stage === "slow") return new Promise<ComputerApps>(() => undefined);
    if (stage === "missing") return this.reply(undefined, 300).then(() => Promise.reject(new Error("not_installed")));
    if (stage === "failed") return this.reply(undefined, 900).then(() => Promise.reject(new Error("helper_failed: the helper did not answer list_apps in time")));
    const apps = installedApps(this.state.app.os).filter((a) => stage !== "bare" || !a.sensitive);
    const suggested = apps.filter((a) => a.sensitive).map((a) => ({ id: a.id, name: a.name }));
    // The real list takes a few seconds on a computer with many apps.
    return this.reply({ apps, suggested }, 1400);
  }

  cuaHealthCheck(): Promise<HealthReport> {
    const stage = params.get("health") ?? "ok";
    if (stage === "slow") return new Promise<HealthReport>(() => undefined);
    return this.reply(healthReport(stage, this.state), 1100);
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

  setDeveloper(enabled: boolean) {
    this.state.prefs.developer = enabled;
    if (!enabled) this.state.core.settings.shell.enabled = false;
    return this.changed();
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
    if (target === "safari_extension") this.safariSteps = Math.max(this.safariSteps, 1);
    return this.reply(undefined);
  }
}

/** What the core would put into `detail` (diagnostics wording). */
const FAIL_DETAIL: Partial<Record<HelperFailCode, string>> = {
  offline: "github.com: Dns Failed: resolve dns name 'github.com:443'",
  download: "the server answered HTTP 503",
  checksum: "cua-driver-rs-0.34.1-darwin-universal-binary.tar.gz has sha256 1f0c..., expected 52de...",
  disk: "move the new copy into place: No space left on device (os error 28)",
  verify: 'the unpacked driver reports version "", expected 0.34.1',
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
