// The shape every message must have, and its two wire forms on this side:
// a JSON value inside a Safari native message (from the extension), and one
// JSON object per line on the socket to the app.
//
// The same rules as the helper of the Chromium browsers
// (crates/connect-host/src/frame.rs): a message is a JSON OBJECT with a
// string `type`; the types `host` and `host.*` belong to the relay itself
// and nobody else may send them; everything is bounded before it is passed
// on.

import Foundation

public enum Limits {
    /// The largest message the extension may send (one site's cookies).
    public static let fromBrowser = 2 * 1024 * 1024
    /// The largest message the app may send to the extension.
    public static let toBrowser = 64 * 1024
    /// The longest line read from the app: twice the above, so a line the
    /// extension would not get is dropped here and the link stays.
    public static let lineFromApp = toBrowser * 2
    /// Messages in one request from the extension.
    public static let sendBatch = 64
}

public enum Frame {
    static func isReserved(_ type: String) -> Bool {
        type == "host" || type.hasPrefix("host.")
    }

    /// The message type, when the value is a message at all.
    public static func messageType(_ value: Any?) -> String? {
        guard let object = value as? [String: Any], let type = object["type"] as? String else {
            return nil
        }
        // A JSON boolean or number is not a string, whatever the bridge says.
        guard !type.isEmpty, type.utf8.count <= 64 else {
            return nil
        }
        return type
    }

    /// A message that may be passed on: a typed object whose type is not
    /// one of the relay's own.
    public static func checked(_ value: Any?) -> [String: Any]? {
        guard let type = messageType(value), !isReserved(type) else {
            return nil
        }
        return value as? [String: Any]
    }

    /// One message as one line (JSON never holds a raw line break, so a
    /// message is always exactly one line). Nil when it cannot be written
    /// as JSON or is over `max` bytes.
    public static func line(_ message: [String: Any], max: Int) -> Data? {
        guard JSONSerialization.isValidJSONObject(message),
              var data = try? JSONSerialization.data(withJSONObject: message, options: [])
        else {
            return nil
        }
        guard data.count <= max else {
            return nil
        }
        data.append(0x0A)
        return data
    }

    /// A line from the app as a message for the extension. Nil for
    /// anything that is not a typed object, or carries a reserved type.
    public static func parse(line: Data) -> [String: Any]? {
        guard let value = try? JSONSerialization.jsonObject(with: line, options: []) else {
            return nil
        }
        return checked(value)
    }

    /// The relay's own words to the extension, the same ones the helper of
    /// the Chromium browsers says.
    static func hostApp(_ state: String) -> [String: Any] {
        ["type": "host", "app": state]
    }

    static func hostError(_ code: String) -> [String: Any] {
        ["type": "host", "error": code]
    }
}
