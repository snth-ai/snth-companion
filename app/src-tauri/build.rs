// Every command the webview may call is listed here. Listing them turns on
// Tauri's per-command permissions for the app's own commands: a window can
// invoke a command only if one of its capability files
// (capabilities/*.json) carries the matching `allow-<command>` entry.
// A command that is in `invoke_handler` but not listed here would be
// callable from every window, so keep the two lists in step.
const COMMANDS: &[&str] = &[
    // state
    "get_state",
    "get_journal",
    "ui_ready",
    // pairing
    "pair_begin",
    "pair_confirm",
    "pair_cancel",
    "unpair",
    "finish_onboarding",
    // settings
    "apply_preset",
    "update_settings",
    "suggested_roots",
    "add_suggested_root",
    "pick_folder",
    "remove_root",
    "set_root_write",
    "revoke_grant",
    // pause
    "pause",
    "resume",
    // approvals
    "approvals_pending",
    "approval_answer",
    // computer control helper
    "cua_check",
    "cua_grant_permissions",
    "cua_install",
    // general
    "set_language",
    "set_autostart",
    "set_stop_shortcut",
    "check_update",
    "install_update",
    "diagnostics",
    "open_external",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build script failed");
}
