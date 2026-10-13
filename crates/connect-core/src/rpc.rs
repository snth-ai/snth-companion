//! `rpc` streams: sequential request/response calls (connect.md).
//!
//! Methods: files.list / files.stat / files.read / files.write,
//! cookies.get, permission.request, device.info, and apps.<app>.<action>
//! (every one of those goes through the one gate in apps/mod.rs). One stream serves any
//! number of sequential requests; the synth opens one per call and the
//! app also serves many on one.

use std::sync::Arc;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::{self, Shared};
use crate::files;
use crate::grant::{until_unix, ApprovalRequest};
use crate::journal::Entry;
use crate::proto::{read_line, reply_ok, write_json_line, MuxStream};
use crate::{logln, wire};
use tokio::io::AsyncWriteExt;

const MAX_REQUEST_LINE: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Value,
}

pub async fn handle(mut s: MuxStream, shared: &Arc<Shared>) {
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
                let _ = write_json_line(
                    &mut s,
                    &json!({"id": Value::Null, "error": format!("bad request: {e}")}),
                )
                .await;
                continue;
            }
        };
        let reply = match dispatch(shared, &req).await {
            Ok(result) => json!({"id": req.id, "result": result}),
            Err(why) => {
                logln!("rpc {}: {why}", req.method);
                json!({"id": req.id, "error": why})
            }
        };
        if write_json_line(&mut s, &reply).await.is_err() {
            return;
        }
    }
    let _ = s.shutdown().await;
}

async fn dispatch(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    match req.method.as_str() {
        "files.list" | "files.stat" | "files.read" | "files.write" => {
            files_method(shared, req).await
        }
        "cookies.get" => cookies_get(shared, req).await,
        m if m.starts_with("apps.") => crate::apps::handle(shared, m, &req.params).await,
        "permission.request" => permission_request(shared, req).await,
        "device.info" => Ok(wire::device_info(
            &shared.device_id(),
            &shared.device_name(),
            &shared.app_version,
            &cua_version(shared).await,
        )),
        other => Err(format!("denied: unknown method {other:?}")),
    }
}

/// One call as a stream would make it, for tests of other modules.
#[cfg(test)]
pub(crate) async fn call_for_test(shared: &Arc<Shared>, method: &str, params: Value) -> Result<Value, String> {
    dispatch(shared, &Request { id: Value::Null, method: method.to_string(), params }).await
}

async fn cua_version(shared: &Shared) -> String {
    shared.cua_detect(false).await.version
        .unwrap_or_default()
}

async fn files_method(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    if shared.paused() {
        return Err("denied: files are paused by the owner".into());
    }
    let s = shared.settings();
    if !s.files.enabled {
        return Err("denied: files are turned off on this computer".into());
    }
    let path = req
        .params
        .get("path")
        .and_then(Value::as_str)
        .ok_or("bad request: params.path is required")?;

    match req.method.as_str() {
        "files.list" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            files::list(&real).await.map_err(|e| format!("files.list: {e}"))
        }
        "files.stat" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            files::stat(&real).await.map_err(|e| format!("files.stat: {e}"))
        }
        "files.read" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            let offset = req.params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let length = req.params.get("length").and_then(Value::as_u64).unwrap_or(0);
            let out = files::read(&real, offset, length)
                .await
                .map_err(|e| format!("files.read: {e}"))?;
            shared
                .journal()
                .record(Entry::new("files", "files.read", path.to_string(), "ok"));
            Ok(out)
        }
        "files.write" => {
            let abs = files::resolve_write(path, &s.files.roots).await?;
            let mode = req
                .params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("create");
            let offset = req.params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let data_b64 = req.params.get("data_b64").and_then(Value::as_str).unwrap_or("");
            let data = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| format!("files.write: bad base64: {e}"))?;

            // The careful preset asks the owner before any write.
            if s.files.ask_on_write && !shared.grants().allows("files", path) {
                let req_ask = ApprovalRequest::new(
                    "files",
                    path,
                    "",
                    format!("let the synth write to {path}"),
                );
                let appr = shared.ask_scoped("files", path, req_ask).await;
                if !appr.allow {
                    shared.journal().record(Entry::new(
                        "files",
                        "files.write",
                        path.to_string(),
                        "denied: owner",
                    ));
                    return Err(format!("denied: the owner did not allow writing to {path}"));
                }
            }

            let size = files::write(&abs, mode, offset, &data).await.map_err(|e| {
                // files::write returns a labeled string already (exists:, not found:, denied:)
                if e.starts_with("exists:") || e.starts_with("not found:") || e.starts_with("denied:") {
                    e
                } else {
                    format!("files.write: {e}")
                }
            })?;
            shared
                .journal()
                .record(Entry::new("files", "files.write", path.to_string(), "ok").with_bytes(data.len() as u64));
            Ok(json!({"size": size}))
        }
        _ => unreachable!(),
    }
}

async fn cookies_get(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    let started = std::time::Instant::now();
    if shared.paused() {
        return Err("denied: cookies are paused by the owner".into());
    }
    let s = shared.settings();
    if !s.cookies.enabled {
        return Err("denied: cookies are turned off on this computer".into());
    }
    let raw = req
        .params
        .get("domain")
        .and_then(Value::as_str)
        .ok_or("bad request: params.domain is required")?;
    let reason = req.params.get("reason").and_then(Value::as_str).unwrap_or("");
    let domain = core::registrable_domain(raw);
    // One site at a time. "com" or "co.uk" would cover every site under
    // it, so it is refused before the owner is asked anything.
    if !core::is_one_site(&domain) {
        return Err("bad request: params.domain must name one site".into());
    }
    let extension_first = shared.cookies_extension_first.load(std::sync::atomic::Ordering::Acquire);

    use crate::settings::CookiesMode;
    let need_ask = match s.cookies.mode {
        CookiesMode::Auto => false,
        CookiesMode::AskNew => !shared.grants().allows("cookies", &domain),
        CookiesMode::Ask => true,
    };
    if need_ask {
        let mut ask = ApprovalRequest::new(
            "cookies",
            &domain,
            reason,
            format!("hand the synth your browser cookies for {domain}"),
        );
        // When the answer leads to a second question, in the browser, the
        // window says so before the owner answers: the two read as one.
        if extension_first {
            if let Some((browser, false)) = shared.extension_for(&domain) {
                ask = ask.with_details(vec![(
                    "browser_step".to_string(),
                    crate::browser::browser_name(&browser).to_string(),
                )]);
            }
        }
        let appr = shared.ask_scoped("cookies", &domain, ask).await;
        if !appr.allow {
            shared
                .journal()
                .record(Entry::new("cookies", "cookies.get", domain.clone(), "denied: owner"));
            return Err(format!("denied: the owner did not allow cookies for {domain}"));
        }
    }

    // The owner allowed the site (now, earlier, or by "never ask"). Two
    // sources: a browser the owner linked through the SNTH Vault
    // extension, and the browsers' own files. Where the files of the
    // common browsers cannot be read (Chrome and Edge on Windows) the
    // extension goes first; elsewhere it is the fallback.
    let done = |r: crate::cookies::CookieRead| {
        // Journal: domain, count, browser. NEVER a value.
        shared.journal().record(Entry::new(
            "cookies",
            "cookies.get",
            format!("{domain} ({} from {})", r.cookies.len(), r.source),
            "ok",
        ));
        Ok(json!({"cookies": r.cookies, "source": r.source}))
    };
    // What is left of the answer window (the synth gives up at 60 s).
    let left = || {
        crate::grant::APPROVAL_TIMEOUT
            .saturating_sub(started.elapsed())
            .max(std::time::Duration::from_secs(3))
    };
    use crate::browser::ExtensionRead;
    let refused = |outcome: ExtensionRead| -> Option<String> {
        let why = match outcome {
            ExtensionRead::Denied(why) | ExtensionRead::NoAnswer(why) => format!("denied: {why}"),
            ExtensionRead::Failed(why) => {
                // Not the owner's decision: the other source may still serve.
                logln!("cookies.get: {why}");
                return None;
            }
            _ => return None,
        };
        shared.journal().record(Entry::new("cookies", "cookies.get", domain.clone(), why.clone()));
        Some(why)
    };

    if extension_first {
        match shared.cookies_via_extension(&domain, left(), need_ask).await {
            ExtensionRead::Cookies(r) if !r.cookies.is_empty() => return done(r),
            other => {
                if let Some(why) = refused(other) {
                    return Err(why);
                }
            }
        }
    }

    // Read on a blocking pool: rusqlite + file copy are blocking.
    let source = shared.cookies.clone();
    let dom = domain.clone();
    let read = tokio::task::spawn_blocking(move || source.get(&dom))
        .await
        .map_err(|e| format!("cookies.get: join: {e}"))?;
    let unavailable = match read {
        Ok(r) if !r.cookies.is_empty() => return done(r),
        Ok(_) | Err(crate::cookies::CookieError::NotFound) => None,
        Err(crate::cookies::CookieError::Unavailable(m)) => Some(m),
        Err(crate::cookies::CookieError::Failed(m)) => return Err(format!("cookies.get: {m}")),
    };

    if !extension_first {
        match shared.cookies_via_extension(&domain, left(), need_ask).await {
            ExtensionRead::Cookies(r) if !r.cookies.is_empty() => return done(r),
            other => {
                if let Some(why) = refused(other) {
                    return Err(why);
                }
            }
        }
    }

    match unavailable {
        // The files could not be read and no linked browser could help.
        Some(m) => Err(format!(
            "unavailable: {m}; the owner can link that browser in the SNTH Connect app (Logins) to share its sign-ins"
        )),
        None => Ok(json!({"cookies": [], "source": ""})),
    }
}

async fn permission_request(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    if shared.paused() {
        return Err("denied: paused by the owner".into());
    }
    let capability = req
        .params
        .get("capability")
        .and_then(Value::as_str)
        .ok_or("bad request: params.capability is required")?;
    let scope = req.params.get("scope").and_then(Value::as_str).unwrap_or("");
    let reason = req.params.get("reason").and_then(Value::as_str).unwrap_or("");
    // Nothing in the owner's apps can be allowed ahead of time. Each
    // change is asked about with its own facts on the screen (who a
    // message goes to and what it says), by the gate in apps/mod.rs; a
    // grant made here would be a way around that.
    if capability == crate::apps::CAP {
        return Err("denied: actions in the owner's apps are asked about one by one, when they happen".into());
    }
    let ask = ApprovalRequest::new(
        capability,
        scope,
        reason,
        format!("grant {capability} for {scope}"),
    );
    let appr = shared.ask_scoped(capability, scope, ask).await;
    if appr.allow {
        Ok(json!({"granted": true, "until_unix": until_unix(appr)}))
    } else {
        // connect.md: a refusal is the error "denied: ...", not granted:false.
        Err("denied: the owner did not allow it".into())
    }
}
