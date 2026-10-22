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
    "unpair_synth",
    "reconnect_synth",
    "finish_onboarding",
    // the platform account
    "login_cancel",
    "account_connect",
    "account_sync",
    "sign_out",
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
    "computer_apps",
    "cua_health_check",
    // the owner's own apps
    "apps_check",
    "apps_request_permission",
    // logins: the vault list and the browsers
    "logins_refresh",
    "safari_check",
    "vault_forget",
    "browser_link",
    "browser_unlink",
    "browser_forget",
    // general
    "set_language",
    "set_autostart",
    "set_stop_shortcut",
    "check_update",
    "install_update",
    "set_auto_update",
    "set_developer",
    "diagnostics",
    "open_external",
    // the island: settings (main window)
    "island_set",
    "island_hotkey_set",
    "island_hotkey_reset",
    "face_refresh",
    "face_set",
    "island_ask_design",
    // on-device speech-to-text: the switch and the model (main window;
    // the island's card may retry the download)
    "island_stt_set",
    "island_stt_engine_set",
    "island_stt_tone_set",
    "island_stt_download",
    "island_stt_cancel",
    "island_stt_delete",
    // wake by name: the switch, the phrase, its model (main window)
    "island_wake_set",
    "island_wake_phrase",
    "island_wake_download",
    "island_wake_download_cancel",
    "island_wake_delete",
    // the island window
    "island_view",
    "island_layout",
    "island_text_submit",
    "island_text_cancel",
    "island_select_synth",
    "island_stop",
    "island_open_app",
    "island_mic",
    "island_stt_offer",
    "island_stt_card_hide",
    "island_wake_cancel",
];

fn main() {
    stt_cfgs();
    link_compiler_rt();
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build script failed");
}

/// Two cfg names for the island's on-device speech-to-text, so the code
/// says `#[cfg(device_stt)]` instead of repeating the target lists. Keep
/// them in step with the target tables of Cargo.toml:
///
///  - `device_stt`: ONNX Runtime (`ort`) is in the build, so Parakeet, the
///    ONNX voice detector and the tone of voice run here: Apple-silicon
///    Macs and Windows x64.
///  - `whisper_stt`: the whisper HOLD engine and whisper.cpp's voice
///    detector exist: Apple-silicon Macs only (whisper.cpp with Metal).
///  - `wake_whisper`: whisper.cpp (`whisper-rs`) is in the build, so wake
///    by name's ears (whisper tiny, on the CPU) run: Apple-silicon Macs
///    and Windows x64. On Windows whisper.cpp is built for the CPU only
///    (no Metal, AVX2 as the floor: .cargo/ggml-x64.cmake) and the
///    voice detector of wake by name is the ONNX one (stt/silero.rs).
fn stt_cfgs() {
    println!("cargo:rustc-check-cfg=cfg(device_stt)");
    println!("cargo:rustc-check-cfg=cfg(whisper_stt)");
    println!("cargo:rustc-check-cfg=cfg(wake_whisper)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let mac_arm = os == "macos" && arch == "aarch64";
    let windows_x64 = os == "windows" && arch == "x86_64";
    if mac_arm || windows_x64 {
        println!("cargo:rustc-cfg=device_stt");
    }
    if mac_arm {
        println!("cargo:rustc-cfg=whisper_stt");
    }
    if mac_arm || windows_x64 {
        println!("cargo:rustc-cfg=wake_whisper");
    }
}

/// whisper.cpp's Metal code (island speech-to-text, Apple silicon only)
/// asks `@available(macOS ...)`, which clang compiles to a call into its
/// runtime library (`__isPlatformVersionAtLeast`). The release build
/// targets macOS 12 (tauri sets MACOSX_DEPLOYMENT_TARGET from
/// minimumSystemVersion), where those checks are real, and rustc does not
/// link clang's runtime: link it from the toolchain that compiled the code.
fn link_compiler_rt() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target != "aarch64-apple-darwin" {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "-print-file-name=libclang_rt.osx.a"]).output();
    let Ok(out) = out else {
        println!("cargo:warning=xcrun clang not found: libclang_rt.osx.a is not linked");
        return;
    };
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    match path.parent().filter(|_| path.is_file()) {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        None => println!("cargo:warning=libclang_rt.osx.a not found: {}", path.display()),
    }
}
