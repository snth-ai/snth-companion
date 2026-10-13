// The relay's end of the socket the running app listens on.
//
// The app binds a Unix socket in a folder of the app group both programs
// belong to (mode 0700, the socket 0600). Before a byte is written the
// relay checks that what it found is the app of the SAME user: the folder
// and the socket belong to that user, the folder is closed to everybody
// else, it is a real folder and a real socket (not a link put in their
// place), and the process on the other end runs as that user. The app
// checks the same about the relay (crates/connect-host/src/transport.rs).

import Foundation

/// Who the other end must be.
public struct PeerPolicy {
    public var uid: uid_t

    public init(uid: uid_t) {
        self.uid = uid
    }

    /// The user this process runs as. The only policy the extension uses.
    public static var currentUser: PeerPolicy {
        PeerPolicy(uid: getuid())
    }
}

public enum SocketError: Error, Equatable {
    /// Nothing listens there: the app is not running.
    case notThere
    /// Something is there and it is not the app of this user.
    case notOurs(String)
    case pathTooLong
    /// A line longer than the limit: the stream cannot be read further.
    case lineTooLong
    case io(Int32)
}

public final class AppSocket {
    private let fd: Int32
    private let writeLock = NSLock()
    private let stateLock = NSLock()
    private var shut = false
    private var pending = Data()

    private init(fd: Int32) {
        self.fd = fd
    }

    deinit {
        Darwin.close(fd)
    }

    private static func status(_ path: String) -> stat? {
        var st = stat()
        return lstat(path, &st) == 0 ? st : nil
    }

    /// Connect to the app's socket at `path`, or say why not.
    public static func connect(path: String, policy: PeerPolicy) throws -> AppSocket {
        let folder = (path as NSString).deletingLastPathComponent
        guard !folder.isEmpty, let dir = status(folder) else {
            throw SocketError.notThere
        }
        guard dir.st_mode & S_IFMT == S_IFDIR else {
            throw SocketError.notOurs("the app's folder is not a folder")
        }
        guard dir.st_uid == policy.uid else {
            throw SocketError.notOurs("the app's folder belongs to another user")
        }
        guard dir.st_mode & 0o077 == 0 else {
            throw SocketError.notOurs("the app's folder is open to other users")
        }
        guard let node = status(path) else {
            throw SocketError.notThere
        }
        guard node.st_mode & S_IFMT == S_IFSOCK else {
            throw SocketError.notOurs("the app's endpoint is not a socket")
        }
        guard node.st_uid == policy.uid else {
            throw SocketError.notOurs("the app's endpoint belongs to another user")
        }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let room = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < room else {
            throw SocketError.pathTooLong
        }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
        }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw SocketError.io(errno)
        }
        var on: Int32 = 1
        // A write to an app that went away is an error here, not a signal
        // that ends the extension's process.
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        // An app that stopped reading must not hold a request forever.
        var patience = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &patience, socklen_t(MemoryLayout<timeval>.size))

        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            let code = errno
            Darwin.close(fd)
            // A socket file with nobody behind it is an app that is gone.
            if code == ECONNREFUSED || code == ENOENT {
                throw SocketError.notThere
            }
            throw SocketError.io(code)
        }

        var peer: uid_t = 0
        var group: gid_t = 0
        guard getpeereid(fd, &peer, &group) == 0, peer == policy.uid else {
            Darwin.close(fd)
            throw SocketError.notOurs("the app's endpoint is served by another user")
        }
        return AppSocket(fd: fd)
    }

    /// Write all of `data`. One writer at a time, so two messages never
    /// interleave.
    public func write(_ data: Data) throws {
        writeLock.lock()
        defer { writeLock.unlock() }
        try data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            var offset = 0
            while offset < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + offset, raw.count - offset)
                if n > 0 {
                    offset += n
                } else if n < 0 && errno == EINTR {
                    continue
                } else {
                    throw SocketError.io(n < 0 ? errno : EPIPE)
                }
            }
        }
    }

    /// The next line, without its terminator. Nil when the app closed the
    /// connection. Blocks; call it from one thread only.
    public func readLine(max: Int) throws -> Data? {
        while true {
            if let end = pending.firstIndex(of: 0x0A) {
                let line = pending.subdata(in: pending.startIndex..<end)
                pending.removeSubrange(pending.startIndex...end)
                guard line.count <= max else {
                    throw SocketError.lineTooLong
                }
                return line
            }
            guard pending.count <= max else {
                throw SocketError.lineTooLong
            }
            var chunk = [UInt8](repeating: 0, count: 16 * 1024)
            let n = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
            if n > 0 {
                pending.append(contentsOf: chunk[0..<n])
            } else if n == 0 {
                return nil
            } else if errno != EINTR {
                throw SocketError.io(errno)
            }
        }
    }

    /// End the connection. A reader blocked in `readLine` wakes up.
    public func close() {
        stateLock.lock()
        let first = !shut
        shut = true
        stateLock.unlock()
        if first {
            shutdown(fd, SHUT_RDWR)
        }
    }
}
