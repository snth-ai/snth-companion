// The shapes the shell sends. They mirror `connect_core` (Snapshot,
// Settings, Entry) and `app/src-tauri/src/state.rs` (UiState).

export type LinkState = "unpaired" | "connecting" | "linked" | "reconnecting" | "paused";
export type NetMode = "all" | "ask" | "list";
export type CookiesMode = "ask" | "ask_new" | "auto";
export type ComputerMode = "ask" | "auto" | "away";
export type Foreground = "never" | "idle" | "ask" | "always";
export type Preset = "careful" | "balanced" | "full" | "custom";
export type Capability = "net" | "cookies" | "computer" | "files";

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
    app_blocklist: string[];
    tool_denylist: string[];
  };
  files: { enabled: boolean; roots: Root[]; ask_on_write: boolean };
  preset: Preset;
}

export interface Snapshot {
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
  capability: string;
  scope: string;
  /** 0 = always, -1 = until the app quits, else the unix second it ends. */
  until: number;
}

export interface Pending {
  synth_id: string;
  synth_url: string;
}

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
  prefs: {
    onboarded: boolean;
    /** "en" / "ru" when the owner picked one; null = follow the system. */
    language: string | null;
    autostart: boolean;
    stop_shortcut: string;
    stop_shortcut_active: boolean;
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
}

export interface ApprovalRequest {
  id: number;
  capability: string;
  scope: string;
  /** App-authored English fallback. */
  summary: string;
  /** The synth's own words. Untrusted. */
  reason: string;
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

export type ExternalTarget = "privacy_accessibility" | "privacy_screen" | "helper" | "releases";

export type UiEvent =
  | { type: "state" }
  | { type: "helper"; state: HelperState }
  | { type: "update"; status: UpdateStatus }
  | { type: "journal"; entry: Entry }
  | { type: "approval"; request: ApprovalRequest }
  | { type: "approval_closed"; id: number }
  | { type: "pair_link" };

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
  unpair(): Promise<UiState>;
  finishOnboarding(autostart: boolean): Promise<UiState>;

  applyPreset(preset: Exclude<Preset, "custom">): Promise<UiState>;
  updateSettings(settings: Settings): Promise<UiState>;
  suggestedRoots(): Promise<SuggestedRoot[]>;
  addSuggestedRoot(path: string, write: boolean): Promise<UiState>;
  /** Opens the system folder picker. null = cancelled. */
  pickFolder(write: boolean): Promise<UiState | null>;
  removeRoot(path: string): Promise<UiState>;
  setRootWrite(path: string, write: boolean): Promise<UiState>;
  revokeGrant(capability: string, scope: string): Promise<UiState>;

  pause(): Promise<UiState>;
  resume(): Promise<UiState>;

  approvalsPending(): Promise<ApprovalRequest[]>;
  approvalAnswer(id: number, allow: boolean, duration: AnswerDuration, hours?: number): Promise<boolean>;

  cuaCheck(): Promise<CuaStatus>;
  /** macOS shows its own dialogs for the two permissions. */
  cuaGrantPermissions(): Promise<void>;
  /** "Try again now" after a failed helper install. */
  cuaInstall(): Promise<void>;

  setLanguage(language: string | null, resolved: string): Promise<UiState>;
  setAutostart(enabled: boolean): Promise<UiState>;
  setStopShortcut(accelerator: string): Promise<UiState>;
  /** "Check now". Progress after it arrives as `update` events. */
  checkUpdate(): Promise<UiState>;
  /** Install the new version and restart into it now. */
  installUpdate(): Promise<UiState>;
  setAutoUpdate(enabled: boolean): Promise<UiState>;
  diagnostics(): Promise<string>;
  openExternal(target: ExternalTarget): Promise<void>;

  subscribe(listener: (event: UiEvent) => void): () => void;
}
