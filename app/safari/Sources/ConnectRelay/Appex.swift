// What the Safari extension inside SNTH Connect.app is made of, apart from
// the five lines Safari calls (Extension/SafariWebExtensionHandler.swift).
//
// The names here are repeated in three other places, and the release
// script fails when they differ: the app's side
// (crates/connect-host/src/safari.rs), the entitlements both programs are
// signed with (app/src-tauri/Entitlements.plist,
// app/safari/Extension/Extension.entitlements) and the extension's
// Info.plist.

import Foundation

public enum ConnectApp {
    /// The app group the app and its Safari extension share: the team id,
    /// a dot, a name. This form needs no registration and no provisioning
    /// profile on macOS; the system checks that whoever opens the group's
    /// folder is signed by that team.
    public static let group = "CZAP38JFAE.ai.snth.connect"

    /// The extension's bundle id (prefixed by the app's, as macOS requires
    /// of an extension inside an app).
    public static let extensionId = "ai.snth.connect.safari"

    /// What the app is told a connection from here is for. Not a web
    /// origin (Safari gives every installation a random one): the bundle
    /// id of the one extension that can run this code.
    public static let origin = "safari-web-extension://ai.snth.connect.safari/"

    /// The app's socket, inside the group's folder.
    public static let socketInGroup = "run/app.sock"

    /// Where the app listens, as this process may reach it. Inside the
    /// extension's sandbox the group's folder is the one place outside its
    /// own container it can open.
    public static func socketPath(group: String = ConnectApp.group) -> String? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: group)?
            .appendingPathComponent(socketInGroup)
            .path
    }

    /// The relay as the extension runs it: the same user only.
    public static func extensionConfig() -> RelayConfig {
        RelayConfig(socketPath: { ConnectApp.socketPath() }, policy: .currentUser, origin: origin)
    }

    /// The handler's whole job: the message Safari delivered under `key`
    /// (SFExtensionMessageKey) goes to the relay, and its answer goes back
    /// under the same key.
    public static func answer(userInfo: [AnyHashable: Any]?, key: String, hub: RelayHub) -> [AnyHashable: Any] {
        [key: hub.reply(to: userInfo?[key])]
    }
}
