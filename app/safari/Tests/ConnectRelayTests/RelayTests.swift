import XCTest

@testable import ConnectRelay

/// A value as Safari hands it over: whatever JSON can carry, bridged.
private func bridged(_ value: Any) -> Any {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])
    return try! JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
}

private let sidA = "0123456789abcdef0123456789abcdef"
private let sidB = "fedcba9876543210fedcba9876543210"
private let origin = "safari-web-extension://ai.snth.connect.safari/"

final class RelayTests: XCTestCase {
    private func hub(_ app: FakeApp, uid: uid_t = getuid(), tune: (inout RelayConfig) -> Void = { _ in }) -> RelayHub {
        var config = RelayConfig(socketPath: { app.path }, policy: PeerPolicy(uid: uid), origin: origin)
        config.retryFirst = 0.05
        config.retryMax = 0.2
        tune(&config)
        return RelayHub(config: config)
    }

    private func exchange(_ hub: RelayHub, sid: String = sidA, ack: Int = 0, send: [Any] = [], wait: Int = 0) -> [String: Any] {
        hub.reply(to: bridged(["type": "relay", "v": 1, "sid": sid, "ack": ack, "send": send, "wait": wait]))
    }

    private func messages(_ reply: [String: Any]) -> [(seq: Int, msg: [String: Any])] {
        (reply["messages"] as? [[String: Any]] ?? []).map { ($0["seq"] as! Int, $0["msg"] as! [String: Any]) }
    }

    private func words(_ reply: [String: Any]) -> [String] {
        messages(reply).map { entry in
            let m = entry.msg
            if m["type"] as? String == "host" {
                return "host:" + ((m["app"] as? String) ?? (m["error"] as? String) ?? "?")
            }
            return m["type"] as? String ?? "?"
        }
    }

    // --- the shape of a message ---

    func testOnlyATypedObjectIsAMessage() {
        XCTAssertNotNil(Frame.checked(bridged(["type": "hello", "v": 1])))
        for bad: Any in [[1, 2], "hello", 42, [String: Any](), ["type": 7], ["type": ""], ["type": true], ["type": String(repeating: "a", count: 65)]] {
            XCTAssertNil(Frame.checked(bridged(bad)), "\(bad)")
        }
        XCTAssertNil(Frame.checked(nil))
    }

    func testTheRelaysOwnTypesCannotBeSentByAnyoneElse() {
        for type in ["host", "host.open", "host.anything"] {
            XCTAssertNil(Frame.checked(bridged(["type": type])), type)
            XCTAssertNil(Frame.parse(line: try! JSONSerialization.data(withJSONObject: ["type": type])), type)
        }
        // A type that merely starts with the same letters is fine.
        XCTAssertNotNil(Frame.checked(bridged(["type": "hostile"])))
    }

    func testOneMessageIsOneLineAndSizesAreBounded() {
        let line = Frame.line(["type": "x", "text": "a\nb\r\nc"], max: 1024)!
        XCTAssertEqual(line.filter { $0 == 0x0A }.count, 1)
        XCTAssertEqual(line.last, 0x0A)
        XCTAssertNil(Frame.line(["type": "x", "pad": String(repeating: "a", count: 200)], max: 100))
        XCTAssertNil(Frame.parse(line: Data("not json".utf8)))
        XCTAssertNil(Frame.parse(line: Data("[1,2]".utf8)))
    }

    // --- the relay ---

    func testTheFirstLineNamesTheExtensionAndMessagesPassBothWays() {
        let app = FakeApp("both")
        let hub = hub(app)
        defer { hub.shutdown() }

        let first = exchange(hub, send: [["type": "hello", "v": 1, "install": "ab"]])
        let conn = app.accept()!
        let open = conn.line()!
        XCTAssertEqual(open["type"] as? String, "host.open")
        XCTAssertEqual(open["v"] as? Int, 1)
        XCTAssertEqual(open["origin"] as? String, origin)
        let hello = conn.line()!
        XCTAssertEqual(hello["type"] as? String, "hello")
        XCTAssertEqual(hello["install"] as? String, "ab")
        // The extension is told the app is there, as the Chromium helper says it.
        XCTAssertEqual(words(first), ["host:connected"])
        XCTAssertEqual(first["app"] as? Bool, true)

        conn.send(["type": "hello_ack", "v": 1, "linked": true, "synth": "Mia"])
        let second = exchange(hub, ack: 1, wait: 2000)
        XCTAssertEqual(words(second), ["hello_ack"])
        XCTAssertEqual(messages(second)[0].seq, 2)
        XCTAssertEqual(messages(second)[0].msg["synth"] as? String, "Mia")
        XCTAssertEqual(messages(second)[0].msg["linked"] as? Bool, true)
    }

    func testWhatTheAppSaysIsKeptUntilTheExtensionAcknowledgesIt() {
        let app = FakeApp("ack")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()
        conn.send(["type": "pair.code", "id": 1, "code": "K7QW3MHX"])
        conn.send(["type": "status", "linked": true])

        // Safari lost this answer: the next one carries the same messages,
        // under the same numbers, in the same order.
        let lost = exchange(hub, wait: 2000)
        var again = exchange(hub, wait: 0)
        for _ in 0..<50 where messages(again).count < 3 {
            usleep(20_000)
            again = exchange(hub, wait: 0)
        }
        XCTAssertEqual(words(again), ["host:connected", "pair.code", "status"])
        XCTAssertEqual(messages(again).map(\.seq), [1, 2, 3])
        XCTAssertEqual(Array(words(again).prefix(words(lost).count)), words(lost))

        // A background script that Safari stopped and started again comes
        // back with nothing acknowledged: it is handed what is still waiting.
        XCTAssertEqual(messages(exchange(hub, ack: 0)).map(\.seq), [1, 2, 3])

        // Acknowledged up to 2: only the third is left, and then nothing.
        XCTAssertEqual(messages(exchange(hub, ack: 2)).map(\.seq), [3])
        XCTAssertTrue(messages(exchange(hub, ack: 3)).isEmpty)
        // After that a restarted script gets no message, only the fact
        // that the app is there.
        let resumed = exchange(hub, ack: 0)
        XCTAssertTrue(messages(resumed).isEmpty)
        XCTAssertEqual(resumed["app"] as? Bool, true)
    }

    func testAHeldAnswerReturnsAsSoonAsTheAppSpeaks() {
        let app = FakeApp("hold")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()

        // Nothing to say: the answer is held for the time asked, not longer.
        let started = Date()
        XCTAssertTrue(messages(exchange(hub, ack: 1, wait: 300)).isEmpty)
        let held = Date().timeIntervalSince(started)
        XCTAssertTrue(held >= 0.25 && held < 2, "\(held)")

        DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) {
            conn.send(["type": "cookies.get", "id": 9, "site": "example.com"])
        }
        let before = Date()
        let reply = exchange(hub, ack: 1, wait: 5000)
        XCTAssertEqual(words(reply), ["cookies.get"])
        XCTAssertLessThan(Date().timeIntervalSince(before), 3)
    }

    func testTheAnswerIsNeverHeldLongerThanTheRelayAllows() {
        let app = FakeApp("cap")
        let hub = hub(app) { $0.maxWait = 0.2 }
        defer { hub.shutdown() }
        _ = exchange(hub)
        let started = Date()
        _ = exchange(hub, ack: 1, wait: 600_000)
        XCTAssertLessThan(Date().timeIntervalSince(started), 2)
    }

    func testWhileTheAppIsAwayTheExtensionIsToldSoAndNothingIsKeptForLater() {
        let app = FakeApp("away", listen: false)
        let hub = hub(app)
        defer { hub.shutdown() }

        // Told at once, and again in answer to anything it sends.
        let told = exchange(hub)
        XCTAssertEqual(words(told), ["host:not_running"])
        XCTAssertEqual(told["app"] as? Bool, false)
        XCTAssertEqual(words(exchange(hub, ack: 1, send: [["type": "pair.request", "id": 1]])), ["host:not_running"])
        // Not repeated while it only waits.
        XCTAssertTrue(messages(exchange(hub, ack: 2, wait: 200)).isEmpty)

        // The app starts: the relay finds it by itself while an answer is held.
        app.start()
        let reply = exchange(hub, ack: 2, wait: 3000)
        XCTAssertEqual(words(reply), ["host:connected"])
        let conn = app.accept()!
        XCTAssertEqual(conn.line()?["type"] as? String, "host.open")
        // The pairing request sent while it was away was not replayed.
        conn.send(["type": "status", "linked": false])
        XCTAssertEqual(words(exchange(hub, ack: 3, wait: 2000)), ["status"])
    }

    func testAnAppThatRestartsIsFoundAgainAndToldWhoIsCalling() {
        let app = FakeApp("restart")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub)
        var conn = app.accept()
        XCTAssertEqual(conn?.line()?["type"] as? String, "host.open")

        // The app quits (an update): its socket goes with it.
        conn?.hangUp()
        conn = nil
        app.stop()
        XCTAssertEqual(words(exchange(hub, ack: 1, wait: 3000)), ["host:not_running"])

        app.start()
        XCTAssertEqual(words(exchange(hub, ack: 2, wait: 3000)), ["host:connected"])
        let again = app.accept()!
        XCTAssertEqual(again.line()?["type"] as? String, "host.open")
    }

    func testMalformedAndReservedMessagesNeverReachTheApp() {
        let app = FakeApp("bad")
        let hub = hub(app)
        defer { hub.shutdown() }
        let reply = exchange(hub, send: [
            ["no": "type"],
            "text",
            ["type": "host.open", "v": 1, "origin": "chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa/"],
            ["type": "host", "app": "connected"],
            ["type": "state", "paired": false],
        ])
        XCTAssertEqual(words(reply), ["host:connected", "host:malformed", "host:malformed", "host:malformed", "host:malformed"])
        let conn = app.accept()!
        XCTAssertEqual(conn.line()?["type"] as? String, "host.open")
        // The one well-formed message is the only other line the app sees.
        XCTAssertEqual(conn.line()?["type"] as? String, "state")
        conn.send(["type": "status", "linked": true])
        XCTAssertEqual(words(exchange(hub, ack: 5, wait: 2000)), ["status"])
    }

    func testAMessageOverTheLimitIsRefusedHere() {
        let app = FakeApp("big")
        let hub = hub(app)
        defer { hub.shutdown() }
        let huge: [String: Any] = ["type": "cookies.result", "pad": String(repeating: "a", count: Limits.fromBrowser)]
        XCTAssertEqual(words(exchange(hub, send: [huge, ["type": "state"]])), ["host:connected", "host:malformed"])
        let conn = app.accept()!
        XCTAssertEqual(conn.line()?["type"] as? String, "host.open")
        XCTAssertEqual(conn.line()?["type"] as? String, "state")
    }

    func testWhatTheAppSaysThatIsNotAMessageIsDroppedAndTheLinkStays() {
        let app = FakeApp("junk")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()
        conn.sendRaw(Data("not json\n".utf8))
        conn.sendRaw(Data("[1,2,3]\n".utf8))
        conn.send(["type": "host", "app": "connected"])
        // Too big for the extension (the browsers' limit), small enough to skip.
        conn.send(["type": "status", "pad": String(repeating: "a", count: Limits.toBrowser)])
        conn.send(["type": "status", "linked": true])
        let reply = exchange(hub, ack: 1, wait: 3000)
        XCTAssertEqual(words(reply), ["status"])
        XCTAssertEqual(messages(reply)[0].msg["linked"] as? Bool, true)
        XCTAssertNil(messages(reply)[0].msg["pad"])
    }

    func testALineTheRelayCannotReadPastEndsTheConnection() {
        let app = FakeApp("long")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()
        conn.sendRaw(Data(repeating: 0x61, count: Limits.lineFromApp + 4096))
        XCTAssertEqual(words(exchange(hub, ack: 1, wait: 3000)), ["host:not_running"])
        XCTAssertTrue(conn.closedByPeer())
    }

    func testAnAppOfAnotherUserIsRefusedBeforeAByteIsWritten() {
        let app = FakeApp("uid")
        // The extension expects the app to run as its own user. Here the
        // relay is told to expect somebody else, so the real socket (owned
        // and served by this user) is "another user's" to it.
        let hub = hub(app, uid: getuid() + 1)
        defer { hub.shutdown() }
        XCTAssertEqual(words(exchange(hub, send: [["type": "hello", "v": 1]])), ["host:not_running"])
        XCTAssertFalse(app.hasCaller(within: 0.3), "the relay must not even connect")
        XCTAssertThrowsError(try AppSocket.connect(path: app.path, policy: PeerPolicy(uid: getuid() + 1))) { error in
            XCTAssertEqual(error as? SocketError, .notOurs("the app's folder belongs to another user"))
        }
        XCTAssertNoThrow(try AppSocket.connect(path: app.path, policy: .currentUser).close())
    }

    func testWhatIsNotTheAppsOwnSocketIsRefused() {
        // The folder is open to other users.
        let open = FakeApp("perm")
        chmod(open.folder, 0o755)
        XCTAssertThrowsError(try AppSocket.connect(path: open.path, policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notOurs("the app's folder is open to other users"))
        }
        chmod(open.folder, 0o700)

        // A file in place of the socket.
        let file = FakeApp("file", listen: false)
        FileManager.default.createFile(atPath: file.path, contents: Data())
        XCTAssertThrowsError(try AppSocket.connect(path: file.path, policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notOurs("the app's endpoint is not a socket"))
        }

        // A link in place of the folder (pointing at a real app's folder).
        let real = FakeApp("real")
        let base = "/tmp/crt-\(getpid())-link"
        try? FileManager.default.removeItem(atPath: base)
        try! FileManager.default.createDirectory(atPath: base, withIntermediateDirectories: true)
        try! FileManager.default.createSymbolicLink(atPath: base + "/run", withDestinationPath: real.folder)
        XCTAssertThrowsError(try AppSocket.connect(path: base + "/run/app.sock", policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notOurs("the app's folder is not a folder"))
        }
        try? FileManager.default.removeItem(atPath: base)

        // A link in place of the socket.
        let linked = FakeApp("slink", listen: false)
        try! FileManager.default.createSymbolicLink(atPath: linked.path, withDestinationPath: real.path)
        XCTAssertThrowsError(try AppSocket.connect(path: linked.path, policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notOurs("the app's endpoint is not a socket"))
        }

        // Nothing there, or a socket file nobody serves: the app is not running.
        let none = FakeApp("none", listen: false)
        XCTAssertThrowsError(try AppSocket.connect(path: none.path, policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notThere)
        }
        XCTAssertThrowsError(try AppSocket.connect(path: "/tmp/crt-\(getpid())-nowhere/run/app.sock", policy: .currentUser)) { error in
            XCTAssertEqual(error as? SocketError, .notThere)
        }
        XCTAssertThrowsError(try AppSocket.connect(path: "/tmp/" + String(repeating: "x", count: 200) + "/app.sock", policy: .currentUser))
    }

    func testARequestThatIsNotAnExchangeIsRefused() {
        let app = FakeApp("req")
        let hub = hub(app)
        defer { hub.shutdown() }
        func code(_ request: Any?) -> String? {
            hub.reply(to: request.map(bridged))["error"] as? String
        }
        XCTAssertEqual(code(nil), "bad_request")
        XCTAssertEqual(code("relay"), "bad_request")
        XCTAssertEqual(code(["type": "hello", "v": 1]), "bad_request")
        XCTAssertEqual(code(["type": "relay", "v": 2, "sid": sidA]), "unsupported_version")
        XCTAssertEqual(code(["type": "relay", "v": true, "sid": sidA]), "unsupported_version")
        for sid: Any in ["short", "0123456789ABCDEF0123", "0123456789abcdeg0123", 12345678901234567, String(repeating: "a", count: 65)] {
            XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sid]), "bad_request", "\(sid)")
        }
        XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sidA, "ack": -1]), "bad_request")
        XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sidA, "ack": 1.5]), "bad_request")
        XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sidA, "send": ["type": "x"]]), "bad_request")
        XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sidA, "wait": "long"]), "bad_request")
        let many = Array(repeating: ["type": "x"], count: Limits.sendBatch + 1)
        XCTAssertEqual(code(["type": "relay", "v": 1, "sid": sidA, "send": many]), "bad_request")
        // None of that opened a session or reached the app.
        XCTAssertEqual(hub.sessionCount, 0)
        XCTAssertFalse(app.hasCaller(within: 0.1))

        XCTAssertEqual(hub.reply(to: bridged(["type": "relay.ping"]))["type"] as? String, "relay.pong")
        XCTAssertEqual(hub.sessionCount, 0)
    }

    func testEachLifeOfTheExtensionIsItsOwnConnectionAndClosingEndsIt() {
        let app = FakeApp("two")
        let hub = hub(app)
        defer { hub.shutdown() }
        _ = exchange(hub, sid: sidA, send: [["type": "hello", "install": "a"]])
        let first = app.accept()!
        _ = first.line()
        XCTAssertEqual(first.line()?["install"] as? String, "a")
        _ = exchange(hub, sid: sidB, send: [["type": "hello", "install": "b"]])
        let second = app.accept()!
        _ = second.line()
        XCTAssertEqual(second.line()?["install"] as? String, "b")
        XCTAssertEqual(hub.sessionCount, 2)

        // What the app says to one is not heard by the other.
        first.send(["type": "pair.code", "id": 1, "code": "K7QW3MHX"])
        XCTAssertEqual(words(exchange(hub, sid: sidA, ack: 1, wait: 2000)), ["pair.code"])
        XCTAssertTrue(messages(exchange(hub, sid: sidB, ack: 1, wait: 100)).isEmpty)

        let closed = hub.reply(to: bridged(["type": "relay", "v": 1, "sid": sidA, "close": true]))
        XCTAssertEqual(closed["closed"] as? Bool, true)
        XCTAssertTrue(first.closedByPeer())
        XCTAssertEqual(hub.sessionCount, 1)
    }

    func testASessionNobodyAsksAboutIsClosedAndTheAppSeesTheBrowserLeave() {
        let app = FakeApp("idle")
        let hub = hub(app) { $0.idleLimit = 0.2 }
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()
        // The extension's background script is gone: nobody asks any more.
        XCTAssertTrue(conn.closedByPeer())
        XCTAssertEqual(hub.sessionCount, 0)
    }

    func testAtMostSoManySessionsAndTheOldestMakesRoom() {
        let app = FakeApp("cap2")
        let hub = hub(app) { $0.maxSessions = 2 }
        defer { hub.shutdown() }
        let sids = ["a", "b", "c"].map { String(repeating: $0, count: 32) }
        _ = exchange(hub, sid: sids[0])
        let first = app.accept()!
        _ = first.line()
        usleep(20_000)
        _ = exchange(hub, sid: sids[1])
        _ = app.accept()
        _ = exchange(hub, sid: sids[2])
        XCTAssertEqual(hub.sessionCount, 2)
        XCTAssertTrue(first.closedByPeer(), "the session unused for the longest was closed")
    }

    func testAnExtensionThatStopsListeningIsCutOff() {
        let app = FakeApp("flood")
        let hub = hub(app) { $0.maxOutbox = 4 }
        defer { hub.shutdown() }
        _ = exchange(hub)
        let conn = app.accept()!
        _ = conn.line()
        for i in 0..<8 {
            conn.send(["type": "status", "n": i])
        }
        XCTAssertTrue(conn.closedByPeer())
        let reply = exchange(hub, wait: 500)
        XCTAssertEqual(reply["closed"] as? Bool, true)
        XCTAssertEqual(hub.sessionCount, 0)
    }

    func testTheHandlersEntryPointAnswersUnderTheSameKey() {
        let app = FakeApp("entry")
        let hub = hub(app)
        defer { hub.shutdown() }
        let request = bridged(["type": "relay", "v": 1, "sid": sidA, "send": [["type": "hello", "v": 1]]])
        let answer = ConnectApp.answer(userInfo: ["message": request], key: "message", hub: hub)
        XCTAssertEqual(words(answer["message"] as! [String: Any]), ["host:connected"])
        XCTAssertEqual((ConnectApp.answer(userInfo: nil, key: "message", hub: hub)["message"] as? [String: Any])?["error"] as? String, "bad_request")
    }

    func testTheNamesTheExtensionIsBuiltOn() {
        // Team id, a dot, a name: the form macOS takes without a profile.
        XCTAssertEqual(ConnectApp.group, "CZAP38JFAE.ai.snth.connect")
        XCTAssertTrue(ConnectApp.extensionId.hasPrefix("ai.snth.connect."))
        XCTAssertEqual(ConnectApp.origin, "safari-web-extension://\(ConnectApp.extensionId)/")
    }
}
