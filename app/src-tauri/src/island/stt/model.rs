//! The speech-to-text model files: where they live, the download (with
//! resume and retries), the check against the pinned sha256, removal.
//!
//! Two files from Hugging Face, each pinned to a repository revision and
//! a sha256: whisper large-v3-turbo quantized to q5_0 (547 MiB) and the
//! Silero voice detector whisper.cpp runs (0.9 MB). Parakeet's files
//! (`PARAKEET_FILES`) are pinned the same way; on Windows its voice
//! detector is the official Silero ONNX model, pinned to a commit of its
//! GitHub repository (`SILERO_ONNX`). They live in the app's
//! data folder under `stt/`. They are fetched only when the owner turns
//! on-device speech-to-text on (in Settings or from the synth's offer on
//! the island), never silently.
//!
//! A file is downloaded to `<name>.part`; a broken download resumes from
//! where it stopped (an HTTP range). A finished file is checked against
//! its sha256, then renamed and marked (`<name>.ok`, holding the hash),
//! so a start of the app knows the file is good without hashing half a
//! gigabyte again.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// whisper large-v3-turbo, q5_0 (ggerganov/whisper.cpp at 5359861).
pub const WHISPER: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin",
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    size: 574_041_195,
};

/// Silero VAD v5.1.2 for whisper.cpp (ggml-org/whisper-vad at 9ffd54a).
pub const VAD: ModelFile = ModelFile {
    name: "ggml-silero-v5.1.2.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v5.1.2.bin",
    sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    size: 885_098,
};

/// The small one first: the progress is about the big one anyway.
pub const FILES: [&ModelFile; 2] = [&VAD, &WHISPER];

/// NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0), istupakov's ONNX export, int8
/// (istupakov/parakeet-tdt-0.6b-v3-onnx at 8f23f0c): the second engine
/// (parakeet.rs). Its own folder (`stt/parakeet/`) with its own copy of
/// the voice detector, so either engine's files go without the other's.
/// The mel preprocessor and config.json of that repository are not
/// needed: the features are computed in parakeet.rs.
pub const PARAKEET_ENCODER: ModelFile = ModelFile {
    name: "encoder-model.int8.onnx",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/encoder-model.int8.onnx",
    sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    size: 652_183_999,
};

pub const PARAKEET_JOINT: ModelFile = ModelFile {
    name: "decoder_joint-model.int8.onnx",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/decoder_joint-model.int8.onnx",
    sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
    size: 18_202_004,
};

pub const PARAKEET_VOCAB: ModelFile = ModelFile {
    name: "vocab.txt",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/vocab.txt",
    sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    size: 93_939,
};

/// Silero VAD v5.1.2 as the official ONNX model (snakers4/silero-vad at
/// the v5.1.2 tag, 6478567; MIT), the same detector as `VAD` for builds
/// without whisper.cpp (silero.rs). 2.3 MB.
pub const SILERO_ONNX: ModelFile = ModelFile {
    name: "silero_vad.onnx",
    url: "https://raw.githubusercontent.com/snakers4/silero-vad/6478567951ae5c9979ad7b234185b5515f4be7a1/src/silero_vad/data/silero_vad.onnx",
    sha256: "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f",
    size: 2_327_524,
};

/// Parakeet's voice detector: whisper.cpp's file where whisper.cpp is in
/// the build (and where no engine is), the ONNX one on Windows.
#[cfg(not(all(device_stt, not(whisper_stt))))]
pub const PARAKEET_VAD: &ModelFile = &VAD;
#[cfg(all(device_stt, not(whisper_stt)))]
pub const PARAKEET_VAD: &ModelFile = &SILERO_ONNX;

/// The small ones first.
pub const PARAKEET_FILES: [&ModelFile; 4] = [PARAKEET_VAD, &PARAKEET_VOCAB, &PARAKEET_JOINT, &PARAKEET_ENCODER];

#[cfg(test)]
pub fn parakeet_size() -> u64 {
    PARAKEET_FILES.iter().map(|f| f.size).sum()
}

/// emotion2vec+ base (emotion2vec/emotion2vec_plus_base, FunASR Model
/// License 1.1), our ONNX export with int8 matrix products (tone/emotion.rs).
/// The tone of voice; fetched after the speech model, only while the tone
/// switch is on, and never needed for the text.
pub const EMOTION: ModelFile = ModelFile {
    name: "emotion2vec_plus_base_int8.onnx",
    url: "https://github.com/snth-ai/snth-companion/releases/download/model-emotion2vec-plus-base-1/emotion2vec_plus_base_int8.onnx",
    sha256: "fe11b794562ae75cf96d7c268893688f889a15ae9166426fdede16aba139b69c",
    size: 117_754_668,
};

pub const TONE_FILES: [&ModelFile; 1] = [&EMOTION];

/// Room kept free on the disk beyond the files themselves.
const SPARE: u64 = 300 * 1024 * 1024;
/// A failed download is tried again this many times (resuming).
const TRIES: u32 = 4;
/// Progress reaches the UI at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[cfg(test)]
pub fn total_size() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

pub fn tone_size() -> u64 {
    TONE_FILES.iter().map(|f| f.size).sum()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Missing,
    Downloading { done: u64, total: u64 },
    /// Checking the sha256.
    Verifying,
    Ready,
    /// "no_space" (detail: "<need> <free>" in bytes), "network", "checksum",
    /// "io".
    Failed { code: String, detail: String },
}

pub struct Models {
    dir: PathBuf,
    /// The files this set holds: `FILES` for speech-to-text, another list
    /// for wake by name (island/wake).
    files: &'static [&'static ModelFile],
    status: Mutex<Status>,
    cancel: AtomicBool,
    running: AtomicBool,
}

fn marker(dir: &Path, f: &ModelFile) -> PathBuf {
    dir.join(format!("{}.ok", f.name))
}

fn part(dir: &Path, f: &ModelFile) -> PathBuf {
    dir.join(format!("{}.part", f.name))
}

/// The file is in place, of the right size, and was checked.
fn good(dir: &Path, f: &ModelFile) -> bool {
    let size_ok = std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size);
    let marked = std::fs::read_to_string(marker(dir, f)).is_ok_and(|s| s.trim() == f.sha256);
    size_ok && marked
}

/// Free bytes on the disk that holds `dir`.
#[cfg(unix)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn free_space(_dir: &Path) -> Option<u64> {
    None
}

pub fn sha256_of(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().fold(String::with_capacity(64), |mut hex, b| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
        hex
    }))
}

enum Fail {
    Cancelled,
    Network(String),
    Io(String),
    Checksum(String),
}

impl Models {
    /// The speech-to-text model and the voice detector.
    pub fn new(dir: PathBuf) -> Models {
        Models::with_files(dir, &FILES)
    }

    /// A set of other pinned files, in a folder of its own.
    pub fn with_files(dir: PathBuf, files: &'static [&'static ModelFile]) -> Models {
        let status = if files.iter().all(|f| good(&dir, f)) { Status::Ready } else { Status::Missing };
        Models { dir, files, status: Mutex::new(status), cancel: AtomicBool::new(false), running: AtomicBool::new(false) }
    }

    /// Where a file of this set lives.
    pub fn path(&self, f: &ModelFile) -> PathBuf {
        self.dir.join(f.name)
    }

    /// The bytes of this set.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn ready(&self) -> bool {
        self.status() == Status::Ready
    }

    pub fn downloading(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// The model and the voice detector.
    pub fn paths(&self) -> (PathBuf, PathBuf) {
        (self.dir.join(WHISPER.name), self.dir.join(VAD.name))
    }

    fn set(&self, s: Status) {
        *self.status.lock().unwrap() = s;
    }

    /// The status as a download would leave it (tests of the hold flow).
    #[cfg(test)]
    pub fn pretend(&self, s: Status) {
        self.set(s);
    }

    /// Bytes still to fetch (the parts already on disk count as fetched).
    fn remaining(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !good(&self.dir, f))
            .map(|f| f.size.saturating_sub(std::fs::metadata(part(&self.dir, f)).map(|m| m.len()).unwrap_or(0)))
            .sum()
    }

    /// Start the download in the background (nothing when it runs or the
    /// files are there). Err("no_space") at once when the disk is short;
    /// the status says so too. `changed` is called on every change of the
    /// status (progress at most four times a second).
    pub fn download(self: &Arc<Self>, changed: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if self.ready() {
            return Ok(());
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            self.running.store(false, Ordering::Release);
            self.set(Status::Failed { code: "io".into(), detail: e.to_string() });
            changed();
            return Err("io".into());
        }
        let need = self.remaining() + SPARE;
        if let Some(free) = free_space(&self.dir) {
            if free < need {
                self.running.store(false, Ordering::Release);
                self.set(Status::Failed { code: "no_space".into(), detail: format!("{need} {free}") });
                changed();
                return Err("no_space".into());
            }
        }
        self.cancel.store(false, Ordering::Release);
        let total = self.size();
        self.set(Status::Downloading { done: total.saturating_sub(need - SPARE), total });
        changed();
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("island-stt-model".into()).spawn(move || {
            let result = me.fetch_all(&*changed);
            me.running.store(false, Ordering::Release);
            match result {
                Ok(()) => {
                    connect_core::logln!("island: model ready: {}", me.files.iter().map(|f| f.name).collect::<Vec<_>>().join(", "));
                    me.set(Status::Ready);
                }
                Err(Fail::Cancelled) => me.set(Status::Missing),
                Err(Fail::Network(e)) => me.failed("network", e),
                Err(Fail::Io(e)) => me.failed("io", e),
                Err(Fail::Checksum(e)) => me.failed("checksum", e),
            }
            changed();
        });
        if let Err(e) = spawned {
            self.running.store(false, Ordering::Release);
            return Err(e.to_string());
        }
        Ok(())
    }

    fn failed(&self, code: &str, detail: String) {
        connect_core::logln!("island: model download failed ({}): {code}: {detail}", self.files.iter().map(|f| f.name).collect::<Vec<_>>().join(", "));
        self.set(Status::Failed { code: code.into(), detail });
    }

    /// Stop a running download (the part stays, for a resume).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Remove the files (and a download's parts). Stops a download first.
    pub fn delete(&self) -> Result<(), String> {
        self.cancel();
        for _ in 0..50 {
            if !self.downloading() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut first_err = None;
        for f in self.files {
            for p in [self.dir.join(f.name), part(&self.dir, f), marker(&self.dir, f)] {
                match std::fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        first_err.get_or_insert(e.to_string());
                    }
                }
            }
        }
        self.set(Status::Missing);
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn fetch_all(&self, changed: &(dyn Fn() + Send + Sync)) -> Result<(), Fail> {
        let total: u64 = self.size();
        // Bytes of the files done before the one in progress.
        let mut base = 0u64;
        for f in self.files {
            if good(&self.dir, f) {
                base += f.size;
                continue;
            }
            let mut tries = 0;
            loop {
                match self.fetch_one(f, base, total, changed) {
                    Ok(()) => break,
                    Err(Fail::Network(e)) if tries + 1 < TRIES => {
                        tries += 1;
                        connect_core::logln!("island: model download of {} broke ({e}), resuming (try {})", f.name, tries + 1);
                        for _ in 0..(tries * 20) {
                            if self.cancel.load(Ordering::Acquire) {
                                return Err(Fail::Cancelled);
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            self.set(Status::Verifying);
            changed();
            let p = part(&self.dir, f);
            let hash = sha256_of(&p, &self.cancel).map_err(|e| if e == "cancelled" { Fail::Cancelled } else { Fail::Io(e) })?;
            if hash != f.sha256 {
                let _ = std::fs::remove_file(&p);
                return Err(Fail::Checksum(format!("{}: sha256 {hash}, expected {}", f.name, f.sha256)));
            }
            std::fs::rename(&p, self.dir.join(f.name)).map_err(|e| Fail::Io(e.to_string()))?;
            std::fs::write(marker(&self.dir, f), f.sha256).map_err(|e| Fail::Io(e.to_string()))?;
            base += f.size;
        }
        Ok(())
    }

    /// One file into its part, from where the part ends.
    fn fetch_one(&self, f: &ModelFile, base: u64, total: u64, changed: &(dyn Fn() + Send + Sync)) -> Result<(), Fail> {
        let p = part(&self.dir, f);
        let mut have = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if have > f.size {
            let _ = std::fs::remove_file(&p);
            have = 0;
        }
        if have == f.size {
            return Ok(());
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(20))
            // A read that stalls this long counts as a lost connection.
            .timeout_read(Duration::from_secs(45))
            .user_agent("snth-connect")
            .build();
        let mut req = agent.get(f.url);
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            // The range is past the end: the part is whole or broken.
            Err(ureq::Error::Status(416, _)) => {
                let _ = std::fs::remove_file(&p);
                return Err(Fail::Network("the server refused the range; starting over".into()));
            }
            Err(ureq::Error::Status(code, _)) => return Err(Fail::Network(format!("HTTP {code}"))),
            Err(ureq::Error::Transport(t)) => return Err(Fail::Network(t.to_string())),
        };
        // 206: the rest. 200: the whole file again (the server ignored
        // the range).
        let append = have > 0 && resp.status() == 206;
        if !append {
            have = 0;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&p)
            .map_err(|e| Fail::Io(e.to_string()))?;
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut last = Instant::now() - PROGRESS_EVERY;
        loop {
            if self.cancel.load(Ordering::Acquire) {
                let _ = file.flush();
                return Err(Fail::Cancelled);
            }
            let n = reader.read(&mut buf).map_err(|e| Fail::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| Fail::Io(e.to_string()))?;
            have += n as u64;
            if have > f.size {
                drop(file);
                let _ = std::fs::remove_file(&p);
                return Err(Fail::Checksum(format!("{} is larger than expected", f.name)));
            }
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                self.set(Status::Downloading { done: base + have, total });
                changed();
            }
        }
        file.sync_all().map_err(|e| Fail::Io(e.to_string()))?;
        if have != f.size {
            return Err(Fail::Network(format!("the download ended early ({have} of {} bytes)", f.size)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("connect-stt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_files_are_pinned() {
        for f in FILES.iter().chain(TONE_FILES.iter()).chain(PARAKEET_FILES.iter()).chain([&&SILERO_ONNX]) {
            // Hugging Face at a revision, a release asset of ours (a tag of
            // its own, never re-uploaded), or a GitHub file at a commit.
            assert!(
                f.url.starts_with("https://huggingface.co/")
                    || f.url.starts_with("https://github.com/snth-ai/snth-companion/releases/download/model-")
                    || f.url.starts_with("https://raw.githubusercontent.com/snakers4/silero-vad/6478567951ae5c9979ad7b234185b5515f4be7a1/"),
                "{}",
                f.url
            );
            assert!(f.url.ends_with(f.name));
            assert_eq!(f.sha256.len(), 64);
            assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            // The revision, not "main": the file cannot change under the hash.
            assert!(!f.url.contains("/resolve/main/") && !f.url.contains("/main/"), "{}", f.url);
        }
        // The Windows build's detector (no whisper.cpp there), the Mac's
        // own file elsewhere.
        if cfg!(all(device_stt, not(whisper_stt))) {
            assert_eq!(PARAKEET_VAD.name, SILERO_ONNX.name);
        } else {
            assert_eq!(PARAKEET_VAD.name, VAD.name);
        }
        assert!((540..600).contains(&(total_size() / 1_000_000)), "about 550 MB");
        assert!((100..130).contains(&(tone_size() / 1_000_000)), "about 118 MB");
        assert!((660..680).contains(&(parakeet_size() / 1_000_000)), "about 671 MB");
        assert_eq!(Models::with_files(PathBuf::from("/nonexistent"), &TONE_FILES).size(), tone_size());
    }

    #[test]
    fn a_file_counts_only_when_checked() {
        let d = tmp("good");
        assert_eq!(Models::new(d.clone()).status(), Status::Missing);
        // The right size without the mark is not enough (a crash before
        // the check), nor the mark of another hash.
        let f = &VAD;
        let file = std::fs::File::create(d.join(f.name)).unwrap();
        file.set_len(f.size).unwrap();
        assert!(!good(&d, f));
        std::fs::write(marker(&d, f), "0".repeat(64)).unwrap();
        assert!(!good(&d, f));
        std::fs::write(marker(&d, f), f.sha256).unwrap();
        assert!(good(&d, f));
        // Both files are needed.
        assert_eq!(Models::new(d.clone()).status(), Status::Missing);
        let m = Models::new(d.clone());
        m.delete().unwrap();
        assert!(!d.join(f.name).exists() && !marker(&d, f).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_part_on_disk_counts_as_fetched() {
        let d = tmp("part");
        let m = Models::new(d.clone());
        assert_eq!(m.remaining(), total_size());
        std::fs::write(part(&d, &WHISPER), vec![0u8; 1000]).unwrap();
        assert_eq!(m.remaining(), total_size() - 1000);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_hash_is_sha256() {
        let d = tmp("hash");
        let p = d.join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_of(&p, &AtomicBool::new(false)).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn free_space_is_known_here() {
        if cfg!(unix) {
            assert!(free_space(&std::env::temp_dir()).unwrap() > 0);
        }
    }

    /// The real download of Parakeet's vocabulary (a plain file behind a
    /// relative redirect, unlike the LFS ones).
    /// `cargo test -p snth-connect -- --ignored stt_parakeet_vocab_download`.
    #[test]
    #[ignore]
    fn stt_parakeet_vocab_download() {
        let d = tmp("pk-net");
        let m = Models::with_files(d.clone(), &PARAKEET_FILES);
        m.fetch_one(&PARAKEET_VOCAB, 0, PARAKEET_VOCAB.size, &|| {}).unwrap_or_else(|_| panic!("download failed"));
        assert_eq!(sha256_of(&part(&d, &PARAKEET_VOCAB), &AtomicBool::new(false)).unwrap(), PARAKEET_VOCAB.sha256);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The real download of the small file, resumed from a part. Network:
    /// `cargo test -p snth-connect -- --ignored stt_vad_download`.
    #[test]
    #[ignore]
    fn stt_vad_download_resumes_and_checks() {
        let d = tmp("net");
        let m = Arc::new(Models::new(d.clone()));
        // Half of the file, as a broken download leaves it.
        let full = ureq::get(VAD.url).call().unwrap();
        let mut bytes = Vec::new();
        full.into_reader().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, VAD.size);
        std::fs::write(part(&d, &VAD), &bytes[..400_000]).unwrap();
        m.fetch_one(&VAD, 0, VAD.size, &|| {}).unwrap_or_else(|_| panic!("resume failed"));
        assert_eq!(sha256_of(&part(&d, &VAD), &AtomicBool::new(false)).unwrap(), VAD.sha256);
        let _ = std::fs::remove_dir_all(&d);
    }
}
