// The relay between the Safari extension and the RUNNING SNTH Connect app.
//
// In the Chromium browsers a helper process sits between the two and holds
// one pipe to the extension and one connection to the app. Safari has no
// such process: the extension calls its native handler with one message
// and gets one answer (`browser.runtime.sendNativeMessage`). So the same
// relay is cut into exchanges:
//
//   extension -> handler   {"type":"relay","v":1,"sid":"<hex>","ack":7,
//                           "send":[<message>, ...],"wait":20000}
//   handler -> extension   {"type":"relay","v":1,"app":true,
//                           "messages":[{"seq":8,"msg":<message>}, ...]}
//
// `sid` names the extension in one Safari profile (the random id it made
// for itself). For each sid the relay keeps one connection to the app,
// exactly as the helper does: the first line is
// `{"type":"host.open","v":1,"origin":...}`, then every well-formed
// message is passed on, both ways. What the app says waits in a numbered
// list until the extension names its number in `ack`, so an answer Safari
// never delivered is sent again and nothing is lost or seen twice; and a
// background script that Safari stopped and started again (it comes back
// with `ack` 0) is handed what arrived while it was gone. `wait` is how
// long the handler may hold the answer when it has nothing to say yet.
// `app` says whether the connection to the app is up right now.
//
// The relay's own words are the helper's: `{"type":"host","app":
// "not_running"|"connected"}` and `{"type":"host","error":"malformed"}`.
// Like the helper it keeps no state on disk, reads no file but the socket,
// opens no network connection, never starts the app and knows nothing
// about what the messages mean.

import Foundation

public struct RelayConfig {
    /// Where the app listens. Asked again at every try.
    public var socketPath: () -> String?
    public var policy: PeerPolicy
    /// What the app is told this connection is for.
    public var origin: String
    /// Between two tries to reach the app: this long at first, growing.
    public var retryFirst: TimeInterval = 1
    public var retryMax: TimeInterval = 10
    /// The longest an answer is held.
    public var maxWait: TimeInterval = 25
    /// A session nobody asked about for this long is closed.
    public var idleLimit: TimeInterval = 90
    public var maxSessions = 8
    /// Messages from the app waiting for the extension. More than this and
    /// the session ends: the extension is not listening.
    public var maxOutbox = 256

    public init(socketPath: @escaping () -> String?, policy: PeerPolicy = .currentUser, origin: String) {
        self.socketPath = socketPath
        self.policy = policy
        self.origin = origin
    }
}

/// The protocol version in `host.open` and in every exchange.
public let relayVersion = 1

final class Session {
    let id: String
    private let config: RelayConfig
    private let cond = NSCondition()

    // Everything below is guarded by `cond`.
    private var outbox: [(seq: Int, message: [String: Any], size: Int)] = []
    private var nextSeq = 1
    private var socket: AppSocket?
    /// Grows at every connection, so the reader of an old one changes nothing.
    private var generation = 0
    private var connecting = false
    private var failures = 0
    private var nextTry = Date.distantPast
    private var toldDown = false
    private var closed = false
    private(set) var lastSeen = Date()
    private(set) var inFlight = 0

    init(id: String, config: RelayConfig) {
        self.id = id
        self.config = config
    }

    var idleSince: Date? {
        cond.lock()
        defer { cond.unlock() }
        return inFlight == 0 ? lastSeen : nil
    }

    private func enqueue(_ message: [String: Any]) {
        let size = (try? JSONSerialization.data(withJSONObject: message, options: []))?.count ?? 0
        outbox.append((nextSeq, message, size))
        nextSeq += 1
        if outbox.count > config.maxOutbox {
            closeLocked()
        }
    }

    private func closeLocked() {
        closed = true
        socket?.close()
        socket = nil
        generation += 1
        cond.broadcast()
    }

    func close() {
        cond.lock()
        closeLocked()
        cond.unlock()
    }

    private func retryDelay(_ failures: Int) -> TimeInterval {
        min(config.retryFirst * Double(1 + failures / 5), config.retryMax)
    }

    /// Reach the app when it is time to try. Never blocks on another try.
    /// True when this call told the extension that the app is away.
    @discardableResult
    private func connectIfDue() -> Bool {
        cond.lock()
        if closed || socket != nil || connecting || Date() < nextTry {
            cond.unlock()
            return false
        }
        connecting = true
        cond.unlock()

        var opened: AppSocket?
        if let path = config.socketPath(), let candidate = try? AppSocket.connect(path: path, policy: config.policy) {
            let open: [String: Any] = ["type": "host.open", "v": relayVersion, "origin": config.origin]
            if let line = Frame.line(open, max: 4096), (try? candidate.write(line)) != nil {
                opened = candidate
            } else {
                candidate.close()
            }
        }

        var told = false
        cond.lock()
        connecting = false
        if closed {
            opened?.close()
        } else if let opened {
            socket = opened
            generation += 1
            failures = 0
            toldDown = false
            enqueue(Frame.hostApp("connected"))
            startReader(opened, generation)
        } else {
            failures += 1
            nextTry = Date().addingTimeInterval(retryDelay(failures))
            if !toldDown {
                toldDown = true
                told = true
                enqueue(Frame.hostApp("not_running"))
            }
        }
        cond.broadcast()
        cond.unlock()
        return told
    }

    private func startReader(_ socket: AppSocket, _ generation: Int) {
        let thread = Thread { [weak self] in
            while true {
                guard let line = try? socket.readLine(max: Limits.lineFromApp) else {
                    break
                }
                // Not a message, or too big for the extension: dropped, the
                // connection stays.
                guard line.count <= Limits.toBrowser, let message = Frame.parse(line: line) else {
                    continue
                }
                guard let self else {
                    break
                }
                self.cond.lock()
                if self.generation == generation {
                    self.enqueue(message)
                    self.cond.broadcast()
                }
                self.cond.unlock()
            }
            socket.close()
            self?.lost(socket, generation)
        }
        thread.name = "connect-relay-reader"
        thread.stackSize = 256 * 1024
        thread.start()
    }

    /// The connection ended: say so once and try again soon.
    private func lost(_ socket: AppSocket, _ generation: Int) {
        cond.lock()
        if self.generation == generation && self.socket === socket {
            self.socket = nil
            toldDown = true
            failures = 0
            nextTry = Date().addingTimeInterval(config.retryFirst)
            enqueue(Frame.hostApp("not_running"))
            cond.broadcast()
        }
        cond.unlock()
    }

    private func drop(_ socket: AppSocket) {
        // The reader notices and reports the loss.
        socket.close()
    }

    /// One exchange: take the extension's acknowledgement and messages,
    /// answer with what the app (or the relay) has to say.
    func exchange(ack: Int, send: [Any], wait: TimeInterval) -> [String: Any] {
        cond.lock()
        inFlight += 1
        lastSeen = Date()
        outbox.removeAll { $0.seq <= ack }
        cond.unlock()
        defer {
            cond.lock()
            inFlight -= 1
            lastSeen = Date()
            cond.unlock()
        }

        // Said at most once in one answer, however many messages came.
        var saidDown = connectIfDue()
        for item in send {
            guard let message = Frame.checked(item), let line = Frame.line(message, max: Limits.fromBrowser) else {
                cond.lock()
                enqueue(Frame.hostError("malformed"))
                cond.unlock()
                continue
            }
            cond.lock()
            let current = socket
            if current == nil && !saidDown {
                // Whatever it asked while the app is away, the answer is
                // the same, and nothing is kept to be sent later.
                saidDown = true
                enqueue(Frame.hostApp("not_running"))
            }
            cond.unlock()
            if let current, (try? current.write(line)) == nil {
                drop(current)
            }
        }

        let deadline = Date().addingTimeInterval(wait)
        cond.lock()
        while outbox.isEmpty && !closed {
            let now = Date()
            if now >= deadline {
                break
            }
            if socket == nil && !connecting && nextTry <= now {
                cond.unlock()
                connectIfDue()
                cond.lock()
                continue
            }
            let wake = (socket == nil && !connecting) ? min(deadline, nextTry) : deadline
            cond.wait(until: wake)
        }
        var messages: [[String: Any]] = []
        var total = 0
        for entry in outbox {
            // Always at least one; then as many as fit one answer.
            if !messages.isEmpty && total + entry.size > Limits.toBrowser {
                break
            }
            messages.append(["seq": entry.seq, "msg": entry.message])
            total += entry.size
        }
        let isClosed = closed
        let isUp = socket != nil
        cond.unlock()

        var reply: [String: Any] = ["type": "relay", "v": relayVersion, "app": isUp, "messages": messages]
        if isClosed {
            reply["closed"] = true
        }
        return reply
    }
}

public final class RelayHub {
    private let config: RelayConfig
    private let lock = NSLock()
    private var sessions: [String: Session] = [:]
    private var reaper: DispatchSourceTimer?

    public init(config: RelayConfig) {
        self.config = config
    }

    static func error(_ code: String) -> [String: Any] {
        ["type": "relay", "v": relayVersion, "error": code]
    }

    private static func isSessionId(_ value: String) -> Bool {
        (16...64).contains(value.utf8.count) && value.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
    }

    /// A whole number a JSON message can carry, and not a boolean.
    private static func integer(_ value: Any?) -> Int? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
            return nil
        }
        let double = number.doubleValue
        guard double.isFinite, double == double.rounded(), abs(double) < 9_007_199_254_740_992 else {
            return nil
        }
        return Int(double)
    }

    /// The answer to one native message from the extension. Blocks for at
    /// most `wait` (bounded by the config); call it off the main thread.
    public func reply(to request: Any?) -> [String: Any] {
        guard let object = request as? [String: Any], let type = object["type"] as? String else {
            return RelayHub.error("bad_request")
        }
        if type == "relay.ping" {
            return ["type": "relay.pong", "v": relayVersion]
        }
        guard type == "relay" else {
            return RelayHub.error("bad_request")
        }
        guard RelayHub.integer(object["v"]) == relayVersion else {
            return RelayHub.error("unsupported_version")
        }
        guard let sid = object["sid"] as? String, RelayHub.isSessionId(sid) else {
            return RelayHub.error("bad_request")
        }
        if (object["close"] as? Bool) == true {
            lock.lock()
            let gone = sessions.removeValue(forKey: sid)
            lock.unlock()
            gone?.close()
            return ["type": "relay", "v": relayVersion, "messages": [], "closed": true]
        }
        let ack = object["ack"] == nil ? 0 : (RelayHub.integer(object["ack"]) ?? -1)
        let send = object["send"] == nil ? [] : (object["send"] as? [Any])
        let waitMs = object["wait"] == nil ? 0 : (RelayHub.integer(object["wait"]) ?? -1)
        guard ack >= 0, let send, send.count <= Limits.sendBatch, waitMs >= 0 else {
            return RelayHub.error("bad_request")
        }
        guard let session = session(for: sid) else {
            return RelayHub.error("busy")
        }
        let wait = min(Double(waitMs) / 1000, config.maxWait)
        let reply = session.exchange(ack: ack, send: send, wait: wait)
        if reply["closed"] != nil {
            // Ended (the extension stopped taking what the app says): the
            // extension starts over under a new id.
            lock.lock()
            if sessions[sid] === session {
                sessions.removeValue(forKey: sid)
            }
            lock.unlock()
        }
        return reply
    }

    private func session(for sid: String) -> Session? {
        lock.lock()
        defer { lock.unlock() }
        if let have = sessions[sid] {
            return have
        }
        reapLocked()
        if sessions.count >= config.maxSessions {
            // Make room by closing the one unused for the longest; never
            // one that is being served right now.
            let idle = sessions.values.compactMap { s in s.idleSince.map { (s, $0) } }.min { $0.1 < $1.1 }
            guard let (oldest, _) = idle else {
                return nil
            }
            sessions.removeValue(forKey: oldest.id)
            oldest.close()
        }
        let made = Session(id: sid, config: config)
        sessions[sid] = made
        startReaperLocked()
        return made
    }

    /// Close sessions whose extension stopped asking (its background
    /// script is gone): the app then sees that browser as away.
    private func reapLocked() {
        let limit = Date().addingTimeInterval(-config.idleLimit)
        for (sid, session) in sessions {
            if let since = session.idleSince, since < limit {
                sessions.removeValue(forKey: sid)
                session.close()
            }
        }
        if sessions.isEmpty {
            reaper?.cancel()
            reaper = nil
        }
    }

    private func startReaperLocked() {
        guard reaper == nil else {
            return
        }
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
        let every = max(config.idleLimit / 4, 0.05)
        timer.schedule(deadline: .now() + every, repeating: every)
        timer.setEventHandler { [weak self] in
            guard let self else {
                return
            }
            self.lock.lock()
            self.reapLocked()
            self.lock.unlock()
        }
        reaper = timer
        timer.resume()
    }

    /// How many sessions are open (tests).
    public var sessionCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return sessions.count
    }

    /// Close everything.
    public func shutdown() {
        lock.lock()
        let all = sessions
        sessions.removeAll()
        reaper?.cancel()
        reaper = nil
        lock.unlock()
        for session in all.values {
            session.close()
        }
    }
}
