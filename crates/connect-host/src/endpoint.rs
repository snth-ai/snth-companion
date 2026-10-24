//! Where the running app listens for its helper: one endpoint per user.
//!
//! macOS:   ~/Library/Application Support/ai.snth.connect/browser/app.sock
//! Linux:   $XDG_RUNTIME_DIR/ai.snth.connect/app.sock, or
//!          ~/.local/share/ai.snth.connect/browser/app.sock
//! Windows: \\.\pipe\ai.snth.connect.browser.<the user's SID>
//!
//! macOS has a second one, for the Safari extension that ships inside the
//! app (`Endpoint::safari`, see `safari.rs`):
//!          ~/Library/Group Containers/<app group>/run/app.sock
//!
//! Both the app and the helper compute it from the user they run as, so
//! nothing has to be passed between them. On Unix the socket sits in a
//! directory only its owner can enter; on Windows the pipe carries an
//! access list with the one user on it (`transport`), and the helper checks
//! who owns the pipe before it says a word (`host`).

use std::io;

/// A debug build of the helper takes the endpoint from here when it is
/// set, so a test can point it at a test app. A release build ignores it.
pub const ENDPOINT_ENV: &str = "SNTH_CONNECT_HOST_ENDPOINT";

/// A Unix socket path or a Windows pipe name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(String);

impl Endpoint {
    /// An explicit endpoint (tests, the headless CLI).
    pub fn at(path: impl Into<String>) -> Endpoint {
        Endpoint(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The current user's endpoint.
    pub fn system() -> io::Result<Endpoint> {
        system_endpoint().map(Endpoint)
    }

    /// The endpoint the Safari extension's handler connects to: a socket
    /// in the folder of the app group the app and its extension share.
    /// The extension is sandboxed; that folder is the one place outside
    /// its own container it can open.
    #[cfg(target_os = "macos")]
    pub fn safari() -> io::Result<Endpoint> {
        Endpoint::in_app_group(crate::safari::APP_GROUP)
    }

    /// The same inside another app group (a test with a group of its own).
    #[cfg(target_os = "macos")]
    pub fn in_app_group(group: &str) -> io::Result<Endpoint> {
        let ok = !group.is_empty() && group.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !ok {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app group id"));
        }
        let path = home()?.join("Library/Group Containers").join(group).join(crate::safari::SOCKET_IN_GROUP);
        Ok(Endpoint(path.to_string_lossy().into_owned()))
    }

    /// What the helper connects to: the user's endpoint, or in a debug
    /// build the one named by `SNTH_CONNECT_HOST_ENDPOINT`.
    pub fn for_helper() -> io::Result<Endpoint> {
        if cfg!(debug_assertions) {
            if let Some(over) = std::env::var_os(ENDPOINT_ENV).filter(|v| !v.is_empty()) {
                return Ok(Endpoint(over.to_string_lossy().into_owned()));
            }
        }
        Endpoint::system()
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(unix)]
fn home() -> io::Result<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home folder (HOME is not set)"))
}

#[cfg(target_os = "macos")]
fn system_endpoint() -> io::Result<String> {
    let path = home()?
        .join("Library/Application Support")
        .join(crate::HOST_NAME)
        .join("browser/app.sock");
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_endpoint() -> io::Result<String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_absolute());
    let path = match runtime {
        Some(dir) => dir.join(crate::HOST_NAME).join("app.sock"),
        None => home()?.join(".local/share").join(crate::HOST_NAME).join("browser/app.sock"),
    };
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn system_endpoint() -> io::Result<String> {
    Ok(format!(r"\\.\pipe\{}.browser.{}", crate::HOST_NAME, crate::win::user_sid_string()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_endpoint_is_per_user_and_stable() {
        let a = Endpoint::system().unwrap();
        let b = Endpoint::system().unwrap();
        assert_eq!(a, b);
        assert!(a.as_str().contains(crate::HOST_NAME));
        #[cfg(unix)]
        {
            let home = std::env::var("HOME").unwrap_or_default();
            let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
            assert!(
                a.as_str().starts_with(&home) || (!runtime.is_empty() && a.as_str().starts_with(&runtime)),
                "{a}"
            );
            // sockaddr_un holds about 100 bytes of path.
            assert!(a.as_str().len() < 100, "{a}");
        }
        #[cfg(windows)]
        assert!(a.as_str().starts_with(r"\\.\pipe\") && a.as_str().contains(".S-1-"), "{a}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_safari_endpoint_is_in_the_app_group_folder_and_is_another_one() {
        let safari = Endpoint::safari().unwrap();
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(
            safari.as_str(),
            format!("{home}/Library/Group Containers/CZAP38JFAE.ai.snth.connect/run/app.sock")
        );
        assert_ne!(safari, Endpoint::system().unwrap());
        // sockaddr_un holds about 100 bytes of path.
        assert!(safari.as_str().len() < 100, "{safari}");
        // Only a plain group id: never a path of the caller's choosing.
        assert!(Endpoint::in_app_group("../../tmp").is_err());
        assert!(Endpoint::in_app_group("").is_err());
        assert!(Endpoint::in_app_group("CZAP38JFAE.ai.snth.connect.probe").is_ok());
    }
}
