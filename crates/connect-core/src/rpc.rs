//! `rpc` streams: sequential request/response calls (connect.md).
//!
//! Methods: files.list / files.stat / files.read / files.write,
//! cookies.get, permission.request, device.info. One stream serves any
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

async fn cua_version(shared: &Shared) -> String {
    crate::cua::detect(&shared.cua_bin, &shared.cua_env)
        .await
        .version
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

    use crate::settings::CookiesMode;
    let need_ask = match s.cookies.mode {
        CookiesMode::Auto => false,
        CookiesMode::AskNew => !shared.grants().allows("cookies", &domain),
        CookiesMode::Ask => true,
    };
    if need_ask {
        let ask = ApprovalRequest::new(
            "cookies",
            &domain,
            reason,
            format!("hand the synth your browser cookies for {domain}"),
        );
        let appr = shared.ask_scoped("cookies", &domain, ask).await;
        if !appr.allow {
            shared
                .journal()
                .record(Entry::new("cookies", "cookies.get", domain.clone(), "denied: owner"));
            return Err(format!("denied: the owner did not allow cookies for {domain}"));
        }
    }

    // Read on a blocking pool: rusqlite + file copy are blocking.
    let source = shared.cookies.clone();
    let dom = domain.clone();
    let read = tokio::task::spawn_blocking(move || source.get(&dom))
        .await
        .map_err(|e| format!("cookies.get: join: {e}"))?;
    match read {
        Ok(r) => {
            // Journal: domain, count, browser. NEVER a value.
            shared.journal().record(Entry::new(
                "cookies",
                "cookies.get",
                format!("{domain} ({} from {})", r.cookies.len(), r.source),
                "ok",
            ));
            Ok(json!({"cookies": r.cookies, "source": r.source}))
        }
        Err(crate::cookies::CookieError::Unavailable(m)) => Err(format!("unavailable: {m}")),
        Err(crate::cookies::CookieError::NotFound) => {
            Ok(json!({"cookies": [], "source": ""}))
        }
        Err(crate::cookies::CookieError::Failed(m)) => Err(format!("cookies.get: {m}")),
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
