// The native half of the Safari extension that ships inside SNTH
// Connect.app. Safari calls `beginRequest` once for every
// `browser.runtime.sendNativeMessage` of the extension's own background
// script or pages (never for a web page or another extension), in a
// sandboxed process of its own.
//
// It is a relay and nothing more: the message goes to the running app over
// the socket in the shared app group folder, the answer comes back. All of
// it is ConnectRelay (../Sources/ConnectRelay), the same code the tests
// and the test host run.

import Foundation
import SafariServices

final class SafariWebExtensionHandler: NSObject, NSExtensionRequestHandling {
    /// One relay for the life of this process: Safari keeps the process
    /// between calls, and each call gets a handler object of its own.
    private static let hub = RelayHub(config: ConnectApp.extensionConfig())
    /// An answer may be held for up to 25 seconds; never on the main thread.
    private static let queue = DispatchQueue(label: "ai.snth.connect.safari.relay", attributes: .concurrent)

    func beginRequest(with context: NSExtensionContext) {
        let item = context.inputItems.first as? NSExtensionItem
        let userInfo = item?.userInfo
        SafariWebExtensionHandler.queue.async {
            let response = NSExtensionItem()
            response.userInfo = ConnectApp.answer(userInfo: userInfo, key: SFExtensionMessageKey, hub: SafariWebExtensionHandler.hub)
            context.completeRequest(returningItems: [response], completionHandler: nil)
        }
    }
}
