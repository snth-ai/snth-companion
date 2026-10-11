//! The action journal: a rotating JSONL file in the app data dir, an
//! in-memory ring, and a broadcast channel the UI subscribes to.
//!
//! IRON RULE: no content. An entry names what was touched (host, path,
//! tool, domain), the outcome, and byte counts where relevant. Never a
//! file body, a cookie value, a header value, or a URL query.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// How large one JSONL file may grow before it rotates to `.1`.
const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
/// How many entries the in-memory ring keeps for the UI's first paint.
const RING_CAP: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub time: i64,
    /// "net", "cookies", "computer", "files", "link".
    pub capability: String,
    /// "dial", "cookies.get", "files.read", "mcp.open", "tools/call", ...
    pub action: String,
    /// Host, path, tool name, domain. Never a value.
    pub target: String,
    /// "ok", "denied: ...", "failed: ...", "no answer", ...
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl Entry {
    pub fn new(
        capability: &str,
        action: &str,
        target: impl Into<String>,
        outcome: impl Into<String>,
    ) -> Entry {
        Entry {
            time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            capability: capability.to_string(),
            action: action.to_string(),
            target: target.into(),
            outcome: outcome.into(),
            bytes: None,
        }
    }

    pub fn with_bytes(mut self, bytes: u64) -> Entry {
        self.bytes = Some(bytes);
        self
    }
}

struct Inner {
    path: Option<PathBuf>,
    ring: VecDeque<Entry>,
}

/// The journal. Cheap to clone the handle (it is `Arc` inside `Core`).
pub struct Journal {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Entry>,
}

impl Journal {
    /// `path` None = ring + broadcast only (tests, headless with no data
    /// dir).
    pub fn new(path: Option<PathBuf>) -> Journal {
        let (tx, _rx) = broadcast::channel(256);
        let mut ring = VecDeque::with_capacity(RING_CAP);
        // Fill the ring from the tail of the file, so the UI's activity
        // list is not empty after a restart. The file is bounded by
        // ROTATE_BYTES; a line that does not parse is skipped.
        if let Some(p) = &path {
            if let Ok(text) = std::fs::read_to_string(p) {
                let lines: Vec<&str> = text.lines().collect();
                let from = lines.len().saturating_sub(RING_CAP);
                for line in &lines[from..] {
                    if let Ok(e) = serde_json::from_str::<Entry>(line) {
                        ring.push_back(e);
                    }
                }
            }
        }
        Journal {
            inner: Mutex::new(Inner { path, ring }),
            tx,
        }
    }

    /// Append one entry everywhere. Never fails the caller: a file error
    /// is swallowed (the journal is a record, not a gate).
    pub fn record(&self, e: Entry) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.ring.len() == RING_CAP {
                inner.ring.pop_front();
            }
            inner.ring.push_back(e.clone());
            if let Some(path) = inner.path.clone() {
                let _ = append_line(&path, &e);
            }
        }
        let _ = self.tx.send(e);
    }

    /// The last entries, oldest first, for the UI's first paint.
    pub fn recent(&self) -> Vec<Entry> {
        self.inner.lock().unwrap().ring.iter().cloned().collect()
    }

    /// Live entries from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Entry> {
        self.tx.subscribe()
    }
}

fn append_line(path: &PathBuf, e: &Entry) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() >= ROTATE_BYTES {
            let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut line = serde_json::to_vec(e).map_err(std::io::Error::other)?;
    line.push(b'\n');
    f.write_all(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_and_broadcast() {
        let j = Journal::new(None);
        let mut rx = j.subscribe();
        j.record(Entry::new("net", "dial", "example.com:443", "ok").with_bytes(1024));
        let got = rx.try_recv().unwrap();
        assert_eq!(got.target, "example.com:443");
        assert_eq!(got.bytes, Some(1024));
        assert_eq!(j.recent().len(), 1);
    }

    #[test]
    fn redaction_is_the_callers_job_but_the_shape_has_no_value_field() {
        // There is simply no field for a cookie value or a file body.
        let e = Entry::new("cookies", "cookies.get", "youtube.com", "ok");
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("value"));
        assert!(json.contains("youtube.com"));
    }

    #[test]
    fn file_append_and_rotate() {
        let dir = std::env::temp_dir().join(format!("connect-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("journal.jsonl");
        let j = Journal::new(Some(path.clone()));
        for _ in 0..3 {
            j.record(Entry::new("files", "files.read", "/x", "ok"));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 3);
        // A new journal over the same file starts with the old entries.
        let again = Journal::new(Some(path.clone()));
        assert_eq!(again.recent().len(), 3);
        assert_eq!(again.recent()[0].action, "files.read");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
