//! The channel between the SNTH Vault browser extension and the running
//! SNTH Connect app.
//!
//! A browser starts a native messaging host as a short-lived child process
//! that speaks length-prefixed JSON on stdio. That child is `host::main`:
//! a small relay that carries each message to the RUNNING app over a
//! per-user local endpoint (a Unix socket in a directory only the user can
//! enter, a named pipe only the user can open on Windows). It never starts
//! the app and holds no state; when the app is not running it says so.
//!
//! ```text
//! extension (service worker)
//!    |  chrome.runtime.connectNative("ai.snth.connect")
//!    v
//! helper process (this crate, `host`)        one per browser profile
//!    |  JSON lines over the per-user endpoint (`endpoint`, `transport`)
//!    v
//! the app (connect-core `browser`)           the only place with policy
//! ```
//!
//! Who may use the channel: the browser starts the helper only for an
//! extension named in the host manifest (`install`), the helper checks the
//! origin the browser passed it against the same list, and the endpoint
//! admits the same user only. Everything that arrives is untrusted input
//! and is bounded here (`frame`) before the app parses it.
//!
//! Nothing in this crate knows what the messages mean.

pub mod endpoint;
pub mod frame;
pub mod host;
pub mod install;
pub mod transport;
#[cfg(windows)]
mod win;

/// The native messaging host name. Lowercase letters, digits, dots and
/// underscores only (the browsers' rule).
pub const HOST_NAME: &str = "ai.snth.connect";

/// The extensions that may talk to the app, by id.
///
/// The first is the id the extension gets from the `key` in its own
/// manifest (an unpacked or self-distributed build). A store assigns its
/// own id when the extension is first uploaded; that id is added here in
/// the app release that follows, next to this one.
pub const EXTENSION_IDS: &[&str] = &["fmccidehlojeieenopnkfeggeghkhgpa"];

/// `chrome-extension://<id>/`, the form browsers pass to a host and write
/// into `allowed_origins`.
pub fn origin_of(extension_id: &str) -> String {
    format!("chrome-extension://{extension_id}/")
}

/// Is this origin one of our extensions? Exact match, trailing slash
/// included: that is what Chromium passes.
pub fn origin_allowed(origin: &str) -> bool {
    EXTENSION_IDS.iter().any(|id| origin == origin_of(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_extension_is_an_allowed_origin() {
        assert!(origin_allowed("chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa/"));
        // No trailing slash, another id, another scheme, a web page.
        assert!(!origin_allowed("chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa"));
        assert!(!origin_allowed("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"));
        assert!(!origin_allowed("moz-extension://fmccidehlojeieenopnkfeggeghkhgpa/"));
        assert!(!origin_allowed("https://fmccidehlojeieenopnkfeggeghkhgpa/"));
        assert!(!origin_allowed(""));
    }

    #[test]
    fn ids_and_the_host_name_have_the_shape_browsers_require() {
        for id in EXTENSION_IDS {
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| (b'a'..=b'p').contains(&b)), "{id}");
        }
        assert!(HOST_NAME
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'));
        assert!(!HOST_NAME.starts_with('.') && !HOST_NAME.ends_with('.') && !HOST_NAME.contains(".."));
    }
}
