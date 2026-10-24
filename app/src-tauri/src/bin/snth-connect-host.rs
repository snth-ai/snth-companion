//! The helper a browser starts for the SNTH Vault extension (native
//! messaging host `ai.snth.connect`). It ships inside the app, next to the
//! app's own program, and is a relay and nothing more: it passes the
//! extension's messages to the RUNNING app over a per-user local endpoint
//! and says so when the app is not running. It never starts the app.
//!
//! All of it is `connect_host::host` (crates/connect-host).

fn main() {
    connect_host::host::main()
}
