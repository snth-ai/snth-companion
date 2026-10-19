import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend, UiEvent } from "./types";

const CHANNEL = "connect:event";

/** The real backend: one Tauri command per method, one event channel. */
export const tauriBackend: Backend = {
  mock: false,
  getState: () => invoke("get_state"),
  getJournal: () => invoke("get_journal"),
  uiReady: (language) => invoke("ui_ready", { language }),

  pairBegin: (code, hubUrl) => invoke("pair_begin", { code, hubUrl: hubUrl ?? null }),
  pairConfirm: () => invoke("pair_confirm"),
  pairCancel: () => invoke("pair_cancel"),
  unpair: () => invoke("unpair"),
  unpairSynth: (synthId) => invoke("unpair_synth", { synthId }),
  reconnectSynth: (synthId) => invoke("reconnect_synth", { synthId }),
  finishOnboarding: (autostart) => invoke("finish_onboarding", { autostart }),

  applyPreset: (preset) => invoke("apply_preset", { preset }),
  updateSettings: (settings) => invoke("update_settings", { settings }),
  suggestedRoots: () => invoke("suggested_roots"),
  addSuggestedRoot: (path, write) => invoke("add_suggested_root", { path, write }),
  pickFolder: (write) => invoke("pick_folder", { write }),
  removeRoot: (path) => invoke("remove_root", { path }),
  setRootWrite: (path, write) => invoke("set_root_write", { path, write }),
  revokeGrant: (capability, scope, synthId) => invoke("revoke_grant", { capability, scope, synthId }),

  loginCancel: () => invoke("login_cancel"),
  accountConnect: () => invoke("account_connect"),
  accountSync: () => invoke("account_sync"),
  signOut: () => invoke("sign_out"),

  pause: () => invoke("pause"),
  resume: () => invoke("resume"),

  approvalsPending: () => invoke("approvals_pending"),
  approvalAnswer: (id, allow, duration, hours) =>
    invoke("approval_answer", { id, allow, duration, hours: hours ?? null }),

  cuaCheck: () => invoke("cua_check"),
  cuaGrantPermissions: () => invoke("cua_grant_permissions"),
  cuaInstall: () => invoke("cua_install"),
  computerApps: () => invoke("computer_apps"),
  cuaHealthCheck: () => invoke("cua_health_check"),

  appsCheck: () => invoke("apps_check"),
  appsRequestPermission: (app) => invoke("apps_request_permission", { app }),

  loginsRefresh: (synthId) => invoke("logins_refresh", { synthId }),
  safariCheck: () => invoke("safari_check"),
  vaultForget: (site, synthId) => invoke("vault_forget", { site, synthId }),
  browserLink: (install, synthId) => invoke("browser_link", { install, synthId }),
  browserUnlink: (install) => invoke("browser_unlink", { install }),
  browserForget: (install) => invoke("browser_forget", { install }),

  setLanguage: (language, resolved) => invoke("set_language", { language, resolved }),
  setAutostart: (enabled) => invoke("set_autostart", { enabled }),
  setStopShortcut: (accelerator) => invoke("set_stop_shortcut", { accelerator }),
  checkUpdate: () => invoke("check_update"),
  installUpdate: () => invoke("install_update"),
  setAutoUpdate: (enabled) => invoke("set_auto_update", { enabled }),
  setDeveloper: (enabled) => invoke("set_developer", { enabled }),
  diagnostics: () => invoke("diagnostics"),
  openExternal: (target) => invoke("open_external", { target }),

  subscribe(listener) {
    let stopped = false;
    let unlisten: (() => void) | undefined;
    void listen<UiEvent>(CHANNEL, (e) => listener(e.payload)).then((fn) => {
      if (stopped) fn();
      else unlisten = fn;
    });
    return () => {
      stopped = true;
      unlisten?.();
    };
  },
};
