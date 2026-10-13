//! The per-user endpoint as a listener (the app) and as a client (the
//! helper).
//!
//! Unix: a socket file, mode 0600, in a directory of mode 0700 that the
//! current user owns. The listener refuses a peer of another user, the
//! client refuses a socket of another user.
//!
//! Windows: a named pipe whose access list holds the current user and
//! nobody else, local clients only. The client hangs up on a pipe it does
//! not own (a pipe name is machine-wide, so another account could have
//! created it first).

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::endpoint::Endpoint;

pub type Reader = Box<dyn AsyncRead + Send + Unpin>;
pub type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// One connection between a helper and the app.
pub struct Conn {
    pub reader: Reader,
    pub writer: Writer,
}

fn conn<S>(stream: S) -> Conn
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (r, w) = tokio::io::split(stream);
    Conn { reader: Box::new(r), writer: Box::new(w) }
}

/// The app's side: bound once, accepts any number of helpers.
pub struct Listener {
    inner: imp::Listener,
}

impl Listener {
    /// Bind the endpoint. `AddrInUse` when another process already serves
    /// it (a second copy of the app).
    pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
        Ok(Listener { inner: imp::Listener::bind(endpoint.as_str())? })
    }

    /// The next helper of the same user. Safe to drop and call again.
    pub async fn accept(&mut self) -> io::Result<Conn> {
        self.inner.accept().await
    }
}

/// The helper's side. An error means the app is not running (or the
/// endpoint is not ours).
pub async fn connect(endpoint: &Endpoint) -> io::Result<Conn> {
    imp::connect(endpoint.as_str()).await
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    use super::{conn, Conn};

    fn uid() -> u32 {
        // Always succeeds.
        unsafe { libc::getuid() }
    }

    fn refuse(why: String) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, why)
    }

    pub struct Listener {
        listener: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub fn bind(path: &str) -> io::Result<Listener> {
            let path = PathBuf::from(path);
            let dir = path
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the endpoint has no directory"))?;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            // Not a link to somewhere else, and ours: then nobody else can
            // put a socket of their own in our place.
            let meta = std::fs::symlink_metadata(dir)?;
            if !meta.is_dir() {
                return Err(refuse(format!("{} is not a folder", dir.display())));
            }
            if meta.uid() != uid() {
                return Err(refuse(format!("{} belongs to another user", dir.display())));
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;

            if std::fs::symlink_metadata(&path).is_ok() {
                // A socket file outlives the process that made it. If
                // something answers, it is another copy of the app.
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another copy of the app already serves the browser link",
                    ));
                }
                std::fs::remove_file(&path)?;
            }
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Listener { listener, path })
        }

        pub async fn accept(&mut self) -> io::Result<Conn> {
            loop {
                let (stream, _) = self.listener.accept().await?;
                // The folder already keeps other users out; this is the
                // same rule checked on the connection itself.
                match stream.peer_cred() {
                    Ok(cred) if cred.uid() == uid() => return Ok(conn(stream)),
                    _ => continue,
                }
            }
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(path: &str) -> io::Result<Conn> {
        let owner = std::fs::symlink_metadata(Path::new(path))?.uid();
        if owner != uid() {
            return Err(refuse("the endpoint belongs to another user".into()));
        }
        let stream = UnixStream::connect(path).await?;
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid() => Ok(conn(stream)),
            _ => Err(refuse("the endpoint is served by another user".into())),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    use super::{conn, Conn};
    use crate::win::{pipe_owner_is_current_user, PipeSecurity};

    /// ERROR_PIPE_BUSY: every instance is taken for a moment.
    const PIPE_BUSY: i32 = 231;

    pub struct Listener {
        name: String,
        security: PipeSecurity,
        /// The instance the next helper connects to.
        waiting: NamedPipeServer,
    }

    fn instance(name: &str, security: &mut PipeSecurity, first: bool) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options.first_pipe_instance(first).reject_remote_clients(true);
        // The attributes outlive the call (they live in the listener).
        unsafe { options.create_with_security_attributes_raw(name, security.as_raw()) }
    }

    impl Listener {
        pub fn bind(name: &str) -> io::Result<Listener> {
            let mut security = PipeSecurity::for_current_user()?;
            // `first`: fails when a pipe of this name exists already,
            // whoever made it.
            let waiting = instance(name, &mut security, true).map_err(|e| {
                if e.raw_os_error() == Some(5) || e.raw_os_error() == Some(PIPE_BUSY) {
                    io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another process already serves the browser link",
                    )
                } else {
                    e
                }
            })?;
            Ok(Listener { name: name.to_string(), security, waiting })
        }

        pub async fn accept(&mut self) -> io::Result<Conn> {
            self.waiting.connect().await?;
            let next = instance(&self.name, &mut self.security, false)?;
            Ok(conn(std::mem::replace(&mut self.waiting, next)))
        }
    }

    pub async fn connect(name: &str) -> io::Result<Conn> {
        let mut tries = 0;
        let client = loop {
            match ClientOptions::new().open(name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && tries < 20 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        };
        if !pipe_owner_is_current_user(client.as_raw_handle())? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the endpoint was created by another user",
            ));
        }
        Ok(conn(client))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("connect-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[tokio::test]
    async fn a_helper_reaches_the_app_and_bytes_go_both_ways() {
        let dir = temp_dir("rt");
        let ep = Endpoint::at(dir.join("browser/app.sock").to_string_lossy());
        let mut listener = Listener::bind(&ep).unwrap();

        // Only the owner may enter the folder or touch the socket.
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("browser")), 0o700);
        assert_eq!(mode(&dir.join("browser/app.sock")), 0o600);

        let client = tokio::spawn({
            let ep = ep.clone();
            async move {
                let mut c = connect(&ep).await.unwrap();
                c.writer.write_all(b"ping\n").await.unwrap();
                let mut buf = [0u8; 5];
                c.reader.read_exact(&mut buf).await.unwrap();
                buf
            }
        });
        let mut served = listener.accept().await.unwrap();
        let mut buf = [0u8; 5];
        served.reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping\n");
        served.writer.write_all(b"pong\n").await.unwrap();
        assert_eq!(&client.await.unwrap(), b"pong\n");

        // A second helper while the first is still connected.
        let second = tokio::spawn({
            let ep = ep.clone();
            async move { connect(&ep).await.is_ok() }
        });
        let kept = listener.accept().await;
        assert!(kept.is_ok());
        assert!(second.await.unwrap());
        drop(kept);

        drop(listener);
        assert!(!dir.join("browser/app.sock").exists(), "the socket file goes with the listener");
        assert!(connect(&ep).await.is_err(), "no app, no connection");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_second_app_cannot_take_the_endpoint_but_a_stale_file_is_replaced() {
        let dir = temp_dir("dup");
        let ep = Endpoint::at(dir.join("app.sock").to_string_lossy());
        let first = Listener::bind(&ep).unwrap();
        let err = Listener::bind(&ep).err().expect("second bind must fail");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        // The first one still works after the failed attempt.
        assert!(dir.join("app.sock").exists());
        drop(first);

        // A crash leaves a file behind with nobody listening.
        let stale = dir.join("stale.sock");
        std::fs::write(&stale, b"").unwrap();
        let ep2 = Endpoint::at(stale.to_string_lossy());
        assert!(Listener::bind(&ep2).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_folder_that_is_not_ours_is_refused() {
        // /tmp itself belongs to root (or is world-writable): never bind
        // straight into a shared folder.
        let shared = if cfg!(target_os = "macos") { "/private/tmp" } else { "/tmp" };
        let ep = Endpoint::at(format!("{shared}/connect-host-refused-{}.sock", std::process::id()));
        let err = Listener::bind(&ep).err().expect("must refuse a shared folder");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");

        // A link in place of the folder is refused too.
        let dir = temp_dir("link");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("real");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("browser")).unwrap();
        let ep = Endpoint::at(dir.join("browser/app.sock").to_string_lossy());
        let err = Listener::bind(&ep).err().expect("must refuse a link");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
