//! The helper as a stand-alone program (tests, the self-test). The one
//! that ships inside the app is the same function, built as
//! `snth-connect-host` in the app's own package.

fn main() {
    connect_host::host::main()
}
