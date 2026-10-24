//! The names the Safari door is built on.
//!
//! Safari has no native messaging host process. Its extension lives in an
//! app extension inside `SNTH Connect.app`
//! (`Contents/PlugIns/SNTH Vault.appex`), and what the extension's
//! JavaScript sends with `browser.runtime.sendNativeMessage` arrives in
//! that app extension's Swift handler (`app/safari`). The handler is
//! sandboxed. It reaches the running app the only way a sandboxed program
//! can reach one that is not: through the folder of an app group both are
//! signed into, where the app binds a second endpoint
//! (`Endpoint::safari`).
//!
//! The same names are written in the Swift side
//! (`app/safari/Sources/ConnectRelay/Appex.swift`), in the entitlements
//! both programs are signed with and in the extension's Info.plist. The
//! release script fails when they differ, and so does a test here.

/// The app group: the team id, a dot, a name. macOS takes this form
/// without a registration or a provisioning profile, and checks that
/// whoever opens the group's folder is signed by that team.
pub const APP_GROUP: &str = "CZAP38JFAE.ai.snth.connect";

/// The bundle id of the app extension (prefixed by the app's own).
pub const EXTENSION_ID: &str = "ai.snth.connect.safari";

/// What the handler names in its first line. Not a web origin (Safari
/// gives every installation a random one): the bundle id of the one
/// extension that runs the handler.
pub const ORIGIN: &str = "safari-web-extension://ai.snth.connect.safari/";

/// The socket, inside the group's folder.
pub const SOCKET_IN_GROUP: &str = "run/app.sock";

/// The app extension inside the app bundle, relative to `Contents`.
pub const APPEX_IN_BUNDLE: &str = "PlugIns/SNTH Vault.appex";

/// The short id and the name of the browser on the Logins page.
pub const BROWSER_ID: &str = "safari";
pub const BROWSER_NAME: &str = "Safari";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_agree_with_each_other() {
        assert_eq!(ORIGIN, format!("safari-web-extension://{EXTENSION_ID}/"));
        assert!(EXTENSION_ID.starts_with(&format!("{}.", crate::HOST_NAME)));
        // "<team id>.<name>": ten capitals or digits, then the app's id.
        let (team, name) = APP_GROUP.split_once('.').unwrap();
        assert_eq!(team.len(), 10);
        assert!(team.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()));
        assert_eq!(name, crate::HOST_NAME);
    }

    /// The Swift side, the entitlements and the Info.plist repeat these
    /// names. They are read here as text so a change on one side only
    /// fails a test on every system, not just on the release Mac.
    #[test]
    fn the_swift_side_and_the_entitlements_use_the_same_names() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app");
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let swift = read("safari/Sources/ConnectRelay/Appex.swift");
        for want in [
            format!("group = \"{APP_GROUP}\""),
            format!("extensionId = \"{EXTENSION_ID}\""),
            format!("origin = \"{ORIGIN}\""),
            format!("socketInGroup = \"{SOCKET_IN_GROUP}\""),
        ] {
            assert!(swift.contains(&want), "Appex.swift lacks: {want}");
        }
        let group = format!("<string>{APP_GROUP}</string>");
        for file in ["src-tauri/Entitlements.plist", "safari/Extension/Extension.entitlements"] {
            assert!(read(file).contains(&group), "{file} does not name the app group");
        }
        let appex = read("safari/Extension/Extension.entitlements");
        assert!(appex.contains("com.apple.security.app-sandbox"), "the extension must be sandboxed");
        // The handler opens a socket in the group folder and nothing else:
        // no network, no files of the user's.
        for never in ["network.client", "network.server", "files.user-selected", "files.downloads"] {
            assert!(!appex.contains(never), "the extension must not be entitled to {never}");
        }
        let plist = read("safari/Extension/Info.plist");
        assert!(plist.contains("com.apple.Safari.web-extension"));
    }
}
