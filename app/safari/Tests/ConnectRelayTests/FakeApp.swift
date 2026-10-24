// A stand-in for the app's end of the socket: binds a Unix socket in a
// folder of its own (mode 0700, the socket 0600, as the app does), takes
// connections and lets a test read and write lines.

import Foundation
import XCTest

final class FakeConnection {
    let fd: Int32
    private var pending = Data()

    init(fd: Int32) {
        self.fd = fd
        var patience = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &patience, socklen_t(MemoryLayout<timeval>.size))
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
    }

    deinit {
        close(fd)
    }

    /// The next line as JSON; nil when nothing came in 5 seconds or the
    /// other end closed.
    func line() -> [String: Any]? {
        while true {
            if let end = pending.firstIndex(of: 0x0A) {
                let line = pending.subdata(in: pending.startIndex..<end)
                pending.removeSubrange(pending.startIndex...end)
                return (try? JSONSerialization.jsonObject(with: line)) as? [String: Any]
            }
            var chunk = [UInt8](repeating: 0, count: 64 * 1024)
            let n = chunk.withUnsafeMutableBytes { read(fd, $0.baseAddress, $0.count) }
            guard n > 0 else {
                return nil
            }
            pending.append(contentsOf: chunk[0..<n])
        }
    }

    /// True when the other end closed (or nothing was written for a while
    /// and then it closed).
    func closedByPeer() -> Bool {
        var byte: UInt8 = 0
        return read(fd, &byte, 1) == 0
    }

    func send(_ message: [String: Any]) {
        var data = try! JSONSerialization.data(withJSONObject: message)
        data.append(0x0A)
        sendRaw(data)
    }

    func sendRaw(_ data: Data) {
        data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = write(fd, raw.baseAddress! + offset, raw.count - offset)
                if n <= 0 { break }
                offset += n
            }
        }
    }

    func hangUp() {
        shutdown(fd, SHUT_RDWR)
    }
}

final class FakeApp {
    let folder: String
    let path: String
    private var listener: Int32 = -1

    /// `tag` keeps the socket path short: a Unix socket path holds about
    /// 100 bytes.
    init(_ tag: String, listen: Bool = true) {
        let base = "/tmp/crt-\(getpid())-\(tag)"
        try? FileManager.default.removeItem(atPath: base)
        folder = base + "/run"
        path = folder + "/app.sock"
        try! FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        chmod(folder, 0o700)
        if listen {
            start()
        }
    }

    deinit {
        stop()
        try? FileManager.default.removeItem(atPath: (folder as NSString).deletingLastPathComponent)
    }

    func start() {
        unlink(path)
        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        XCTAssertEqual(bound, 0, "bind \(path): \(String(cString: strerror(errno)))")
        chmod(path, 0o600)
        listen(listener, 8)
        var patience = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(listener, SOL_SOCKET, SO_RCVTIMEO, &patience, socklen_t(MemoryLayout<timeval>.size))
    }

    /// The app goes away: the socket file with it.
    func stop() {
        if listener >= 0 {
            close(listener)
            listener = -1
        }
        unlink(path)
    }

    /// The next connection, or nil when none came in 5 seconds.
    func accept() -> FakeConnection? {
        let fd = Darwin.accept(listener, nil, nil)
        return fd >= 0 ? FakeConnection(fd: fd) : nil
    }

    /// Is a connection waiting right now?
    func hasCaller(within seconds: Double) -> Bool {
        var set = pollfd(fd: listener, events: Int16(POLLIN), revents: 0)
        return poll(&set, 1, Int32(seconds * 1000)) > 0
    }
}
