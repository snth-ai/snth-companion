//! `rpc` streams: request/response calls. P0 carries the read-only file
//! methods `files.list`, `files.stat`, `files.read`.
//!
//! ```text
//! -> {"id":1,"method":"files.list","params":{"path":"/Users/x/Desktop"}}
//! <- {"id":1,"result":{...}}   or   {"id":1,"error":"denied: <reason>"}
//! ```
//!
//! Access is confined to the folders the user shared (`--root`). The check
//! runs AFTER symlink resolution: the path is canonicalized and must sit
//! under a canonical root, so a symlink inside a shared folder that points
//! outside is refused. Every refusal carries a label.
//!
//! TODO(P1): the check and the open are two steps. Something on this
//! computer that swaps a path component for a symlink between them wins
//! the race. Closing it needs openat2/O_NOFOLLOW-style opening per
//! component; relevant once the synth can also write.

use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::proto::{read_line, reply_ok, write_json_line, MuxStream};
use crate::{logln, Config};

const MAX_REQUEST_LINE: usize = 1024 * 1024;
/// Largest chunk one `files.read` returns.
const READ_MAX: u64 = 4 * 1024 * 1024;
/// Most entries one `files.list` returns; the rest is flagged `truncated`.
const LIST_MAX: usize = 2000;

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Value,
}

pub async fn handle(mut s: MuxStream, cfg: &Config) {
    if reply_ok(&mut s).await.is_err() {
        return;
    }
    loop {
        let line = match read_line(&mut s, MAX_REQUEST_LINE).await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                logln!("rpc: read: {e}");
                return;
            }
        };
        let req: Request = match serde_json::from_slice(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_json_line(&mut s, &json!({"id": null, "error": format!("bad request: {e}")})).await;
                continue;
            }
        };
        let reply = match dispatch(&req, cfg).await {
            Ok(result) => json!({"id": req.id, "result": result}),
            Err(why) => {
                logln!("rpc {}: refused: {why}", req.method);
                json!({"id": req.id, "error": why})
            }
        };
        if write_json_line(&mut s, &reply).await.is_err() {
            return;
        }
    }
    let _ = s.shutdown().await;
}

async fn dispatch(req: &Request, cfg: &Config) -> Result<Value, String> {
    match req.method.as_str() {
        "files.list" | "files.stat" | "files.read" => {}
        other => return Err(format!("denied: unknown method {other:?}")),
    }
    if cfg.roots.is_empty() {
        return Err("denied: files are turned off (no folder is shared)".into());
    }
    let path = req
        .params
        .get("path")
        .and_then(Value::as_str)
        .ok_or("bad request: params.path is required")?;
    let real = resolve(path, &cfg.roots).await?;
    match req.method.as_str() {
        "files.list" => list(&real).await,
        "files.stat" => stat(&real).await,
        _ => {
            let offset = req.params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let length = req.params.get("length").and_then(Value::as_u64).unwrap_or(0);
            read(&real, offset, length).await
        }
    }
    .map_err(|e| format!("{}: {e}", req.method))
}

/// Canonicalize `path` and require it to be inside one of `roots` (which
/// are canonical already).
///
/// A path that does not exist is "not found" only when its nearest
/// existing ancestor is inside a root. Otherwise it is "denied", the same
/// answer an existing outside path gets, so the synth cannot probe which
/// paths exist outside the shared folders.
async fn resolve(path: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("denied: path must be absolute".into());
    }
    let inside = |real: &Path| roots.iter().any(|root| real.starts_with(root));
    let outside = || format!("denied: {path} is outside the shared folders (checked after resolving symlinks)");

    match tokio::fs::canonicalize(p).await {
        Ok(real) => {
            if inside(&real) {
                Ok(real)
            } else {
                Err(outside())
            }
        }
        Err(_) => {
            let mut cur = p.parent();
            while let Some(dir) = cur {
                if let Ok(real) = tokio::fs::canonicalize(dir).await {
                    if inside(&real) {
                        return Err(format!("not found: {path}"));
                    }
                    break;
                }
                cur = dir.parent();
            }
            Err(outside())
        }
    }
}

fn entry(name: &str, path: &Path, md: &std::fs::Metadata) -> Value {
    let ft = md.file_type();
    let kind = if ft.is_symlink() {
        "symlink"
    } else if ft.is_dir() {
        "dir"
    } else if ft.is_file() {
        "file"
    } else {
        "other"
    };
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    json!({
        "name": name,
        "path": path.to_string_lossy(),
        "type": kind,
        "size": md.len(),
        "mod_unix": modified,
    })
}

async fn list(dir: &Path) -> io::Result<Value> {
    let mut rd = tokio::fs::read_dir(dir).await?;
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut truncated = false;
    while let Some(de) = rd.next_entry().await? {
        if entries.len() >= LIST_MAX {
            truncated = true;
            break;
        }
        let name = de.file_name().to_string_lossy().into_owned();
        // symlink_metadata: a link is reported as a link, not followed.
        // Following it is a separate call and goes through resolve().
        let Ok(md) = tokio::fs::symlink_metadata(de.path()).await else {
            continue;
        };
        let v = entry(&name, &de.path(), &md);
        entries.push((name, v));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Value> = entries.into_iter().map(|(_, v)| v).collect();
    Ok(json!({"entries": entries, "truncated": truncated}))
}

async fn stat(path: &Path) -> io::Result<Value> {
    let md = tokio::fs::metadata(path).await?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(entry(&name, path, &md))
}

async fn read(path: &Path, offset: u64, length: u64) -> io::Result<Value> {
    // Checked before the open: opening a FIFO or a device would block.
    if !tokio::fs::metadata(path).await?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mut f = tokio::fs::File::open(path).await?;
    let md = f.metadata().await?;
    let size = md.len();
    let offset = offset.min(size);
    let want = if length == 0 || length > READ_MAX { READ_MAX } else { length };
    let want = want.min(size - offset) as usize;
    f.seek(io::SeekFrom::Start(offset)).await?;
    let mut buf = vec![0u8; want];
    let mut got = 0;
    while got < want {
        let n = f.read(&mut buf[got..]).await?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    Ok(json!({
        "data_b64": base64::engine::general_purpose::STANDARD.encode(&buf),
        "size": size,
        "eof": offset + got as u64 >= size,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("connectd-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn roots_are_enforced_after_symlink_resolution() {
        let root = tmp("root");
        let outside = tmp("outside");
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let roots = vec![root.clone()];
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // Inside.
        assert_eq!(resolve(&s(&root), &roots).await.unwrap(), root);
        assert!(resolve(&s(&root.join("a.txt")), &roots).await.is_ok());
        assert!(resolve(&s(&root.join("sub/../a.txt")), &roots).await.is_ok());

        // Outside, by every route: direct, dot-dot, symlink, sibling prefix.
        let denied = |r: Result<PathBuf, String>| r.unwrap_err().starts_with("denied: ");
        assert!(denied(resolve(&s(&outside.join("secret.txt")), &roots).await));
        assert!(denied(resolve(&s(&root.join("../")), &roots).await));
        #[cfg(unix)]
        assert!(denied(resolve(&s(&root.join("escape/secret.txt")), &roots).await));
        #[cfg(unix)]
        assert!(denied(resolve(&s(&root.join("escape")), &roots).await));
        assert!(denied(resolve("relative/path", &roots).await));
        let sibling = PathBuf::from(format!("{}-sibling", s(&root)));
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(denied(resolve(&s(&sibling), &roots).await));

        // Missing inside = not found; missing outside = denied (no oracle).
        let missing_in = resolve(&s(&root.join("nope.txt")), &roots).await.unwrap_err();
        assert!(missing_in.starts_with("not found: "), "{missing_in}");
        assert!(denied(resolve(&s(&outside.join("nope.txt")), &roots).await));
        #[cfg(unix)]
        assert!(denied(resolve(&s(&root.join("escape/nope.txt")), &roots).await));

        // The operations themselves.
        let l = list(&root).await.unwrap();
        let names: Vec<&str> = l["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        #[cfg(unix)]
        assert_eq!(names, ["a.txt", "escape", "sub"]);
        let r = read(&root.join("a.txt"), 1, 3).await.unwrap();
        assert_eq!(r["data_b64"], base64::engine::general_purpose::STANDARD.encode(b"ell"));
        assert_eq!(r["size"], 5);
        assert_eq!(r["eof"], false);
        assert!(read(&root.join("sub"), 0, 0).await.is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&sibling);
    }
}
