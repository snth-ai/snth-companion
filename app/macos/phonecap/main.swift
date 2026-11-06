// snth-phonecap: the screen of the iPhone on the USB cable, for SNTH Connect's
// phone_control (crates/connect-core/src/phone/eyes.rs).
//
// Started by the app's core when the owner switches the phone on, killed when
// it is switched off; it also exits when its stdin closes (the app is gone).
// It ships inside the app bundle (Contents/MacOS) and is signed with it, so
// the camera permission macOS asks for is the app's.
//
// Capture: the CoreMediaIO screen device QuickTime uses (allowed with
// kCMIOHardwarePropertyAllowScreenCaptureDevices), through AVFoundation as an
// external muxed device, ONLY a device whose model is "iOS Device". Never
// another one: a Continuity Camera is someone's own iPhone. When the cable is
// pulled or the phone restarts, the session is set up again every second.
//
// Protocol (never a network port): one JSON request per line on stdin,
//   {"id":1,"cmd":"status"}
//   {"id":2,"cmd":"jpeg","after_ms":0,"max_edge":0,"quality":0.85}
//   {"id":3,"cmd":"gray","max_edge":448}
// and for each, on stdout, one JSON header line and then exactly `len` bytes
// (a JPEG, or gray pixels, one byte each, rows top to bottom). Logs go to
// stderr.
import AVFoundation
import CoreImage
import CoreMediaIO
import Foundation

func log(_ s: String) {
    FileHandle.standardError.write(Data("\(s)\n".utf8))
}

func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

// SNTH_PHONECAP_FAKE=1: no camera, no capture; a made-up 828x1792 screen
// that changes every 50 ms (the protocol's self-test, `connectd selftest
// phone --fake-screen`). The app never sets it.
let fake = ProcessInfo.processInfo.environment["SNTH_PHONECAP_FAKE"] == "1"

// Let macOS expose iOS screens as capture devices.
var prop = CMIOObjectPropertyAddress(
    mSelector: CMIOObjectPropertySelector(kCMIOHardwarePropertyAllowScreenCaptureDevices),
    mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
    mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain))
var allow: UInt32 = 1
if !fake {
    CMIOObjectSetPropertyData(CMIOObjectID(kCMIOObjectSystemObject), &prop, 0, nil,
                              UInt32(MemoryLayout<UInt32>.size), &allow)
}

/// The newest frame and what is being captured.
final class Grabber: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    private let lock = NSLock()
    private var last: CVPixelBuffer?
    private var lastMs: Int64 = 0
    private var frames = 0
    private var device = ""
    private var problem = ""

    func captureOutput(_ output: AVCaptureOutput, didOutput sb: CMSampleBuffer, from c: AVCaptureConnection) {
        guard let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        put(pb)
    }

    func put(_ pb: CVPixelBuffer) {
        lock.lock()
        last = pb
        lastMs = nowMs()
        frames += 1
        lock.unlock()
    }

    func snapshot() -> (CVPixelBuffer?, Int64, Int, String, String) {
        lock.lock(); defer { lock.unlock() }
        return (last, lastMs, frames, device, problem)
    }

    func setDevice(_ name: String) {
        lock.lock(); device = name; problem = ""; lock.unlock()
    }

    /// No device (or it went away): no frame either.
    func clear(_ why: String) {
        lock.lock(); device = ""; last = nil; problem = why; lock.unlock()
    }
}

let grabber = Grabber()
let session = AVCaptureSession()
let ciContext = CIContext(options: [.cacheIntermediates: false])
let frameQueue = DispatchQueue(label: "phonecap.frames")
let out = FileHandle.standardOutput
let outLock = NSLock()

func accessWord() -> String {
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized: return "allowed"
    case .denied: return "denied"
    case .restricted: return "restricted"
    default: return "undecided"
    }
}

log("camera access: \(accessWord())")
if !fake && AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined {
    AVCaptureDevice.requestAccess(for: .video) { ok in log("camera access granted: \(ok)") }
}

func findDevice() -> AVCaptureDevice? {
    var types: [AVCaptureDevice.DeviceType] = []
    if #available(macOS 14.0, *) {
        types = [.external]
    } else {
        types = [.externalUnknown]
    }
    let s = AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: .muxed, position: .unspecified)
    // Never fall back: a Continuity Camera is someone's own phone.
    return s.devices.first { $0.modelID == "iOS Device" }
}

func startCapture() {
    guard let dev = findDevice() else {
        grabber.clear("no_device: no iPhone is connected by cable (plug it in, unlock it and tap Trust)")
        return
    }
    if dev.isInUseByAnotherApplication {
        grabber.clear("device_busy: the phone's screen is in use by another program (QuickTime or another capture); close it")
    }
    do {
        let input = try AVCaptureDeviceInput(device: dev)
        session.beginConfiguration()
        if session.canAddInput(input) { session.addInput(input) }
        let output = AVCaptureVideoDataOutput()
        output.alwaysDiscardsLateVideoFrames = true
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
        output.setSampleBufferDelegate(grabber, queue: frameQueue)
        if session.canAddOutput(output) { session.addOutput(output) }
        session.commitConfiguration()
        session.startRunning()
        if session.isRunning && !session.inputs.isEmpty {
            grabber.setDevice(dev.localizedName)
            log("capturing \(dev.localizedName) (\(dev.modelID))")
        }
    } catch {
        let denied = AVCaptureDevice.authorizationStatus(for: .video) == .denied
        grabber.clear(denied
            ? "camera_denied: SNTH Connect may not use the camera; allow it in System Settings, Privacy & Security, Camera"
            : "no_device: cannot open the phone's screen: \(error.localizedDescription)")
        log("input error: \(error)")
    }
}

// --- frames ---

/// The image scaled so its long edge is at most `maxEdge` (0 = as is), with
/// whole-pixel size.
func scaled(_ img: CIImage, maxEdge: Int) -> CIImage {
    let w = img.extent.width, h = img.extent.height
    let long = max(w, h)
    guard maxEdge > 0, long > CGFloat(maxEdge) else { return img }
    let s = CGFloat(maxEdge) / long
    let tw = (w * s).rounded(), th = (h * s).rounded()
    let f = CIFilter(name: "CILanczosScaleTransform")!
    f.setValue(img, forKey: kCIInputImageKey)
    f.setValue(th / h, forKey: kCIInputScaleKey)
    f.setValue((tw / w) / (th / h), forKey: kCIInputAspectRatioKey)
    let o = f.outputImage ?? img
    let e = o.extent
    return o.cropped(to: CGRect(x: e.origin.x, y: e.origin.y, width: min(tw, e.width.rounded(.down)), height: min(th, e.height.rounded(.down))))
}

/// Gray pixels, one byte each, rows top to bottom.
func grayBytes(_ img: CIImage, w: Int, h: Int) -> Data? {
    guard let cg = ciContext.createCGImage(img, from: img.extent) else { return nil }
    var data = Data(count: w * h)
    let ok: Bool = data.withUnsafeMutableBytes { raw in
        guard let ctx = CGContext(data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                                  space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return false }
        ctx.interpolationQuality = .medium
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
        return true
    }
    return ok ? data : nil
}

// --- the protocol ---

func reply(_ header: [String: Any], _ payload: Data = Data()) {
    var h = header
    h["len"] = payload.count
    guard var line = try? JSONSerialization.data(withJSONObject: h) else { return }
    line.append(0x0A)
    outLock.lock()
    out.write(line)
    if !payload.isEmpty { out.write(payload) }
    outLock.unlock()
}

func noFrameReason(_ problem: String) -> String {
    if accessWord() == "denied" {
        return "camera_denied: SNTH Connect may not use the camera; allow it in System Settings, Privacy & Security, Camera"
    }
    if !problem.isEmpty { return problem }
    return "no_frame: the phone sent no picture yet"
}

func handle(_ req: [String: Any]) {
    let id = req["id"] ?? NSNull()
    let cmd = req["cmd"] as? String ?? ""
    switch cmd {
    case "status":
        let (pb, ms, n, device, problem) = grabber.snapshot()
        var r: [String: Any] = ["id": id, "ok": true, "device": device, "frames": n, "last_frame_ms": ms,
                                "access": accessWord(), "width": 0, "height": 0]
        if let pb = pb {
            r["width"] = CVPixelBufferGetWidth(pb)
            r["height"] = CVPixelBufferGetHeight(pb)
        }
        if device.isEmpty || pb == nil { r["error"] = noFrameReason(problem) }
        reply(r)
    case "jpeg", "gray":
        let after = (req["after_ms"] as? NSNumber)?.int64Value ?? 0
        let deadline = Date().addingTimeInterval(after > 0 ? 1.5 : 0)
        var (pb, ms, _, _, problem) = grabber.snapshot()
        while (pb == nil || ms <= after) && Date() < deadline {
            usleep(10_000)
            (pb, ms, _, _, problem) = grabber.snapshot()
        }
        guard let buf = pb else {
            reply(["id": id, "ok": false, "error": noFrameReason(problem)])
            return
        }
        let nw = CVPixelBufferGetWidth(buf), nh = CVPixelBufferGetHeight(buf)
        let img = CIImage(cvPixelBuffer: buf)
        let base: [String: Any] = ["id": id, "ok": true, "native_width": nw, "native_height": nh, "frame_ms": ms]
        if cmd == "jpeg" {
            let maxEdge = (req["max_edge"] as? NSNumber)?.intValue ?? 0
            let q = min(max((req["quality"] as? NSNumber)?.doubleValue ?? 0.85, 0.1), 1.0)
            let small = scaled(img, maxEdge: maxEdge)
            guard let data = ciContext.jpegRepresentation(
                of: small, colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
                options: [CIImageRepresentationOption(rawValue: kCGImageDestinationLossyCompressionQuality as String): q]) else {
                reply(["id": id, "ok": false, "error": "encode_failed: cannot make a JPEG of the frame"])
                return
            }
            var r = base
            r["width"] = Int(small.extent.width)
            r["height"] = Int(small.extent.height)
            reply(r, data)
        } else {
            let maxEdge = max((req["max_edge"] as? NSNumber)?.intValue ?? 448, 16)
            let small = scaled(img, maxEdge: maxEdge)
            let w = Int(small.extent.width), h = Int(small.extent.height)
            guard w > 0, h > 0, let data = grayBytes(small, w: w, h: h) else {
                reply(["id": id, "ok": false, "error": "encode_failed: cannot make a gray frame"])
                return
            }
            var r = base
            r["width"] = w
            r["height"] = h
            reply(r, data)
        }
    default:
        reply(["id": id, "ok": false, "error": "bad_request: unknown cmd \(cmd)"])
    }
}

// Requests are read on their own thread and served one at a time.
let requests = DispatchQueue(label: "phonecap.requests")
Thread.detachNewThread {
    while let line = readLine(strippingNewline: true) {
        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            if !line.isEmpty { reply(["id": NSNull(), "ok": false, "error": "bad_request: not a JSON object"]) }
            continue
        }
        requests.sync { handle(obj) }
    }
    // stdin closed: the app is gone or switched the phone off.
    log("stdin closed, exiting")
    exit(0)
}

/// A made-up frame: bands whose brightness follows the tick.
func fakeFrame(_ tick: Int) -> CVPixelBuffer? {
    var pb: CVPixelBuffer?
    CVPixelBufferCreate(nil, 828, 1792, kCVPixelFormatType_32BGRA, nil, &pb)
    guard let buf = pb else { return nil }
    CVPixelBufferLockBaseAddress(buf, [])
    let row = CVPixelBufferGetBytesPerRow(buf)
    let base = CVPixelBufferGetBaseAddress(buf)!.assumingMemoryBound(to: UInt8.self)
    for y in 0..<1792 {
        let v = UInt8((y / 64 * 37 + tick * 9) % 256)
        memset(base + y * row, Int32(v), 828 * 4)
    }
    CVPixelBufferUnlockBaseAddress(buf, [])
    return buf
}

if fake {
    grabber.setDevice("Fake iPhone")
    var tick = 0
    Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
        tick += 1
        // Still for the first 40 ticks of every 80, then changing.
        if let f = fakeFrame(tick % 80 < 40 ? 0 : tick) { grabber.put(f) }
    }
    log("ready (fake screen)")
    RunLoop.main.run()
}

// The iOS device shows up a few seconds after the property is set; retry, and
// start again when the session stops (cable out, phone restarted).
Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
    if !session.isRunning || session.inputs.isEmpty {
        for i in session.inputs { session.removeInput(i) }
        for o in session.outputs { session.removeOutput(o) }
        startCapture()
    }
}
NotificationCenter.default.addObserver(forName: AVCaptureDevice.wasDisconnectedNotification, object: nil, queue: .main) { note in
    if let d = note.object as? AVCaptureDevice, d.modelID != "iOS Device" { return }
    log("device disconnected")
    grabber.clear("no_device: the phone was disconnected")
    session.stopRunning()
}
NotificationCenter.default.addObserver(forName: .AVCaptureSessionRuntimeError, object: session, queue: .main) { note in
    let err = note.userInfo?[AVCaptureSessionErrorKey] as? NSError
    log("capture error: \(err?.localizedDescription ?? "unknown")")
    grabber.clear("no_device: the capture stopped: \(err?.localizedDescription ?? "unknown error")")
    session.stopRunning()
}
log("ready")
RunLoop.main.run()
