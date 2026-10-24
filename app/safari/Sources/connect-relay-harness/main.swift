// The test host: the relay of the Safari extension, running outside
// Safari. `connectd selftest safari` starts it and plays the extension.
//
// It reads one JSON object per line on stdin,
//     {"id": 1, "message": <what the extension would pass to sendNativeMessage>}
// hands `message` to the same entry point the extension's handler calls
// (ConnectApp.answer), and writes
//     {"id": 1, "reply": <what the extension would get back>}
// Several requests may be open at once, as in Safari. It exits when stdin
// closes.
//
//   --socket <path>      the app's socket (a test's own)
//   --group <id>         or: the socket inside this app group's folder, found
//                        the way the extension finds it
//   --expect-uid <n>     pretend the app must run as this user (a test of
//                        the refusal; the extension always uses its own)
//   --max-wait <seconds> the longest an answer is held (default 25)

import ConnectRelay
import Foundation

var socket: String?
var group: String?
var uid = getuid()
var maxWait: TimeInterval = 25
var args = Array(CommandLine.arguments.dropFirst())
while !args.isEmpty {
    let flag = args.removeFirst()
    guard !args.isEmpty else {
        FileHandle.standardError.write("connect-relay-harness: \(flag) needs a value\n".data(using: .utf8)!)
        exit(2)
    }
    let value = args.removeFirst()
    switch flag {
    case "--socket": socket = value
    case "--group": group = value
    case "--expect-uid": uid = uid_t(value) ?? uid
    case "--max-wait": maxWait = TimeInterval(value) ?? maxWait
    default:
        FileHandle.standardError.write("connect-relay-harness: unknown option \(flag)\n".data(using: .utf8)!)
        exit(2)
    }
}
guard (socket == nil) != (group == nil) else {
    FileHandle.standardError.write("connect-relay-harness: give --socket <path> or --group <id>\n".data(using: .utf8)!)
    exit(2)
}

let path = socket
let groupId = group
var config = RelayConfig(
    socketPath: { path ?? ConnectApp.socketPath(group: groupId ?? ConnectApp.group) },
    policy: PeerPolicy(uid: uid),
    origin: ConnectApp.origin
)
config.maxWait = maxWait
let hub = RelayHub(config: config)
let key = "message"
let out = NSLock()
let work = DispatchQueue(label: "harness", attributes: .concurrent)
let open = DispatchGroup()

while let line = readLine(strippingNewline: true) {
    guard let data = line.data(using: .utf8),
          let request = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) as? [String: Any]
    else {
        continue
    }
    work.async(group: open) {
        var userInfo: [AnyHashable: Any] = [:]
        if let message = request["message"] {
            userInfo[key] = message
        }
        let answer = ConnectApp.answer(userInfo: userInfo, key: key, hub: hub)
        let reply: [String: Any] = ["id": request["id"] ?? NSNull(), "reply": answer[key] ?? NSNull()]
        guard var bytes = try? JSONSerialization.data(withJSONObject: reply, options: []) else {
            return
        }
        bytes.append(0x0A)
        out.lock()
        FileHandle.standardOutput.write(bytes)
        out.unlock()
    }
}
// What is being answered right now is answered; nothing waits longer than
// the relay holds an answer.
hub.shutdown()
_ = open.wait(timeout: .now() + maxWait + 5)
exit(0)
