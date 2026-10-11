//! Reading the owner's browser cookies for one domain.
//!
//! `cookies.get{domain}` (connect.md) reads the cookies that apply to a
//! domain and its subdomains from the owner's browsers. v1 only READS;
//! the synth keeps its own jar and never asks the app to write a cookie.
//!
//! IRON RULE: a cookie VALUE never appears in a log line or the journal.
//! The journal records domain, count and browser only. The value is
//! carried back to the synth over the rpc stream and nowhere else.
//!
//! HARD LIMIT for this session: nothing here reads a real browser DB or
//! touches the real OS keychain. The key source is a trait so tests
//! inject a known key, and the reader always works on a COPY of the DB
//! (the browser holds a lock on the original). The live read is verified
//! later, with the owner present.
//!
//! Chromium on macOS and Linux encrypts each value as
//! `"v10"` or `"v11"` + AES-128-CBC(key, iv=16 spaces, PKCS7), where
//! key = PBKDF2-HMAC-SHA1(safe-storage-password, "saltysalt", 1003, 16).
//! Firefox stores values in the clear. Chromium on Windows (AES-256-GCM,
//! app-bound) is out of scope in v1 and returns a labeled `unavailable`.

use std::io;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use serde::{Deserialize, Serialize};

/// One cookie in the wire shape of connect.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireCookie {
    pub name: String,
    pub value: String,
    /// Keeps the browser's form: a leading dot for a domain cookie, none
    /// for a host-only cookie.
    pub domain: String,
    pub path: String,
    /// Unix seconds, 0 for a session cookie.
    pub expires: i64,
    pub secure: bool,
    pub http_only: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub same_site: String,
}

// A cookie's `String` never shows the value, so a stray debug print cannot
// leak it.
impl std::fmt::Display for WireCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cookie({}@{})", self.name, self.domain)
    }
}

/// The result of a read: the cookies and the browser they came from.
#[derive(Debug)]
pub struct CookieRead {
    pub cookies: Vec<WireCookie>,
    pub source: String,
}

#[derive(Debug)]
pub enum CookieError {
    /// Policy / capability: cannot read cookies on this OS + browser.
    Unavailable(String),
    /// No cookies anywhere for the domain (not an error on the wire, but
    /// handy internally). Callers usually turn an empty read into an ok.
    NotFound,
    /// A real failure (copy, open, decrypt).
    Failed(String),
}

impl std::fmt::Display for CookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CookieError::Unavailable(s) => write!(f, "unavailable: {s}"),
            CookieError::NotFound => f.write_str("not found: no cookies for that domain"),
            CookieError::Failed(s) => write!(f, "{s}"),
        }
    }
}

/// Source of the Chromium "Safe Storage" password. The real macOS impl
/// reads the Keychain; tests inject a known password; Linux uses the
/// fixed fallback password or the Secret Service.
pub trait KeySource: Send + Sync {
    /// The raw safe-storage password for a browser, or None if there is
    /// no key to be had (Firefox needs none; a Windows Chrome returns
    /// None here and the reader reports `unavailable`).
    fn safe_storage_password(&self, browser: &str) -> io::Result<Option<Vec<u8>>>;
}

/// Reads cookies for a domain. The core holds one of these.
pub trait CookieSource: Send + Sync {
    fn get(&self, domain: &str) -> Result<CookieRead, CookieError>;
}

/// Does a cookie host apply to `want` (a registrable/base domain) or one
/// of its subdomains? Handles the leading-dot form.
pub fn host_applies(cookie_host: &str, want: &str) -> bool {
    let h = cookie_host.trim_start_matches('.').to_ascii_lowercase();
    let w = want.trim_start_matches('.').to_ascii_lowercase();
    h == w || h.ends_with(&format!(".{w}"))
}

/// PBKDF2-HMAC-SHA1(password, "saltysalt", 1003, 16): the Chromium
/// macOS/Linux key schedule.
pub fn derive_key(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", 1003, &mut key);
    key
}

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Decrypt one Chromium `v10`/`v11` value. Returns the plaintext as a
/// lossy UTF-8 string, or None when the blob is not decryptable.
pub fn decrypt_chromium(enc: &[u8], key: &[u8; 16]) -> Option<String> {
    if enc.len() < 3 {
        return None;
    }
    let prefix = &enc[..3];
    if prefix != b"v10" && prefix != b"v11" {
        // An unencrypted value (no scheme prefix) is returned as-is: some
        // Chromium builds store non-secret values in the clear.
        return String::from_utf8(enc.to_vec()).ok();
    }
    let body = &enc[3..];
    if body.is_empty() || body.len() % 16 != 0 {
        return None;
    }
    let iv = [b' '; 16];
    let dec = Aes128CbcDec::new(key.into(), &iv.into());
    let mut buf = body.to_vec();
    let plain = dec.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;
    // Newer Chromium prepends a 32-byte SHA-256 of the domain to the
    // plaintext; drop it when the remainder is valid UTF-8 and the full
    // string is not. Fixtures carry no such prefix.
    if let Ok(s) = std::str::from_utf8(plain) {
        return Some(s.to_string());
    }
    if plain.len() > 32 {
        if let Ok(s) = std::str::from_utf8(&plain[32..]) {
            return Some(s.to_string());
        }
    }
    Some(String::from_utf8_lossy(plain).into_owned())
}

/// Copy a locked DB to a temp file next to our data, so the browser's
/// lock does not block the read. The caller deletes the copy.
fn copy_to_temp(db: &Path) -> io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let uniq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = format!(
        "connect-cookie-copy-{}-{nanos}-{uniq}-{}.sqlite",
        std::process::id(),
        db.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("db")
            .replace('/', "_")
    );
    let tmp = std::env::temp_dir().join(name);
    std::fs::copy(db, &tmp)?;
    // WAL sidecars, if present, must travel with the DB.
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", db.display()));
        if side.exists() {
            let _ = std::fs::copy(&side, format!("{}{ext}", tmp.display()));
        }
    }
    Ok(tmp)
}

struct TempCopy(PathBuf);
impl Drop for TempCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        for ext in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", self.0.display()));
        }
    }
}

fn chromium_same_site(v: i64) -> &'static str {
    match v {
        0 => "none",
        1 => "lax",
        2 => "strict",
        _ => "",
    }
}

/// expires_utc is microseconds since 1601-01-01; convert to unix seconds.
fn chromium_expiry(expires_utc: i64) -> i64 {
    if expires_utc == 0 {
        return 0;
    }
    expires_utc / 1_000_000 - 11_644_473_600
}

/// Read a Chromium-family cookie DB (Chrome, Brave, Edge, Arc) for a
/// domain. `browser` names the source and selects the key.
pub fn read_chromium(
    db: &Path,
    browser: &str,
    want: &str,
    keys: &dyn KeySource,
) -> Result<CookieRead, CookieError> {
    let password = keys
        .safe_storage_password(browser)
        .map_err(|e| CookieError::Failed(format!("read key for {browser}: {e}")))?;
    let Some(password) = password else {
        return Err(CookieError::Unavailable(format!(
            "no Safe Storage key for {browser} on this OS (Chrome on Windows encrypts cookies per application; not supported yet)"
        )));
    };
    let key = derive_key(&password);

    let tmp = copy_to_temp(db).map_err(|e| CookieError::Failed(format!("copy db: {e}")))?;
    let _guard = TempCopy(tmp.clone());
    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CookieError::Failed(format!("open db: {e}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT host_key, name, encrypted_value, path, expires_utc, \
             is_secure, is_httponly, samesite FROM cookies",
        )
        .map_err(|e| CookieError::Failed(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7).unwrap_or(-1),
            ))
        })
        .map_err(|e| CookieError::Failed(format!("query: {e}")))?;

    let mut cookies = Vec::new();
    for row in rows {
        let (host, name, enc, path, expires_utc, secure, httponly, samesite) =
            row.map_err(|e| CookieError::Failed(format!("row: {e}")))?;
        if !host_applies(&host, want) {
            continue;
        }
        let Some(value) = decrypt_chromium(&enc, &key) else {
            continue;
        };
        cookies.push(WireCookie {
            name,
            value,
            domain: host,
            path,
            expires: chromium_expiry(expires_utc),
            secure: secure != 0,
            http_only: httponly != 0,
            same_site: chromium_same_site(samesite).to_string(),
        });
    }
    Ok(CookieRead {
        cookies,
        source: browser.to_string(),
    })
}

fn firefox_same_site(v: i64) -> &'static str {
    match v {
        0 => "none",
        1 => "lax",
        2 => "strict",
        _ => "",
    }
}

/// Read a Firefox cookies.sqlite for a domain. Values are plaintext.
pub fn read_firefox(db: &Path, want: &str) -> Result<CookieRead, CookieError> {
    let tmp = copy_to_temp(db).map_err(|e| CookieError::Failed(format!("copy db: {e}")))?;
    let _guard = TempCopy(tmp.clone());
    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CookieError::Failed(format!("open db: {e}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies",
        )
        .map_err(|e| CookieError::Failed(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7).unwrap_or(-1),
            ))
        })
        .map_err(|e| CookieError::Failed(format!("query: {e}")))?;

    let mut cookies = Vec::new();
    for row in rows {
        let (host, name, value, path, expiry, secure, httponly, samesite) =
            row.map_err(|e| CookieError::Failed(format!("row: {e}")))?;
        if !host_applies(&host, want) {
            continue;
        }
        cookies.push(WireCookie {
            name,
            value,
            domain: host,
            path,
            expires: expiry, // Firefox stores unix seconds already
            secure: secure != 0,
            http_only: httponly != 0,
            same_site: firefox_same_site(samesite).to_string(),
        });
    }
    Ok(CookieRead {
        cookies,
        source: "firefox".to_string(),
    })
}

/// A cookie source over an explicit list of DBs: the real app builds this
/// from the installed-browser paths; tests build it from fixtures. It
/// tries each source in order and returns the first that yields any
/// cookie; otherwise the first `Unavailable`, else an empty read.
pub struct MultiReader {
    pub chromium: Vec<(String, PathBuf)>,
    pub firefox: Vec<PathBuf>,
    pub keys: std::sync::Arc<dyn KeySource>,
}

impl CookieSource for MultiReader {
    fn get(&self, domain: &str) -> Result<CookieRead, CookieError> {
        let mut last_unavailable: Option<CookieError> = None;
        for (browser, db) in &self.chromium {
            if !db.exists() {
                continue;
            }
            match read_chromium(db, browser, domain, self.keys.as_ref()) {
                Ok(r) if !r.cookies.is_empty() => return Ok(r),
                Ok(_) => {}
                Err(CookieError::Unavailable(m)) => {
                    last_unavailable = Some(CookieError::Unavailable(m))
                }
                Err(e) => return Err(e),
            }
        }
        for db in &self.firefox {
            if !db.exists() {
                continue;
            }
            match read_firefox(db, domain) {
                Ok(r) if !r.cookies.is_empty() => return Ok(r),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(u) = last_unavailable {
            return Err(u);
        }
        Ok(CookieRead {
            cookies: Vec::new(),
            source: String::new(),
        })
    }
}

pub mod fixtures {
    //! Build an encrypted Chromium-style cookie DB and a Firefox one with
    //! a known key, so the readers can be tested (and the CLI exercised)
    //! without any real browser or keychain.

    use super::KeySource;
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use std::io;
    use std::path::Path;

    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

    /// (host, name, value, path, expires, secure, httponly, samesite).
    pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str, i64, i64, i64, i64);

    /// Encrypt a value the way Chromium does (v10 + AES-128-CBC).
    pub fn encrypt_chromium(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
        let iv = [b' '; 16];
        let enc = Aes128CbcEnc::new(key.into(), &iv.into());
        // PKCS7 output is the next multiple of 16 strictly above len.
        let mut buf = vec![0u8; (plain.len() / 16 + 1) * 16];
        buf[..plain.len()].copy_from_slice(plain);
        let ct = enc
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .expect("pkcs7 pad");
        let mut out = b"v10".to_vec();
        out.extend_from_slice(ct);
        out
    }

    /// Write a Chromium cookies DB at `path` with the given rows
    /// (host, name, value, path, expires_utc, secure, httponly, samesite),
    /// encrypting each value with `key`.
    pub fn write_chromium_db(
        path: &Path,
        key: &[u8; 16],
        rows: &[Row],
    ) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, encrypted_value BLOB, \
             path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, \
             samesite INTEGER);",
        )
        .unwrap();
        for (host, name, value, p, exp, sec, ho, ss) in rows {
            let enc = encrypt_chromium(value.as_bytes(), key);
            conn.execute(
                "INSERT INTO cookies VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![host, name, enc, p, exp, sec, ho, ss],
            )
            .unwrap();
        }
    }

    pub fn write_firefox_db(
        path: &Path,
        rows: &[Row],
    ) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, \
             expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER);",
        )
        .unwrap();
        for (host, name, value, p, exp, sec, ho, ss) in rows {
            conn.execute(
                "INSERT INTO moz_cookies VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![host, name, value, p, exp, sec, ho, ss],
            )
            .unwrap();
        }
    }

    pub struct FixedKey(pub Vec<u8>);
    impl KeySource for FixedKey {
        fn safe_storage_password(&self, _browser: &str) -> io::Result<Option<Vec<u8>>> {
            Ok(Some(self.0.clone()))
        }
    }

    pub struct NoKey;
    impl KeySource for NoKey {
        fn safe_storage_password(&self, _browser: &str) -> io::Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use std::sync::Arc;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("connect-cookie-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn host_matching() {
        assert!(host_applies(".youtube.com", "youtube.com"));
        assert!(host_applies("www.youtube.com", "youtube.com"));
        assert!(host_applies("youtube.com", "youtube.com"));
        assert!(!host_applies("evil.com", "youtube.com"));
        assert!(!host_applies("notyoutube.com", "youtube.com"));
    }

    #[test]
    fn chromium_roundtrip_and_filter() {
        let dir = tmpdir("chrome");
        let db = dir.join("Cookies");
        let key = derive_key(b"test-safe-storage-pw");
        write_chromium_db(
            &db,
            &key,
            &[
                (".youtube.com", "SID", "secret-sid", "/", 0, 1, 1, 1),
                ("www.youtube.com", "PREF", "x", "/", 16_000_000_000_000_000, 0, 0, 0),
                (".evil.com", "EVIL", "no", "/", 0, 1, 1, 1),
            ],
        );
        let keys = FixedKey(b"test-safe-storage-pw".to_vec());
        let r = read_chromium(&db, "chrome", "youtube.com", &keys).unwrap();
        assert_eq!(r.source, "chrome");
        let mut names: Vec<_> = r.cookies.iter().map(|c| c.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["PREF", "SID"]);
        let sid = r.cookies.iter().find(|c| c.name == "SID").unwrap();
        assert_eq!(sid.value, "secret-sid");
        assert_eq!(sid.expires, 0); // session
        assert!(sid.secure && sid.http_only);
        assert_eq!(sid.same_site, "lax");
        let pref = r.cookies.iter().find(|c| c.name == "PREF").unwrap();
        assert!(pref.expires > 0); // converted from microseconds-since-1601
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn firefox_plaintext() {
        let dir = tmpdir("ff");
        let db = dir.join("cookies.sqlite");
        write_firefox_db(
            &db,
            &[
                (".example.com", "A", "plain", "/", 1_900_000_000, 1, 0, 1),
                ("other.com", "B", "no", "/", 0, 0, 0, 0),
            ],
        );
        let r = read_firefox(&db, "example.com").unwrap();
        assert_eq!(r.source, "firefox");
        assert_eq!(r.cookies.len(), 1);
        assert_eq!(r.cookies[0].value, "plain");
        assert_eq!(r.cookies[0].expires, 1_900_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_key_is_unavailable() {
        let dir = tmpdir("nokey");
        let db = dir.join("Cookies");
        let key = derive_key(b"pw");
        write_chromium_db(&db, &key, &[(".x.com", "A", "v", "/", 0, 1, 1, 0)]);
        let err = read_chromium(&db, "chrome", "x.com", &NoKey).unwrap_err();
        assert!(matches!(err, CookieError::Unavailable(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multireader_prefers_a_hit_then_falls_back() {
        let dir = tmpdir("multi");
        let chrome = dir.join("Cookies");
        let ff = dir.join("cookies.sqlite");
        let key = derive_key(b"pw");
        write_chromium_db(&chrome, &key, &[(".a.com", "C", "v", "/", 0, 1, 1, 0)]);
        write_firefox_db(&ff, &[(".b.com", "F", "v", "/", 0, 1, 1, 0)]);
        let mr = MultiReader {
            chromium: vec![("chrome".into(), chrome)],
            firefox: vec![ff],
            keys: Arc::new(FixedKey(b"pw".to_vec())),
        };
        assert_eq!(mr.get("a.com").unwrap().cookies.len(), 1);
        assert_eq!(mr.get("b.com").unwrap().cookies.len(), 1);
        assert_eq!(mr.get("none.com").unwrap().cookies.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_never_shows_value() {
        let c = WireCookie {
            name: "SID".into(),
            value: "super-secret".into(),
            domain: ".x.com".into(),
            path: "/".into(),
            expires: 0,
            secure: true,
            http_only: true,
            same_site: "lax".into(),
        };
        assert!(!format!("{c}").contains("super-secret"));
    }
}
