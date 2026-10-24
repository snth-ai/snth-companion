//! File access confined to the folders the owner shared.
//!
//! Every path is checked AFTER symlink resolution, so a symlink inside a
//! shared folder that points outside is refused; the check runs on create
//! too (the parent directory is resolved and must sit inside a writable
//! root). There is no delete and no search in v1.
//!
//! Paths that do not exist return "not found" only when a shared root is
//! their nearest existing ancestor; otherwise "denied", so the synth
//! cannot probe which paths exist outside the shared folders.
//!
//! TOCTOU: the confinement check and the open are two steps. A process on
//! this computer that swaps a path component for a symlink between them
//! could still win the race. Closing it fully needs per-component
//! `openat2`/`O_NOFOLLOW` opening (Linux) or `O_SYMLINK` dance (macOS);
//! what remains is documented in README. We narrow it by resolving the
//! final path with `canonicalize` immediately before the open and, for
//! writes, by confining to the canonical parent.

use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::settings::Root;

/// Largest chunk one `files.read` returns / one `files.write` carries.
pub const READ_MAX: u64 = 4 * 1024 * 1024;
pub const WRITE_MAX: usize = 4 * 1024 * 1024;
/// Most entries one `files.list` returns; the rest is flagged `truncated`.
const LIST_MAX: usize = 2000;

/// A path in the form people and the synth write it. On Windows
/// `canonicalize` answers in the verbatim form (`\\?\C:\Users\...`,
/// `\\?\UNC\server\share\...`); this turns it back into `C:\Users\...` /
/// `\\server\share\...`. Elsewhere the path is returned as it is.
pub fn plain(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut parts = path.components();
        let head = match parts.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(letter) => format!("{}:\\", letter as char),
                Prefix::VerbatimUNC(server, share) => {
                    format!("\\\\{}\\{}\\", server.to_string_lossy(), share.to_string_lossy())
                }
                _ => return path.to_path_buf(),
            },
            _ => return path.to_path_buf(),
        };
        let mut out = PathBuf::from(head);
        for part in parts {
            if !matches!(part, Component::RootDir) {
                out.push(part);
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// The root a resolved path sits in, if any. Both sides are compared in
/// their resolved, plain form: a root is stored the way the owner picked
/// it (`C:\Users\me\Desktop`), a resolved path comes back from the system
/// in another spelling on Windows, and a root may itself sit behind a link.
fn which_root<'a>(real: &Path, roots: &'a [Root]) -> Option<&'a Root> {
    let real = plain(real);
    roots.iter().find(|r| {
        let root = std::fs::canonicalize(&r.path)
            .map(|p| plain(&p))
            .unwrap_or_else(|_| plain(&r.path));
        real.starts_with(root)
    })
}

/// Resolve a path for reading. Canonical path inside a root, or a labeled
/// error.
pub async fn resolve_read(path: &str, roots: &[Root]) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("denied: path must be absolute".into());
    }
    let outside =
        || format!("denied: {path} is outside the shared folders (checked after resolving symlinks)");
    match tokio::fs::canonicalize(p).await {
        Ok(real) => {
            if which_root(&real, roots).is_some() {
                Ok(plain(&real))
            } else {
                Err(outside())
            }
        }
        Err(_) => Err(not_found_or_denied(p, roots, path).await),
    }
}

/// Resolve a path for writing: it must land inside a WRITABLE root, after
/// resolving symlinks, including when the file does not exist yet (then
/// the parent directory is resolved and must be inside a writable root).
pub async fn resolve_write(path: &str, roots: &[Root]) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("denied: path must be absolute".into());
    }
    let outside =
        || format!("denied: {path} is outside the shared folders (checked after resolving symlinks)");
    // Existing path: resolve it directly.
    if let Ok(real) = tokio::fs::canonicalize(p).await {
        return match which_root(&real, roots) {
            Some(r) if r.write => Ok(plain(&real)),
            Some(_) => Err(format!("denied: {path} is in a read-only shared folder")),
            None => Err(outside()),
        };
    }
    // New file: the immediate parent must resolve inside a writable root.
    let Some(parent) = p.parent() else {
        return Err(outside());
    };
    let Some(name) = p.file_name() else {
        return Err("denied: path has no file name".into());
    };
    match tokio::fs::canonicalize(parent).await {
        Ok(real_parent) => match which_root(&real_parent, roots) {
            Some(r) if r.write => Ok(plain(&real_parent).join(name)),
            Some(_) => Err(format!("denied: {path} is in a read-only shared folder")),
            None => Err(outside()),
        },
        Err(_) => Err(not_found_or_denied(parent, roots, path).await),
    }
}

/// If some existing ancestor of `p` sits inside a root, the missing path
/// is "not found"; otherwise "denied" (no existence oracle outside roots).
async fn not_found_or_denied(p: &Path, roots: &[Root], shown: &str) -> String {
    let mut cur = p.parent();
    while let Some(dir) = cur {
        if let Ok(real) = tokio::fs::canonicalize(dir).await {
            if which_root(&real, roots).is_some() {
                return format!("not found: {shown}");
            }
            break;
        }
        cur = dir.parent();
    }
    format!("denied: {shown} is outside the shared folders (checked after resolving symlinks)")
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

pub async fn list(dir: &Path) -> io::Result<Value> {
    let mut rd = tokio::fs::read_dir(dir).await?;
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut truncated = false;
    while let Some(de) = rd.next_entry().await? {
        if entries.len() >= LIST_MAX {
            truncated = true;
            break;
        }
        let name = de.file_name().to_string_lossy().into_owned();
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

pub async fn stat(path: &Path) -> io::Result<Value> {
    let md = tokio::fs::metadata(path).await?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(entry(&name, path, &md))
}

pub async fn read(path: &Path, offset: u64, length: u64) -> io::Result<Value> {
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

/// Write one chunk. `abs` is the already-confined absolute path.
///
///  - create: the file must not exist (`exists:` otherwise), offset 0;
///  - replace: create or truncate, offset 0;
///  - append: the file must exist and offset must equal its size.
///
/// Returns the file's size after the write, or a labeled error string.
pub async fn write(abs: &Path, mode: &str, offset: u64, data: &[u8]) -> Result<u64, String> {
    if data.len() > WRITE_MAX {
        return Err(format!(
            "chunk of {} bytes exceeds {WRITE_MAX}",
            data.len()
        ));
    }
    match mode {
        "create" => {
            if offset != 0 {
                return Err("create offset must be 0".into());
            }
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(abs)
                .await
            {
                Ok(mut f) => {
                    f.write_all(data).await.map_err(|e| format!("write: {e}"))?;
                    f.flush().await.map_err(|e| format!("flush: {e}"))?;
                    Ok(data.len() as u64)
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    Err(format!("exists: {}", abs.display()))
                }
                Err(e) => Err(format!("open: {e}")),
            }
        }
        "replace" => {
            if offset != 0 {
                return Err("replace offset must be 0".into());
            }
            let mut f = tokio::fs::File::create(abs)
                .await
                .map_err(|e| format!("open: {e}"))?;
            f.write_all(data).await.map_err(|e| format!("write: {e}"))?;
            f.flush().await.map_err(|e| format!("flush: {e}"))?;
            Ok(data.len() as u64)
        }
        "append" => {
            let md = tokio::fs::metadata(abs)
                .await
                .map_err(|_| format!("not found: {}", abs.display()))?;
            if offset != md.len() {
                return Err(format!(
                    "append offset {offset} must equal the current size {}",
                    md.len()
                ));
            }
            let mut f = tokio::fs::OpenOptions::new()
                .append(true)
                .open(abs)
                .await
                .map_err(|e| format!("open: {e}"))?;
            f.write_all(data).await.map_err(|e| format!("write: {e}"))?;
            f.flush().await.map_err(|e| format!("flush: {e}"))?;
            Ok(offset + data.len() as u64)
        }
        other => Err(format!("unknown write mode {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("connect-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn root(p: &Path, write: bool) -> Root {
        Root {
            path: p.to_path_buf(),
            write,
        }
    }

    #[test]
    fn plain_paths() {
        #[cfg(windows)]
        {
            assert_eq!(plain(Path::new(r"\\?\C:\Users\me\Desktop\a.txt")), PathBuf::from(r"C:\Users\me\Desktop\a.txt"));
            assert_eq!(plain(Path::new(r"\\?\C:\")), PathBuf::from(r"C:\"));
            assert_eq!(plain(Path::new(r"\\?\UNC\server\share\dir\f")), PathBuf::from(r"\\server\share\dir\f"));
            assert_eq!(plain(Path::new(r"C:\Users\me")), PathBuf::from(r"C:\Users\me"));
            assert_eq!(plain(Path::new(r"\\server\share\x")), PathBuf::from(r"\\server\share\x"));
        }
        #[cfg(not(windows))]
        assert_eq!(plain(Path::new("/Users/me/Desktop")), PathBuf::from("/Users/me/Desktop"));
    }

    /// A root as the owner picked it (not in the system's resolved
    /// spelling), with a space and a non-ASCII name in it: reads and
    /// writes inside it work, and the paths handed back are plain.
    #[tokio::test]
    async fn a_root_spelled_the_way_it_was_picked() {
        let base = std::env::temp_dir().join(format!("connect-files-picked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let picked = base.join("Рабочий стол one");
        std::fs::create_dir_all(&picked).unwrap();
        std::fs::write(picked.join("note.txt"), b"hi").unwrap();
        let roots = vec![root(&picked, true)];
        let s = |p: &Path| p.to_string_lossy().into_owned();

        let read = resolve_read(&s(&picked.join("note.txt")), &roots).await.unwrap();
        assert!(!s(&read).starts_with(r"\\?\"), "{}", read.display());
        assert!(read.ends_with("note.txt"));
        assert_eq!(std::fs::read(&read).unwrap(), b"hi");

        let new_file = resolve_write(&s(&picked.join("new.txt")), &roots).await.unwrap();
        assert!(!s(&new_file).starts_with(r"\\?\"), "{}", new_file.display());
        write(&new_file, "create", 0, b"x").await.unwrap();
        assert!(picked.join("new.txt").is_file());

        let listed = list(&resolve_read(&s(&picked), &roots).await.unwrap()).await.unwrap();
        let first = listed["entries"][0]["path"].as_str().unwrap().to_string();
        assert!(!first.starts_with(r"\\?\"), "{first}");

        // Outside stays outside.
        std::fs::write(base.join("secret.txt"), b"s").unwrap();
        assert!(resolve_read(&s(&base.join("secret.txt")), &roots).await.unwrap_err().starts_with("denied: "));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn read_confinement_after_symlinks() {
        let r = tmp("root");
        let outside = tmp("outside");
        std::fs::write(r.join("a.txt"), b"hello").unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, r.join("escape")).unwrap();
        let roots = vec![root(&r, false)];
        let s = |p: &Path| p.to_string_lossy().into_owned();

        assert!(resolve_read(&s(&r.join("a.txt")), &roots).await.is_ok());
        let denied = |x: Result<PathBuf, String>| x.unwrap_err().starts_with("denied: ");
        assert!(denied(resolve_read(&s(&outside.join("secret.txt")), &roots).await));
        #[cfg(unix)]
        assert!(denied(resolve_read(&s(&r.join("escape/secret.txt")), &roots).await));
        let nf = resolve_read(&s(&r.join("nope.txt")), &roots).await.unwrap_err();
        assert!(nf.starts_with("not found: "), "{nf}");
        assert!(denied(resolve_read(&s(&outside.join("nope.txt")), &roots).await));

        let _ = std::fs::remove_dir_all(&r);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn write_create_replace_append() {
        let r = tmp("w");
        let roots = vec![root(&r, true)];
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let f = r.join("out.txt");

        // create
        let abs = resolve_write(&s(&f), &roots).await.unwrap();
        assert_eq!(write(&abs, "create", 0, b"hello").await.unwrap(), 5);
        // create again -> exists:
        let abs2 = resolve_write(&s(&f), &roots).await.unwrap();
        assert!(write(&abs2, "create", 0, b"x").await.unwrap_err().starts_with("exists: "));
        // append at the wrong offset
        let abs3 = resolve_write(&s(&f), &roots).await.unwrap();
        assert!(write(&abs3, "append", 1, b"!").await.is_err());
        // append at the right offset
        assert_eq!(write(&abs3, "append", 5, b" world").await.unwrap(), 11);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "hello world");
        // replace
        assert_eq!(write(&abs3, "replace", 0, b"new").await.unwrap(), 3);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");

        let _ = std::fs::remove_dir_all(&r);
    }

    #[tokio::test]
    async fn write_refused_in_readonly_root_and_outside() {
        let r = tmp("ro");
        let outside = tmp("ro-out");
        let roots = vec![root(&r, false)];
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let e = resolve_write(&s(&r.join("x.txt")), &roots).await.unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        let e2 = resolve_write(&s(&outside.join("x.txt")), &roots).await.unwrap_err();
        assert!(e2.starts_with("denied: "), "{e2}");
        let _ = std::fs::remove_dir_all(&r);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn write_create_cannot_escape_via_symlinked_parent() {
        #[cfg(unix)]
        {
            let r = tmp("esc");
            let outside = tmp("esc-out");
            std::os::unix::fs::symlink(&outside, r.join("link")).unwrap();
            let roots = vec![root(&r, true)];
            let s = |p: &Path| p.to_string_lossy().into_owned();
            // r/link/evil.txt resolves its parent to `outside`, outside the root.
            let e = resolve_write(&s(&r.join("link/evil.txt")), &roots)
                .await
                .unwrap_err();
            assert!(e.starts_with("denied: "), "{e}");
            let _ = std::fs::remove_dir_all(&r);
            let _ = std::fs::remove_dir_all(&outside);
        }
    }
}
