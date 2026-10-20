// The shapes the shell sends. They mirror `connect_core` (Snapshot,
// Settings, Entry) and `app/src-tauri/src/state.rs` (UiState).

export type LinkState = "unpaired" | "connecting" | "linked" | "reconnecting" | "paused" | "needs_repair";
/** How a link to a synth was made: through the SNTH account, or with a
 * code from the synth's bot. */
export type LinkSource = "account" | "code";

/** One synth this computer is linked to. */
export interface LinkBrief {
  synth_id: string;
  /** The friendly name (from the synth or the account); null until known. */
  synth_name: string | null;
  synth_url: string;
  synth_version: string | null;
  state: LinkState;
  source: LinkSource;
  /** Why the link is not up, in the core's words (diagnostics only). */
  detail: string;
  /** The account's hub of this synth, when the account knows it. */
  hub_id: string | null;
}

/** The SNTH account this computer is signed in with. */
export interface AccountInfo {
  id: string;
  email: string;
  name: string;
}

/** One synth of the account, from the platform's last answer. */
export interface AccountSynth {
  hub_id: string;
  synth_id: string;
  name: string;
  avatar_url: string | null;
  /** This computer has a link to it. */
  linked: boolean;
  /** The owner disconnected it here: the sync does not reconnect it by itself. */
  skipped: boolean;
  /** Connecting this computer to it failed on the last try (then linked is
   * false): "connect_unavailable" (Connect is off on that synth; `reason`
   * is the synth's own text), "synth_outdated", "synth_unreachable" (tried
   * again at the next sync), or another short code. */
  error: string | null;
  reason: string | null;
}

/** Sign-in through the browser. */
export type LoginPhase =
  | { phase: "idle" }
  | { phase: "waiting"; started_at: number }
  | { phase: "exchanging" }
  | { phase: "failed"; code: "denied" | "expired" | "state_mismatch" | "exchange_failed" | "platform_unreachable" | string };

export interface AccountState {
  signed_in: boolean;
  account: AccountInfo | null;
  login: LoginPhase;
  /** null = never fetched. */
  synths: AccountSynth[] | null;
  /** The owner clicked "Connect" on the consent screen on this computer:
   * new synths of the account connect by themselves from then on. */
  consented: boolean;
  last_sync: number | null;
  sync_error: string | null;
  platform_url: string;
}
export type NetMode = "all" | "ask" | "list";
export type CookiesMode = "ask" | "ask_new" | "auto";
export type ComputerMode = "ask" | "auto" | "away";
export type Foreground = "never" | "idle" | "ask" | "always";
/** The owner's choice of the new way to drive the computer. "default" = as the synth decides. */
export type Experiment = "default" | "on" | "off";
export type Preset = "careful" | "balanced" | "full" | "custom";
export type Capability = "net" | "cookies" | "computer" | "files" | "apps" | "shell";
/** The command line: every command asks, or none does. */
export type ShellMode = "ask" | "auto";
/** When the app asks before a change in the owner's apps: every time,
 * with the option to remember, or never. Reading never asks; sending a
 * message always does. */
export type AppsMode = "ask" | "ask_new" | "auto";
/** The owner's own apps. Which of them exist depends on the system. */
export type AppId = "messages" | "contacts" | "calendar" | "reminders" | "notes" | "shortcuts" | "notify" | "clipboard" | "open";

/** The colour of the synth's cursor. "auto" is the helper's own cursor,
 * which gets a colour per task; the rest are fixed colours. */
export type CursorColor = "auto" | "sky" | "violet" | "rose" | "green" | "amber" | "red" | "graphite";
export type CursorSpeed = "fast" | "normal" | "relaxed";
/** Whether the cursor's animation plays: as the system is set, never
 * (always still), or always. */
export type ReducedMotion = "system" | "always" | "never";

/** Which apps the synth may use through computer control: every app,
 * every app except the ones on a list, or only the ones on a list. */
export type AppsPolicy = "all" | "except" | "only";

/** One app the owner picked. */
export interface AppRef {
  /** What the helper knows the app by: a bundle id on macOS, the path of
   * the program on Windows and Linux. */
  id: string;
  name: string;
  /** A word typed by hand in an older version of the app: it stands for
   * every app whose name or id contains it. Only among the exceptions. */
  loose?: boolean;
}

/** One app in the picker. */
export interface AppChoice {
  id: string;
  name: string;
  running: boolean;
  /** Where the app is, when that is not the id already. */
  path: string | null;
  /** A password manager or a place where the system's security is set. */
  sensitive: boolean;
}

export interface ComputerApps {
  apps: AppChoice[];
  /** What a list of exceptions starts with on this computer. */
  suggested: AppRef[];
}

/** One line of "Check the helper". */
export interface HealthItem {
  /** installed, daemon, pointer, apps, or one of the helper's own checks
   * (platform_supported, tcc_accessibility, ax_capability, ...). */
  id: string;
  verdict: "ok" | "fail" | "skip";
  /** Why, as a fixed word (not_installed, wrong_version, off, paused,
   * no_start, no_answer, switched_off, not_probed, all, except, only_pipe,
   * bounded, not_bounded). */
  code: string;
  /** A version or a number to show next to it. */
  value: string;
}

export interface HealthReport {
  overall: "ok" | "problems" | "not_run";
  items: HealthItem[];
  /** Plain text for "Copy details": no secrets, no window titles, no path
   * outside the app's own data folder. */
  details: string;
}

/** How computer control looks on this screen. The app's own: the synth is
 * told none of it and cannot change it. */
export interface Look {
  cursor: boolean;
  color: CursorColor;
  speed: CursorSpeed;
  /** Seconds before an idle cursor hides: 5, 15, 60, or 0 for never. */
  idle_hide_seconds: number;
  reduced_motion: ReducedMotion;
  /** The largest screenshot the synth gets (long edge, pixels): 1024,
   * 1568, 2048, or 0 for no limit of the owner's. */
  screenshot_max: number;
  /** The helper's experimental preview window (macOS only). */
  preview_window: boolean;
}

/** Which look settings the helper follows on this computer. */
export interface LookSupport {
  cursor: boolean;
  color: boolean;
  /** Speed, idle hide and reduced motion. */
  motion: boolean;
  preview_window: boolean;
}

export interface Root {
  path: string;
  /** false = read only, true = read and write. */
  write: boolean;
}

export interface Settings {
  net: {
    enabled: boolean;
    mode: NetMode;
    domains: string[];
    allow_local_network: boolean;
    /** 0 = no limit. */
    monthly_cap_bytes: number;
    skip_on_metered: boolean;
  };
  cookies: { enabled: boolean; mode: CookiesMode };
  computer: {
    enabled: boolean;
    mode: ComputerMode;
    foreground: Foreground;
    away_idle_minutes: number;
    /** Which apps the synth may use. Both lists are kept, so switching
     * between the choices loses nothing. */
    apps: { policy: AppsPolicy; except: AppRef[]; only: AppRef[] };
    tool_denylist: string[];
    look: Look;
    experiment: Experiment;
  };
  files: { enabled: boolean; roots: Root[]; ask_on_write: boolean };
  apps: {
    enabled: boolean;
    mode: AppsMode;
    /** Only the switches the owner moved away from the default (on,
     * except Messages). */
    apps: Record<string, boolean>;
  };
  /** The command line. Off in every preset; its switch shows only in
   * developer mode. */
  shell: { enabled: boolean; mode: ShellMode };
  preset: Preset;
}

/** One app this computer has, with the owner's switch. */
export interface AppView {
  id: AppId;
  on: boolean;
  actions: string[];
}

export type AppPermissionKind = "automation" | "full_disk";
/** granted / denied (only System Settings changes it) / not_asked (the
 * system asks on first use) / unknown (cannot be told right now). */
export type AppPermissionState = "granted" | "denied" | "not_asked" | "unknown";

export interface AppPermission {
  kind: AppPermissionKind;
  state: AppPermissionState;
}

export interface AppStatus extends AppView {
  /** The system permissions this app needs. Empty = none. */
  permissions: AppPermission[];
}

export interface AppsStatus {
  apps: AppStatus[];
}

export interface Snapshot {
  /** Every synth this computer is linked to. */
  links: LinkBrief[];
  /** The name of the synth that controls the computer right now (one at a
   * time); null when nobody does. */
  computer_driver: string | null;
  /** Derived: paused > linked (any) > connecting (any) > reconnecting (any)
   * > needs_repair (all) > unpaired. */
  link_state: LinkState;
  paired: boolean;
  synth_id: string | null;
  synth_name: string | null;
  synth_url: string | null;
  synth_version: string | null;
  idle_seconds: number;
  screen_locked: boolean;
  paused: boolean;
  metered: boolean;
  settings: Settings;
  settings_rev: number;
  cua_version: string;
  cua_status: string;
  /** The name the synth gave the computer-control task it is running, as
   * the helper shows it under the synth's pointer (28 characters at
   * most). The synth's own words. null when nothing is running. */
  computer_task: string | null;
  /** "YYYY-MM" */
  egress_month: string;
  egress_bytes: number;
}

/** Why the helper could not be installed. */
export type HelperFailCode =
  | "offline"
  | "download"
  | "checksum"
  | "disk"
  | "verify"
  | "unsupported"
  | "timeout"
  | "cancelled";

/** The helper for computer control. The app installs it by itself. */
export type HelperState =
  | { state: "idle" | "checking" | "installing" | "verifying" }
  | { state: "downloading"; received: number; total: number | null }
  | { state: "ready"; version: string }
  | { state: "failed"; code: HelperFailCode; detail: string };

export interface Grant {
  /** The synth the answer was given to (answers are kept per synth). */
  synth_id: string;
  synth_name: string | null;
  capability: string;
  scope: string;
  /** 0 = always, -1 = until the app quits, else the unix second it ends. */
  until: number;
}

export interface Pending {
  synth_id: string;
  synth_url: string;
  synth_name: string | null;
  /** Only when the code went through the signed-in account: the synth is
   * now in the account (true) or belongs to another account (false). */
  linked_to_account: boolean | null;
  /** "synth_linked_elsewhere": the synth belongs to another account; the
   * link to this computer still works. */
  reason: string | null;
}

/** A browser we know by name, or "browser" for one we do not. */
export type BrowserId = "chrome" | "edge" | "brave" | "chromium" | "safari" | "browser";

/** Safari (macOS): where the extension that ships inside this app stands.
 * on / off: switched on or off in Safari's settings. unknown: Safari did
 * not say. translocated: the app runs from the disk image, and Safari
 * cannot keep an extension from there. */
export interface SafariInfo {
  enabled: "on" | "off" | "unknown" | "translocated";
  /** The stand-alone "SNTH Vault" app for Safari is installed too: Safari
   * lists two extensions. */
  standalone: boolean;
}

/** One browser on the Logins page. */
export interface BrowserRow {
  browser: BrowserId;
  name: string;
  /** The extension's id for its browser profile. null = no extension has
   * talked to the app from this browser. */
  install: string | null;
  /** missing: no extension heard from. installed: heard from, not linked.
   * linked: the owner linked it and it can share logins with the synth. */
  state: "missing" | "installed" | "linked";
  /** The extension is talking to the app right now. */
  connected: boolean;
  ext_version: string;
  /** The sites it may hand a sign-in for. */
  sites: string[];
  /** The owner let the extension read every site in the browser's own
   * settings: that browser asks nothing per site any more. */
  all_sites: boolean;
  /** Unix seconds of its last hello. */
  last_seen: number;
  /** Safari's row only. For a "missing" row it says what is left to do. */
  safari: SafariInfo | null;
  /** The synth a linked browser belongs to. */
  synth_id: string | null;
  synth_name: string | null;
}

/** One login the synth's vault holds. Never the login itself. */
export interface VaultSite {
  site: string;
  has_totp: boolean;
  /** Unix seconds; 0 = unknown / never. */
  updated: number;
  last_used: number;
  use_count: number;
}

/** What the synth's vault holds, as far as the app knows.
 * unknown: not asked yet. offline: the synth cannot be asked right now.
 * outdated: the synth is older than this feature. unavailable: the synth
 * has no vault. */
export type VaultView =
  | { state: "unknown" | "offline" | "outdated" | "unavailable" | "failed" }
  | { state: "ready"; sites: VaultSite[]; devices: number; fetched_at: number };

export interface LoginsState {
  browsers: BrowserRow[];
  /** The browser link is being served by this copy of the app. */
  link_ready: boolean;
  vault: VaultView;
  /** The synth whose vault `vault` describes. */
  vault_synth: string | null;
}

// --- the island ---

/** A synth's face (FaceConfig v1): the face module's schema. */
export type { FaceConfig } from "../island/face/schema";
import type { FaceConfig } from "../island/face/schema";

export type HotkeyKind = "voice" | "screen_voice" | "text";
export interface Hotkeys {
  voice: string;
  screen_voice: string;
  text: string;
}

export type MicPermission = "granted" | "denied" | "not_asked" | "unknown";

/** Who turns the owner's voice into text: the synth (the default, it
 * hears the tone of voice too) or this computer (faster). */
export type SttMode = "synth" | "device";
/** The synth's one-time offer of on-device speech-to-text. */
export type SttOffer = "pending" | "shown" | "accepted" | "declined";

export interface IslandSettings {
  /** Off by default (a test feature). */
  enabled: boolean;
  /** The synth the keys talk to; "" = the first linked synth. */
  main_synth: string;
  /** Accelerators; "" = that key is off. */
  hotkeys: Hotkeys;
  stt: SttMode;
  stt_offer: SttOffer;
  /** Wake by name: off by default. */
  wake: boolean;
  /** The owner's extra wake phrase; "" = the name alone. */
  wake_phrase: string;
}

/** Why wake by name does not listen at all yet. */
export type WakeNeed = "island" | "synth" | "mic" | "model" | "loading";
/** Why it paused by itself. */
export type WakePause = "app_paused" | "hold" | "locked" | "mic_busy" | "bt_mic";
/** What wake by name does now. "woke": the name was heard, the request
 * is being recorded (the island shows it as a hold). */
export type WakeStatus =
  | { state: "off" }
  | { state: "waiting"; need: WakeNeed }
  | { state: "listening" }
  | { state: "paused"; reason: WakePause }
  | { state: "woke" };

/** Wake by name on the Island screen. */
export type WakeState = WakeStatus & {
  /** The mode exists here (a Mac with Apple silicon); hidden elsewhere. */
  available: boolean;
  /** Its own small model (whisper tiny + the voice detector). */
  model: SttModelStatus;
  size: number;
  /** The names it answers to ("Mia", "Мия", "Миа", the extra phrase). */
  names: string[];
  /** Wakes since the app started, and those that sent nothing. */
  wakes: number;
  dropped: number;
};

/** The on-device speech-to-text model. */
export type SttModelStatus =
  | { state: "missing" }
  | { state: "downloading"; done: number; total: number }
  | { state: "verifying" }
  | { state: "ready" }
  /** code: no_space (detail "<need> <free>" in bytes), network, checksum, io. */
  | { state: "failed"; code: string; detail: string };

export interface SttState {
  /** The engine is in this build (a Mac with Apple silicon). */
  available: boolean;
  model: SttModelStatus;
  /** The download, in bytes. */
  size: number;
}

/** The speech-to-text card on the island: the synth's offer, then the
 * model's download when it was accepted there. */
export type SttCard =
  | { kind: "offer"; size: number }
  | { kind: "download"; percent: number }
  | { kind: "verifying" }
  | { kind: "ready" }
  | { kind: "failed"; code: string };

/** The Island screen. */
export interface IslandState {
  /** The island can be shown on this system (macOS, Windows). */
  available: boolean;
  settings: IslandSettings;
  /** The synth the keys talk to right now; null without links. */
  main_synth: string | null;
  /** synth_id -> its face; absent = the default preset of its id. */
  faces: Record<string, FaceConfig>;
  /** synth_id -> it takes island messages (announced the feature). */
  supported: Record<string, boolean>;
  /** synth_id -> it takes a voice message as text recognized here. */
  device_stt: Record<string, boolean>;
  /** On-device speech-to-text. */
  stt: SttState;
  /** Wake by name. */
  wake: WakeState;
  /** This system's default keys. */
  defaults: Hotkeys;
  /** The keys are registered with the system right now. */
  keys_active: boolean;
  /** A key the system would not give: "taken" or "bad". */
  key_problems: Partial<Record<HotkeyKind, string>>;
  mic: MicPermission;
}

/** What face.set answered. */
export type FaceSetResult = { result: "saved"; face: FaceConfig } | { result: "invalid"; reason: string };

export interface IslandRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where the island is: the window, the notch in it, the closed shape. */
export interface IslandPlacement {
  window: IslandRect;
  notch: IslandRect;
  pill: IslandRect;
  /** A real notch (the shape merges with it) or one drawn as if. */
  real_notch: boolean;
}

export interface IslandSynth {
  id: string;
  name: string;
  online: boolean;
}

/** The last message the owner sent from the island. */
export interface IslandMessage {
  synth_id: string;
  message_id: string;
  stage: "accepted" | "working" | "done" | "error";
  /** When this stage began (unix ms). */
  at_ms: number;
  /** done: a short summary of the answer; error: a short text. */
  summary: string;
  /** Where the answer went: telegram, tguser, imessage... */
  channel: string;
}

export interface IslandNotice {
  code: string;
  detail: string;
  at_ms: number;
}

/** Everything the island window renders. */
export interface IslandView {
  os: string;
  lang: "en" | "ru";
  placement: IslandPlacement | null;
  synth: IslandSynth | null;
  face: FaceConfig | null;
  supported: boolean;
  synths: IslandSynth[];
  paused: boolean;
  driving: boolean;
  task: string | null;
  hold: "voice" | "screen_voice" | null;
  hold_since_ms: number;
  text_open: boolean;
  sending: boolean;
  message: IslandMessage | null;
  notice: IslandNotice | null;
  approval: ApprovalRequest | null;
  hover: boolean;
  /** A full-screen app covers the screen and the menu bar is away: the
   * island fades out and up, and the shell lets every click through. It
   * comes back with the menu bar, and stays while the owner is busy with
   * it (a hold, the line of text, a request, a fresh answer). */
  hidden: boolean;
  hotkeys: Hotkeys;
  mic: MicPermission;
  /** On-device speech-to-text: the words of the hold so far, then the
   * text that was sent. Null with the speech going to the synth. */
  transcript: string | null;
  /** A hold recognized on this computer runs while the speech model is
   * still loading: no words yet. */
  stt_loading: boolean;
  stt_card: SttCard | null;
  /** Wake by name: the dot in the right ear while it listens. */
  wake: WakeStatus;
}

export type IslandSignal =
  | { type: "view"; view: IslandView }
  | { type: "gaze"; x: number; y: number; agent: boolean }
  | { type: "level"; level: number };

export interface UiState {
  core: Snapshot;
  link_detail: string;
  needs_repair: boolean;
  pending: Pending | null;
  helper: HelperState;
  /** After a failed install: when the app tries again (unix seconds). */
  helper_retry_at: number | null;
  update: UpdateStatus;
  grants: Grant[];
  /** The apps this computer offers (no Mac app on Windows), with the
   * owner's switches. */
  apps: AppView[];
  /** Which "how it looks" settings the helper follows on this computer;
   * the rest are not shown. */
  look_support: LookSupport;
  /** The Logins page: the browsers and what the synth's vault holds. */
  logins: LoginsState;
  /** The SNTH account. */
  account: AccountState;
  /** The island (the Island screen). */
  island: IslandState;
  prefs: {
    onboarded: boolean;
    /** "en" / "ru" when the owner picked one; null = follow the system. */
    language: string | null;
    autostart: boolean;
    stop_shortcut: string;
    stop_shortcut_active: boolean;
    /** Developer mode: the developer section (the command line) shows. */
    developer: boolean;
  };
  app: {
    version: string;
    device_name: string;
    os: string;
    arch: string;
    debug_build: boolean;
    hub_url: string;
    updater_ready: boolean;
    /** The shell can tell a metered connection on this system. */
    metered_detection: boolean;
    secret_store: string;
    cua_pinned_version: string;
    cua_path: string;
    /** The shell the synth's commands run in: pwsh, powershell, zsh... */
    os_shell: string;
  };
}

export interface Entry {
  /** Unix seconds. */
  time: number;
  capability: string;
  action: string;
  target: string;
  outcome: string;
  bytes?: number;
  /** How many items a call in the owner's apps returned. */
  count?: number;
  /** A command: the folder it ran in, how it ended, how long it took. */
  cwd?: string;
  exit_code?: number;
  duration_ms?: number;
  /** The synth this line is about (its name); absent for the app's own lines. */
  synth?: string;
}

/** One fact of the action the owner is asked about, shown in full. */
export interface ApprovalDetail {
  key: string;
  value: string;
}

export interface ApprovalRequest {
  id: number;
  capability: string;
  scope: string;
  /** App-authored English fallback. */
  summary: string;
  /** The synth's own words. Untrusted. */
  reason: string;
  /** What will actually be sent or created (apps only; else empty). */
  details: ApprovalDetail[];
  /** What may be remembered: anything, only "always" for this exact
   * scope, or nothing. */
  remember: "any" | "always" | "never";
  /** The name of the synth that asks; null when it is not a synth (a browser). */
  synth: string | null;
  asked_at_ms: number;
  expires_at_ms: number;
}

export type AnswerDuration = "once" | "session" | "hours" | "always";

export interface SuggestedRoot {
  path: string;
  kind: "desktop" | "downloads";
}

export interface CuaStatus {
  helper: HelperState;
  /** After a failed install: when the app tries again (unix seconds). */
  retry_at: number | null;
  pinned_version: string;
  /** macOS only, null elsewhere: what the system allows this app. */
  accessibility: boolean | null;
  screen_recording: boolean | null;
}

/** Why this copy of the app cannot replace itself. */
export type UpdateBlock = "translocated" | "read_only" | "package";

export type UpdatePhase =
  | { phase: "idle" | "checking" }
  | { phase: "downloading"; version: string; received: number; total: number | null }
  /** ready: downloaded, waiting for a quiet moment. available: not
   * downloaded (the setting is off, or the app cannot replace itself). */
  | { phase: "ready" | "available"; version: string; notes: string | null }
  | { phase: "applying"; version: string }
  | { phase: "failed"; version: string | null; step: "check" | "download" | "install"; detail: string };

/** Updates of the app itself. */
export type UpdateStatus = UpdatePhase & {
  /** The "Update automatically" setting. */
  auto: boolean;
  /** False in a build without the updater key. */
  configured: boolean;
  blocked: UpdateBlock | null;
  /** Unix seconds of the last successful check. */
  last_checked: number | null;
  /** This run is the first after an update from that version. */
  updated_from: string | null;
};

export type ExternalTarget =
  | "privacy_accessibility"
  | "privacy_screen"
  | "privacy_automation"
  | "privacy_full_disk"
  /** The island's microphone. */
  | "privacy_microphone"
  | "helper"
  | "vault_extension"
  /** Safari's settings, at the extension that ships in this app. */
  | "safari_extension"
  | "releases"
  /** Sign in with SNTH: the shell builds the address, starts (or reuses) a
   * waiting sign-in and opens the browser. Again = "open again". */
  | "platform_login";

export type UiEvent =
  | { type: "state" }
  | { type: "helper"; state: HelperState }
  | { type: "update"; status: UpdateStatus }
  | { type: "journal"; entry: Entry }
  | { type: "approval"; request: ApprovalRequest }
  | { type: "approval_closed"; id: number }
  | { type: "pair_link" }
  /** The island asked the main window to show a screen. */
  | { type: "open_screen"; screen: string };

/** Everything the UI can ask of the shell. Two implementations: the real
 * one (Tauri commands) and the in-memory mock used in a plain browser. */
export interface Backend {
  readonly mock: boolean;
  getState(): Promise<UiState>;
  getJournal(): Promise<Entry[]>;
  uiReady(language: string): Promise<{ pair_code: string | null }>;

  pairBegin(code: string, hubUrl?: string): Promise<Pending>;
  pairConfirm(): Promise<UiState>;
  pairCancel(): Promise<void>;
  /** Disconnect from every synth. */
  unpair(): Promise<UiState>;
  /** Disconnect from one synth. */
  unpairSynth(synthId: string): Promise<UiState>;
  /** A link the synth refused: an account link gets a fresh key; a code
   * link rejects with "needs_code" (a new code from the bot replaces it). */
  reconnectSynth(synthId: string): Promise<UiState>;
  finishOnboarding(autostart: boolean): Promise<UiState>;

  applyPreset(preset: Exclude<Preset, "custom">): Promise<UiState>;
  updateSettings(settings: Settings): Promise<UiState>;
  suggestedRoots(): Promise<SuggestedRoot[]>;
  addSuggestedRoot(path: string, write: boolean): Promise<UiState>;
  /** Opens the system folder picker. null = cancelled. */
  pickFolder(write: boolean): Promise<UiState | null>;
  removeRoot(path: string): Promise<UiState>;
  setRootWrite(path: string, write: boolean): Promise<UiState>;
  revokeGrant(capability: string, scope: string, synthId: string): Promise<UiState>;

  /** Stop waiting for the browser sign-in. */
  loginCancel(): Promise<UiState>;
  /** Consent: connect this computer to every synth of the account. */
  accountConnect(): Promise<UiState>;
  /** Ask the platform again for the account's synths. */
  accountSync(): Promise<UiState>;
  /** Sign out: the account's links end here; links made with a code stay. */
  signOut(): Promise<UiState>;

  pause(): Promise<UiState>;
  resume(): Promise<UiState>;

  approvalsPending(): Promise<ApprovalRequest[]>;
  approvalAnswer(id: number, allow: boolean, duration: AnswerDuration, hours?: number): Promise<boolean>;

  cuaCheck(): Promise<CuaStatus>;
  /** macOS shows its own dialogs for the two permissions. */
  cuaGrantPermissions(): Promise<void>;
  /** "Try again now" after a failed helper install. */
  cuaInstall(): Promise<void>;
  /** The apps on this computer, for the "which apps" picker. Takes a few
   * seconds. Rejects with "not_installed" or "helper_failed: ...". */
  computerApps(): Promise<ComputerApps>;
  /** "Check the helper": the helper's own health report. */
  cuaHealthCheck(): Promise<HealthReport>;

  /** The offered apps and where their system permissions stand. */
  appsCheck(): Promise<AppsStatus>;
  /** macOS asks whether SNTH Connect may control this app. */
  appsRequestPermission(app: string): Promise<void>;

  /** Ask the synth which sites its vault holds a login for. */
  loginsRefresh(synthId: string | null): Promise<UiState>;
  /** Ask Safari again whether the owner switched the extension on. */
  safariCheck(): Promise<UiState>;
  /** Remove one saved login from the synth's vault. */
  vaultForget(site: string, synthId: string | null): Promise<UiState>;
  /** Link a browser whose extension is connected. The click is the
   * owner's confirmation. Rejects with the reason's code. */
  browserLink(install: string, synthId: string | null): Promise<UiState>;
  browserUnlink(install: string): Promise<UiState>;
  /** Take a browser that is gone off the list. */
  browserForget(install: string): Promise<UiState>;

  setLanguage(language: string | null, resolved: string): Promise<UiState>;
  setAutostart(enabled: boolean): Promise<UiState>;
  setStopShortcut(accelerator: string): Promise<UiState>;
  /** "Check now". Progress after it arrives as `update` events. */
  checkUpdate(): Promise<UiState>;
  /** Install the new version and restart into it now. */
  installUpdate(): Promise<UiState>;
  setAutoUpdate(enabled: boolean): Promise<UiState>;
  /** Developer mode. Off also switches the command line off. */
  setDeveloper(enabled: boolean): Promise<UiState>;
  diagnostics(): Promise<string>;
  openExternal(target: ExternalTarget): Promise<void>;

  // --- the island: the Island screen ---
  islandSet(enabled: boolean | null, mainSynth: string | null): Promise<UiState>;
  /** Err: bad_shortcut, conflict:<voice|screen_voice|text|stop>, shortcut_unavailable. */
  islandHotkeySet(which: HotkeyKind, accelerator: string): Promise<UiState>;
  islandHotkeyReset(which: HotkeyKind): Promise<UiState>;
  faceRefresh(synthId: string): Promise<UiState>;
  faceSet(synthId: string, face: FaceConfig, patch: boolean): Promise<FaceSetResult>;
  /** "Ask <name> to design their own face": a line of text to that synth. */
  islandAskDesign(synthId: string): Promise<void>;
  /** The microphone: make the system ask, or open its privacy settings. */
  islandMic(): Promise<void>;
  /** On-device speech-to-text on or off. Err: unsupported. On fetches the
   * model when it is not here. */
  islandSttSet(mode: SttMode): Promise<UiState>;
  /** Fetch the model (again). Err: no_space, unsupported. */
  islandSttDownload(): Promise<UiState>;
  islandSttCancel(): Promise<UiState>;
  /** Delete the model; the speech goes to the synth again. */
  islandSttDelete(): Promise<UiState>;
  /** Wake by name on or off. Err: unsupported. On fetches its model. */
  islandWakeSet(on: boolean): Promise<UiState>;
  islandWakePhrase(phrase: string): Promise<UiState>;
  /** Fetch its model (again). Err: no_space, unsupported. */
  islandWakeDownload(): Promise<UiState>;
  islandWakeDownloadCancel(): Promise<UiState>;
  /** Delete its model; the mode switches off. */
  islandWakeDelete(): Promise<UiState>;

  // --- the island window ---
  islandView(): Promise<IslandView>;
  islandLayout(hit: IslandRect, faceX: number, faceY: number): Promise<void>;
  islandTextSubmit(text: string): Promise<void>;
  islandTextCancel(): Promise<void>;
  islandSelectSynth(synthId: string): Promise<void>;
  /** "Stop": pause, as the stop shortcut does. */
  islandStop(): Promise<void>;
  islandOpenApp(): Promise<void>;
  /** The synth's offer of on-device speech-to-text: "Turn on" / "Not now". */
  islandSttOffer(accept: boolean): Promise<void>;
  /** Hide the model's download card (it goes on, in Settings). */
  islandSttCardHide(): Promise<void>;
  /** A click on the island while a wake records: cancel it. */
  islandWakeCancel(): Promise<void>;
  /** The island window's own signals: its view, the gaze, the mic level. */
  subscribeIsland(listener: (signal: IslandSignal) => void): () => void;

  subscribe(listener: (event: UiEvent) => void): () => void;
}
