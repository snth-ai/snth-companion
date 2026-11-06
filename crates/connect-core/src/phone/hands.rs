//! The phone's hands: the SNTH Phone Bridge board (an ESP32-S3 that is a
//! Bluetooth keyboard and mouse for the phone), on its USB serial port.
//!
//! The board takes one JSON object per line at 115200 baud and answers
//! each with a line carrying the same `id` (firmware/main/cmd.c: status
//! key type move corner click down up scroll media release_all unpair
//! reboot abort). Other lines (its log) are skipped.
//!
//! TRAP (the reason this file looks the way it does): on the S3's own USB
//! port, RTS up with DTR down resets the chip, and a reset drops the
//! phone's Bluetooth link. macOS raises both lines when the port opens.
//! So the port is opened once, both lines are held up while it is open,
//! and on close RTS goes down BEFORE DTR (`Held`). It stays open for as
//! long as the phone is switched on: one thread owns it and runs the
//! commands from a queue, one at a time.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::eyes::Fut;

/// The board's USB ids: Espressif's USB Serial/JTAG.
pub const BOARD_VID: u16 = 0x303A;
pub const BOARD_PID: u16 = 0x1001;
pub const BAUD: u32 = 115_200;

/// The board as `phone_control.status` reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HandsStatus {
    /// The board answers.
    pub ok: bool,
    pub port: String,
    /// The phone is connected to the board over Bluetooth (and the link is
    /// encrypted: the board refuses input otherwise).
    pub ble_connected: bool,
    pub firmware: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Why a command did not run.
#[derive(Debug, Clone, PartialEq)]
pub enum HandsError {
    /// No board, or its port cannot be used.
    NoBoard(String),
    /// The board is there; the phone is not connected to it.
    NotConnected,
    /// The board refused or did not answer.
    Failed(String),
}

pub trait Hands: Send + Sync {
    /// One board command (without `id`); its answer.
    fn call(&self, cmd: Value, timeout: Duration) -> Fut<'_, Result<Value, HandsError>>;
    fn status(&self) -> Fut<'_, HandsStatus>;
}

/// No board at all (a system where it cannot be used).
pub struct NoHands(pub String);

impl Hands for NoHands {
    fn call(&self, _: Value, _: Duration) -> Fut<'_, Result<Value, HandsError>> {
        let why = self.0.clone();
        Box::pin(async move { Err(HandsError::NoBoard(why)) })
    }
    fn status(&self) -> Fut<'_, HandsStatus> {
        let why = self.0.clone();
        Box::pin(async move { HandsStatus { error: why, ..Default::default() } })
    }
}

// --- the serial port ---

/// A serial port, as the owner thread uses it.
pub trait Port: Send {
    fn set_dtr(&mut self, on: bool) -> std::io::Result<()>;
    fn set_rts(&mut self, on: bool) -> std::io::Result<()>;
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()>;
    /// What arrived, waiting up to `wait` for the first byte. Ok(0) =
    /// nothing in that time.
    fn read(&mut self, buf: &mut [u8], wait: Duration) -> std::io::Result<usize>;
    /// Drop what arrived and was not read.
    fn clear_input(&mut self) -> std::io::Result<()>;
}

/// Finds and opens the board's port.
pub trait Opener: Send + Sync {
    /// The board's port, or why there is none.
    fn find(&self) -> Result<String, String>;
    fn open(&self, path: &str) -> Result<Box<dyn Port>, String>;
}

/// An open port that does not reset the board: both lines up while it is
/// open, and on close (drop) RTS down first, then DTR, then the port.
pub struct Held {
    port: Option<Box<dyn Port>>,
}

impl Held {
    pub fn new(mut port: Box<dyn Port>) -> Result<Held, String> {
        let up = port.set_dtr(true).and_then(|_| port.set_rts(true));
        let held = Held { port: Some(port) };
        up.map_err(|e| format!("cannot raise DTR and RTS: {e}"))?;
        Ok(held)
    }

    fn port(&mut self) -> &mut dyn Port {
        self.port.as_mut().expect("open until dropped").as_mut()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(mut p) = self.port.take() {
            // The order is the point: RTS before DTR.
            let _ = p.set_rts(false);
            let _ = p.set_dtr(false);
            drop(p);
        }
    }
}

enum Job {
    Call { cmd: Value, timeout: Duration, reply: oneshot::Sender<Result<Value, HandsError>> },
}

/// The board, owned by one thread. Dropping the last handle closes the
/// port (RTS, then DTR).
pub struct Board {
    tx: Mutex<std_mpsc::Sender<Job>>,
    port: Arc<Mutex<String>>,
}

impl Board {
    /// Start the owner thread. `fixed` = a port the owner named in the
    /// settings instead of finding the board by its USB id.
    pub fn start(opener: Arc<dyn Opener>, fixed: Option<String>) -> Board {
        let (tx, rx) = std_mpsc::channel::<Job>();
        let port = Arc::new(Mutex::new(String::new()));
        let shown = port.clone();
        let _ = std::thread::Builder::new()
            .name("phone-board".into())
            .spawn(move || owner(opener, fixed, rx, shown));
        Board { tx: Mutex::new(tx), port }
    }

    async fn run(&self, cmd: Value, timeout: Duration) -> Result<Value, HandsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .lock()
            .unwrap()
            .send(Job::Call { cmd, timeout, reply })
            .map_err(|_| HandsError::NoBoard("the board link is stopped".into()))?;
        rx.await.unwrap_or_else(|_| Err(HandsError::NoBoard("the board link is stopped".into())))
    }
}

impl Hands for Board {
    fn call(&self, cmd: Value, timeout: Duration) -> Fut<'_, Result<Value, HandsError>> {
        Box::pin(self.run(cmd, timeout))
    }

    fn status(&self) -> Fut<'_, HandsStatus> {
        Box::pin(async move {
            let answer = self.run(json!({"cmd": "status"}), Duration::from_secs(2)).await;
            let port = self.port.lock().unwrap().clone();
            match answer {
                Ok(v) => HandsStatus {
                    ok: true,
                    port,
                    ble_connected: v.get("connected").and_then(Value::as_bool) == Some(true),
                    firmware: v.get("version").and_then(Value::as_str).unwrap_or("").to_string(),
                    error: String::new(),
                },
                Err(HandsError::NoBoard(why)) | Err(HandsError::Failed(why)) => HandsStatus { port, error: why, ..Default::default() },
                Err(HandsError::NotConnected) => HandsStatus { ok: true, port, ..Default::default() },
            }
        })
    }
}

/// The owner thread: opens the port when a command needs it, keeps it
/// open, closes it when the board goes away or the handle is dropped.
fn owner(opener: Arc<dyn Opener>, fixed: Option<String>, rx: std_mpsc::Receiver<Job>, shown: Arc<Mutex<String>>) {
    let mut open: Option<(String, Held)> = None;
    let mut next_id: u64 = 1;
    let mut pending = Vec::new();
    // Open at once, so the board is ready (and its status known) before
    // the first command.
    let _ = ensure_open(&opener, &fixed, &mut open, &shown);
    while let Ok(job) = rx.recv() {
        let Job::Call { mut cmd, timeout, reply } = job;
        if let Err(why) = ensure_open(&opener, &fixed, &mut open, &shown) {
            let _ = reply.send(Err(HandsError::NoBoard(why)));
            continue;
        }
        let (path, held) = open.as_mut().expect("just opened");
        let id = next_id;
        next_id += 1;
        if let Some(o) = cmd.as_object_mut() {
            o.insert("id".into(), json!(id));
        }
        let result = exchange(held.port(), &cmd, id, timeout, &mut pending);
        let result = match result {
            Exchange::Answer(v) => answer(v),
            Exchange::Timeout => Err(HandsError::Failed(format!(
                "no answer from the board in {:.0} s (is another program using {path}?)",
                timeout.as_secs_f64()
            ))),
            Exchange::Gone(e) => {
                // Unplugged: close (the lines first) and find it again next time.
                let why = format!("the board went away ({path}: {e})");
                crate::logln!("phone: {why}");
                open = None;
                shown.lock().unwrap().clear();
                Err(HandsError::NoBoard(why))
            }
        };
        let _ = reply.send(result);
    }
    // The phone was switched off: `open` drops here, RTS before DTR.
    drop(open);
}

fn ensure_open(opener: &Arc<dyn Opener>, fixed: &Option<String>, open: &mut Option<(String, Held)>, shown: &Mutex<String>) -> Result<(), String> {
    if open.is_some() {
        return Ok(());
    }
    let path = match fixed.as_ref().filter(|p| !p.is_empty()) {
        Some(p) => p.clone(),
        None => opener.find()?,
    };
    let port = opener.open(&path)?;
    let held = Held::new(port)?;
    crate::logln!("phone: the board is on {path}");
    *shown.lock().unwrap() = path.clone();
    *open = Some((path, held));
    Ok(())
}

enum Exchange {
    Answer(Value),
    Timeout,
    Gone(std::io::Error),
}

/// Write one command line, read lines until the one with our id.
fn exchange(port: &mut dyn Port, cmd: &Value, id: u64, timeout: Duration, buf: &mut Vec<u8>) -> Exchange {
    let mut line = serde_json::to_vec(cmd).unwrap_or_default();
    line.push(b'\n');
    // What the board said while nobody asked (its log) is of no use.
    buf.clear();
    let _ = port.clear_input();
    if let Err(e) = port.write_all(&line) {
        return Exchange::Gone(e);
    }
    let deadline = Instant::now() + timeout;
    let mut chunk = [0u8; 1024];
    loop {
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=i).collect();
            if let Some(v) = parse_line(&raw) {
                if v.get("id").and_then(Value::as_u64) == Some(id) {
                    return Exchange::Answer(v);
                }
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Exchange::Timeout;
        }
        match port.read(&mut chunk, (deadline - now).min(Duration::from_millis(50))) {
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Exchange::Gone(e),
        }
        if buf.len() > 64 * 1024 {
            buf.clear();
        }
    }
}

/// A JSON object line, or None (a log line).
pub fn parse_line(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim();
    if !text.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Some(v),
        _ => None,
    }
}

/// The board's answer as a result.
fn answer(v: Value) -> Result<Value, HandsError> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(v);
    }
    let error = v.get("error").and_then(Value::as_str).unwrap_or("failed");
    if error == "not_connected" {
        return Err(HandsError::NotConnected);
    }
    match v.get("detail").and_then(Value::as_str) {
        Some(d) => Err(HandsError::Failed(format!("{error}: {d}"))),
        None => Err(HandsError::Failed(error.to_string())),
    }
}

// --- finding the board on macOS ---

/// The board's port in `ioreg -r -c IOUSBHostDevice -l -w0`: the USB
/// device with the board's ids, then the serial port under it.
pub fn find_in_ioreg(text: &str, vid: u16, pid: u16) -> Option<String> {
    let want_vid = format!("\"idVendor\" = {vid}");
    let want_pid = format!("\"idProduct\" = {pid}");
    // One block per device: a line starting with "+-o" at the left edge.
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if line.starts_with("+-o") {
            blocks.push(vec![line]);
        } else if let Some(b) = blocks.last_mut() {
            b.push(line);
        }
    }
    for block in blocks {
        // The device's own properties: up to its first child.
        let own: Vec<&str> = block.iter().skip(1).take_while(|l| !l.contains("+-o")).copied().collect();
        let has = |w: &str| own.iter().any(|l| l.trim_start_matches([' ', '|']).trim() == w);
        if !(has(&want_vid) && has(&want_pid)) {
            continue;
        }
        for l in &block {
            if let Some(rest) = l.split("\"IOCalloutDevice\" = \"").nth(1) {
                if let Some(path) = rest.split('"').next().filter(|p| p.starts_with("/dev/")) {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

/// The real serial port (macOS; termios, any Unix).
#[cfg(unix)]
pub mod posix {
    use std::ffi::CString;
    use std::time::Duration;

    use super::{Opener, Port, BOARD_PID, BOARD_VID};

    pub struct PosixPort {
        fd: libc::c_int,
    }

    fn err() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    impl PosixPort {
        pub fn open(path: &str) -> Result<PosixPort, String> {
            let c = CString::new(path).map_err(|_| format!("bad port name {path:?}"))?;
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
            if fd < 0 {
                let e = err();
                return Err(match e.raw_os_error() {
                    Some(libc::EBUSY) => format!("the board's port {path} is busy: another program holds it"),
                    Some(libc::ENOENT) => format!("the board's port {path} is gone"),
                    _ => format!("cannot open the board's port {path}: {e}"),
                });
            }
            let port = PosixPort { fd };
            // Nobody else opens it while we hold it.
            if unsafe { libc::ioctl(fd, libc::TIOCEXCL as _) } < 0 {
                return Err(format!("cannot hold the board's port {path}: {}", err()));
            }
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut t) < 0 {
                    return Err(format!("cannot read the settings of {path}: {}", err()));
                }
                libc::cfmakeraw(&mut t);
                t.c_cflag |= libc::CLOCAL | libc::CREAD;
                t.c_cc[libc::VMIN] = 0;
                t.c_cc[libc::VTIME] = 0;
                libc::cfsetispeed(&mut t, libc::B115200);
                libc::cfsetospeed(&mut t, libc::B115200);
                if libc::tcsetattr(fd, libc::TCSANOW, &t) < 0 {
                    return Err(format!("cannot set up {path}: {}", err()));
                }
            }
            Ok(port)
        }

        fn line(&mut self, bit: libc::c_int, on: bool) -> std::io::Result<()> {
            let req = if on { libc::TIOCMBIS } else { libc::TIOCMBIC };
            if unsafe { libc::ioctl(self.fd, req as _, &bit as *const libc::c_int) } < 0 {
                return Err(err());
            }
            Ok(())
        }
    }

    impl Drop for PosixPort {
        fn drop(&mut self) {
            unsafe {
                libc::ioctl(self.fd, libc::TIOCNXCL as _);
                libc::close(self.fd);
            }
        }
    }

    impl Port for PosixPort {
        fn set_dtr(&mut self, on: bool) -> std::io::Result<()> {
            self.line(libc::TIOCM_DTR, on)
        }
        fn set_rts(&mut self, on: bool) -> std::io::Result<()> {
            self.line(libc::TIOCM_RTS, on)
        }
        fn write_all(&mut self, mut data: &[u8]) -> std::io::Result<()> {
            while !data.is_empty() {
                let n = unsafe { libc::write(self.fd, data.as_ptr() as *const libc::c_void, data.len()) };
                if n < 0 {
                    let e = err();
                    match e.kind() {
                        std::io::ErrorKind::WouldBlock => {
                            let mut p = libc::pollfd { fd: self.fd, events: libc::POLLOUT, revents: 0 };
                            unsafe { libc::poll(&mut p, 1, 100) };
                            continue;
                        }
                        std::io::ErrorKind::Interrupted => continue,
                        _ => return Err(e),
                    }
                }
                data = &data[n as usize..];
            }
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8], wait: Duration) -> std::io::Result<usize> {
            let mut p = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
            let ms = wait.as_millis().min(i32::MAX as u128) as libc::c_int;
            let r = unsafe { libc::poll(&mut p, 1, ms) };
            if r < 0 {
                let e = err();
                return if e.kind() == std::io::ErrorKind::Interrupted { Ok(0) } else { Err(e) };
            }
            if r == 0 {
                return Ok(0);
            }
            if p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && p.revents & libc::POLLIN == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the port hung up"));
            }
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n < 0 {
                let e = err();
                return if e.kind() == std::io::ErrorKind::WouldBlock { Ok(0) } else { Err(e) };
            }
            if n == 0 {
                // poll said readable, read says end: the device is gone.
                return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the port closed"));
            }
            Ok(n as usize)
        }
        fn clear_input(&mut self) -> std::io::Result<()> {
            if unsafe { libc::tcflush(self.fd, libc::TCIFLUSH) } < 0 {
                return Err(err());
            }
            Ok(())
        }
    }

    /// The board by its USB ids (macOS: from ioreg).
    pub struct SystemOpener;

    impl Opener for SystemOpener {
        fn find(&self) -> Result<String, String> {
            #[cfg(target_os = "macos")]
            {
                let out = crate::proc::std_command("/usr/sbin/ioreg")
                    .args(["-r", "-c", "IOUSBHostDevice", "-l", "-w0"])
                    .output()
                    .map_err(|e| format!("cannot list the USB devices: {e}"))?;
                let text = String::from_utf8_lossy(&out.stdout);
                super::find_in_ioreg(&text, BOARD_VID, BOARD_PID)
                    .ok_or_else(|| "the SNTH Phone Bridge board is not plugged in (no USB device 303a:1001 with a serial port)".to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (BOARD_VID, BOARD_PID);
                Err("finding the board works on macOS only; name its port in the settings".into())
            }
        }
        fn open(&self, path: &str) -> Result<Box<dyn Port>, String> {
            Ok(Box::new(PosixPort::open(path)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// What the fake board says to a line.
    type Reply = Arc<dyn Fn(&Value) -> Vec<u8> + Send + Sync>;

    /// A port that records every call and answers from a script.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);
    impl Log {
        fn push(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }
        fn all(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakePort {
        log: Log,
        /// What the board "says" after each written line: a function of
        /// the line.
        answer: Reply,
        inbox: VecDeque<u8>,
        fail_reads: bool,
    }

    impl Port for FakePort {
        fn set_dtr(&mut self, on: bool) -> std::io::Result<()> {
            self.log.push(format!("dtr={}", on as u8));
            Ok(())
        }
        fn set_rts(&mut self, on: bool) -> std::io::Result<()> {
            self.log.push(format!("rts={}", on as u8));
            Ok(())
        }
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            let v: Value = serde_json::from_slice(data).unwrap();
            self.log.push(format!("write {}", v["cmd"].as_str().unwrap_or("?")));
            let out = (self.answer)(&v);
            self.inbox.extend(out);
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8], wait: Duration) -> std::io::Result<usize> {
            if self.fail_reads {
                return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "unplugged"));
            }
            if self.inbox.is_empty() {
                std::thread::sleep(wait.min(Duration::from_millis(5)));
                return Ok(0);
            }
            // A few bytes at a time, as a serial port gives them.
            let n = buf.len().min(7).min(self.inbox.len());
            for b in buf.iter_mut().take(n) {
                *b = self.inbox.pop_front().unwrap();
            }
            Ok(n)
        }
        fn clear_input(&mut self) -> std::io::Result<()> {
            self.inbox.clear();
            Ok(())
        }
    }

    impl Drop for FakePort {
        fn drop(&mut self) {
            self.log.push("close");
        }
    }

    struct FakeOpener {
        log: Log,
        present: bool,
        fail_reads: bool,
        answer: Reply,
    }

    impl Opener for FakeOpener {
        fn find(&self) -> Result<String, String> {
            if self.present {
                Ok("/dev/cu.usbmodem3101".into())
            } else {
                Err("the SNTH Phone Bridge board is not plugged in".into())
            }
        }
        fn open(&self, path: &str) -> Result<Box<dyn Port>, String> {
            self.log.push(format!("open {path}"));
            Ok(Box::new(FakePort { log: self.log.clone(), answer: self.answer.clone(), inbox: VecDeque::new(), fail_reads: self.fail_reads }))
        }
    }

    /// The firmware's answers: a log line first, then the reply with the
    /// same id; `connected` says whether the phone is on Bluetooth.
    fn firmware(connected: bool) -> Reply {
        Arc::new(move |v: &Value| {
            let id = v["id"].clone();
            let reply = match v["cmd"].as_str().unwrap() {
                "status" => json!({"ok": true, "id": id, "connected": connected, "version": "0.1.0", "name": "SNTH Phone Bridge"}),
                _ if !connected => json!({"ok": false, "id": id, "error": "not_connected"}),
                "key" if v["combo"] == "nope" => json!({"ok": false, "id": id, "error": "bad_args", "detail": "unknown key: nope"}),
                _ => json!({"ok": true, "id": id}),
            };
            // A stale answer to another id, a log line, then ours.
            let mut out = b"I (123) ble: something\n{\"ok\":true,\"id\":999999}\n".to_vec();
            out.extend(serde_json::to_vec(&reply).unwrap());
            out.extend(b"\r\n");
            out
        })
    }

    fn board(log: &Log, present: bool, connected: bool) -> Board {
        Board::start(Arc::new(FakeOpener { log: log.clone(), present, fail_reads: false, answer: firmware(connected) }), None)
    }

    async fn wait_for(log: &Log, what: &str) {
        for _ in 0..200 {
            if log.all().iter().any(|l| l == what) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("never saw {what:?} in {:?}", log.all());
    }

    #[tokio::test]
    async fn the_port_opens_once_with_both_lines_up_and_closes_rts_before_dtr() {
        let log = Log::default();
        let b = board(&log, true, true);
        let s = b.status().await;
        assert!(s.ok && s.ble_connected, "{s:?}");
        assert_eq!(s.port, "/dev/cu.usbmodem3101");
        assert_eq!(s.firmware, "0.1.0");
        b.call(json!({"cmd": "click", "button": "left"}), Duration::from_secs(1)).await.unwrap();
        b.call(json!({"cmd": "key", "combo": "cmd+h"}), Duration::from_secs(1)).await.unwrap();
        drop(b);
        wait_for(&log, "close").await;
        assert_eq!(
            log.all(),
            ["open /dev/cu.usbmodem3101", "dtr=1", "rts=1", "write status", "write click", "write key", "rts=0", "dtr=0", "close"],
            "one open for every command, and RTS drops before DTR"
        );
    }

    #[tokio::test]
    async fn the_answers_map_to_the_error_classes() {
        let log = Log::default();
        let b = board(&log, true, false);
        let s = b.status().await;
        assert!(s.ok && !s.ble_connected, "the board answers, the phone is not on Bluetooth: {s:?}");
        assert_eq!(b.call(json!({"cmd": "click"}), Duration::from_secs(1)).await, Err(HandsError::NotConnected));

        let b = board(&Log::default(), true, true);
        assert_eq!(
            b.call(json!({"cmd": "key", "combo": "nope"}), Duration::from_secs(1)).await,
            Err(HandsError::Failed("bad_args: unknown key: nope".into()))
        );

        let gone = Log::default();
        let b = board(&gone, false, true);
        let s = b.status().await;
        assert!(!s.ok && s.error.contains("not plugged in"), "{s:?}");
        assert!(matches!(b.call(json!({"cmd": "click"}), Duration::from_secs(1)).await, Err(HandsError::NoBoard(_))));
        assert!(gone.all().is_empty(), "nothing was opened");
    }

    #[tokio::test]
    async fn an_unplugged_board_closes_the_port_and_is_found_again() {
        let log = Log::default();
        let b = Board::start(
            Arc::new(FakeOpener { log: log.clone(), present: true, fail_reads: true, answer: firmware(true) }),
            None,
        );
        let e = b.call(json!({"cmd": "click"}), Duration::from_secs(1)).await.unwrap_err();
        assert!(matches!(&e, HandsError::NoBoard(w) if w.contains("went away")), "{e:?}");
        // The next command opens it again.
        let _ = b.call(json!({"cmd": "click"}), Duration::from_secs(1)).await;
        let all = log.all();
        assert_eq!(&all[..6], ["open /dev/cu.usbmodem3101", "dtr=1", "rts=1", "write click", "rts=0", "dtr=0"], "{all:?}");
        assert_eq!(all.iter().filter(|l| l.starts_with("open")).count(), 2, "{all:?}");
    }

    #[tokio::test]
    async fn a_silent_board_times_out() {
        let log = Log::default();
        let b = Board::start(Arc::new(FakeOpener { log, present: true, fail_reads: false, answer: Arc::new(|_| Vec::new()) }), None);
        let e = b.call(json!({"cmd": "click"}), Duration::from_millis(100)).await.unwrap_err();
        assert!(matches!(&e, HandsError::Failed(w) if w.contains("no answer")), "{e:?}");
    }

    #[test]
    fn log_lines_are_skipped() {
        assert_eq!(parse_line(b"I (12) cmd: executor ready\n"), None);
        assert_eq!(parse_line(b"{broken\n"), None);
        assert_eq!(parse_line(b"{\"ok\":true,\"id\":3}\r\n"), Some(json!({"ok": true, "id": 3})));
    }

    #[test]
    fn the_board_is_found_by_its_usb_ids_in_ioreg() {
        let text = r#"+-o USB2.0 Hub@01100000  <class IOUSBHostDevice, id 0x100000a01, registered, matched, active, busy 0 (12 ms), retain 30>
  | {
  |   "idProduct" = 2337
  |   "idVendor" = 1507
  | }
  | 
  +-o USB JTAG/serial debug unit@01110000  <class IOUSBHostDevice, id 0x100000b02, registered, matched, active, busy 0 (3 ms), retain 25>
    | {
    |   "idProduct" = 4097
    |   "idVendor" = 12346
    | }
+-o Other Serial@02100000  <class IOUSBHostDevice, id 0x100000c03, registered, matched, active, busy 0 (3 ms), retain 25>
  | {
  |   "idProduct" = 29987
  |   "idVendor" = 6790
  | }
  | 
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface>
      +-o IOSerialBSDClient  <class IOSerialBSDClient>
            {
              "IOCalloutDevice" = "/dev/cu.usbserial-110"
            }
+-o USB JTAG/serial debug unit@01110000  <class IOUSBHostDevice, id 0x100000b02, registered, matched, active, busy 0 (3 ms), retain 25>
  | {
  |   "USB Product Name" = "USB JTAG/serial debug unit"
  |   "idProduct" = 4097
  |   "idVendor" = 12346
  | }
  | 
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x100000b05>
  | +-o AppleUSBACMControl  <class AppleUSBACMControl>
  +-o IOUSBHostInterface@1  <class IOUSBHostInterface, id 0x100000b07>
    +-o AppleUSBACMData  <class AppleUSBACMData>
      +-o IOSerialBSDClient  <class IOSerialBSDClient>
          {
            "IODialinDevice" = "/dev/tty.usbmodem3101"
            "IOCalloutDevice" = "/dev/cu.usbmodem3101"
          }
"#;
        assert_eq!(find_in_ioreg(text, BOARD_VID, BOARD_PID).as_deref(), Some("/dev/cu.usbmodem3101"));
        assert_eq!(find_in_ioreg(text, 6790, 29987).as_deref(), Some("/dev/cu.usbserial-110"));
        assert_eq!(find_in_ioreg(text, 1, 2), None);
        // The board under a hub is found as its own root, not through the hub.
        assert_eq!(find_in_ioreg(&text[..text.find("+-o Other").unwrap()], BOARD_VID, BOARD_PID), None);
    }
}
