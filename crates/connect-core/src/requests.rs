//! Requests the app makes to the synth, over the control stream.
//!
//! Protocol v1 let only the synth ask (it opens rpc streams); the app's
//! control stream carried hello / presence / settings / unpair. This is
//! the small addition that lets the app ask too, without a new stream:
//!
//! ```text
//! app   -> synth  {"type":"request","id":7,"method":"vault.pair_code","params":{}}
//! synth -> app    {"type":"response","id":7,"result":{...}}
//! synth -> app    {"type":"response","id":7,"error":"unavailable: ..."}
//! ```
//!
//! A synth that answers requests says so in its hello_ack
//! (`"features":["requests", ...]`). An older synth ignores the unknown
//! message type, so without the feature the app does not send at all and
//! the caller gets a labeled `unavailable` at once instead of a timeout.
//!
//! The answer is the synth's word. Callers validate what they take from
//! it like any other input.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

/// The hello_ack feature that turns requests on.
pub const FEATURE: &str = "requests";

/// How long a request may take. The synth's own calls to the hub are
/// bounded at 20 s.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// At most this many requests wait for an answer at once.
const MAX_PENDING: usize = 16;

type Answer = Result<Value, String>;

#[derive(Default)]
struct State {
    /// The control stream's writer takes requests from here. None while
    /// no link is up.
    out: Option<mpsc::UnboundedSender<Value>>,
    /// Which control stream `out` belongs to. A stream that ended late
    /// must not take down the requests of the one that replaced it.
    generation: u64,
    /// The synth of this link said it answers requests.
    supported: bool,
    pending: HashMap<u64, oneshot::Sender<Answer>>,
}

#[derive(Default)]
pub struct LinkRequests {
    next: AtomicU64,
    state: Mutex<State>,
}

impl LinkRequests {
    /// A control stream is up: its writer sends what arrives here.
    /// Returns the queue and the stream's generation (for `detach`).
    pub(crate) fn attach(&self) -> (mpsc::UnboundedReceiver<Value>, u64) {
        let (tx, rx) = mpsc::unbounded_channel();
        let old = {
            let mut st = self.state.lock().unwrap();
            st.out = Some(tx);
            st.supported = false;
            st.generation += 1;
            (std::mem::take(&mut st.pending), st.generation)
        };
        for (_, waiting) in old.0 {
            let _ = waiting.send(Err("unavailable: the link to the synth dropped".into()));
        }
        (rx, old.1)
    }

    /// The synth's hello_ack arrived: does it answer requests?
    pub(crate) fn set_supported(&self, supported: bool) {
        self.state.lock().unwrap().supported = supported;
    }

    /// That control stream is gone: every waiting request fails now.
    /// Nothing happens when a newer stream has attached already.
    pub(crate) fn detach(&self, generation: u64) {
        let pending = {
            let mut st = self.state.lock().unwrap();
            if st.generation != generation {
                return;
            }
            st.out = None;
            st.supported = false;
            std::mem::take(&mut st.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(Err("unavailable: the link to the synth dropped".into()));
        }
    }

    /// A `response` message from the synth.
    pub(crate) fn resolve(&self, msg: &Value) {
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(tx) = self.state.lock().unwrap().pending.remove(&id) else {
            return; // late, or never ours
        };
        let answer = match msg.get("error").and_then(Value::as_str) {
            // A synth may put a code in `error` and the words in `reason`
            // (the island's requests): "<code>: <reason>".
            Some(e) if !e.is_empty() => match msg.get("reason").and_then(Value::as_str).filter(|r| !r.trim().is_empty()) {
                Some(reason) => Err(clean_error(&format!("{e}: {reason}"))),
                None => Err(clean_error(e)),
            },
            _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(answer);
    }

    /// Ask the synth. Errors are labeled like every refusal on the link
    /// (`unavailable: ...`, `denied: ...`, or the synth's own words).
    pub async fn call(&self, method: &str, params: Value) -> Answer {
        self.call_within(method, params, TIMEOUT).await
    }

    pub async fn call_within(&self, method: &str, params: Value, timeout: Duration) -> Answer {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let rx = {
            let mut st = self.state.lock().unwrap();
            let Some(out) = st.out.clone() else {
                return Err("unavailable: not linked to the synth right now".into());
            };
            if !st.supported {
                return Err("unavailable: the synth does not answer this yet (it needs an update)".into());
            }
            if st.pending.len() >= MAX_PENDING {
                return Err("unavailable: too many requests are waiting for the synth".into());
            }
            let (tx, rx) = oneshot::channel();
            st.pending.insert(id, tx);
            let msg = json!({"type": "request", "id": id, "method": method, "params": params});
            if out.send(msg).is_err() {
                st.pending.remove(&id);
                return Err("unavailable: not linked to the synth right now".into());
            }
            rx
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("unavailable: the link to the synth dropped".into()),
            Err(_) => {
                self.state.lock().unwrap().pending.remove(&id);
                Err("unavailable: the synth did not answer in time".into())
            }
        }
    }
}

/// The synth's error text, bounded and on one line.
fn clean_error(e: &str) -> String {
    let mut out = String::new();
    for c in e.chars() {
        if out.len() >= 300 {
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nothing_is_sent_without_a_link_or_to_an_old_synth() {
        let r = LinkRequests::default();
        let e = r.call("vault.list", json!({})).await.unwrap_err();
        assert!(e.starts_with("unavailable: not linked"), "{e}");

        let (mut rx, _) = r.attach();
        let e = r.call("vault.list", json!({})).await.unwrap_err();
        assert!(e.contains("needs an update"), "{e}");
        assert!(rx.try_recv().is_err(), "an old synth is sent nothing");
    }

    #[tokio::test]
    async fn a_request_gets_its_own_answer() {
        let r = std::sync::Arc::new(LinkRequests::default());
        let (mut rx, _) = r.attach();
        r.set_supported(true);

        let a = tokio::spawn({
            let r = r.clone();
            async move { r.call("vault.pair_code", json!({"browser": "chrome"})).await }
        });
        let first = rx.recv().await.unwrap();
        assert_eq!(first["type"], "request");
        assert_eq!(first["method"], "vault.pair_code");
        assert_eq!(first["params"]["browser"], "chrome");
        let b = tokio::spawn({
            let r = r.clone();
            async move { r.call("vault.list", json!({})).await }
        });
        let second = rx.recv().await.unwrap();
        assert_ne!(first["id"], second["id"]);

        // Answered out of order, and one unknown id in between.
        r.resolve(&json!({"type": "response", "id": 99999, "result": {"x": 1}}));
        r.resolve(&json!({"type": "response", "id": second["id"], "error": "unavailable: the hub\nis down"}));
        r.resolve(&json!({"type": "response", "id": first["id"], "result": {"code": "K7QW3MHX"}}));
        assert_eq!(a.await.unwrap().unwrap()["code"], "K7QW3MHX");
        assert_eq!(b.await.unwrap().unwrap_err(), "unavailable: the hub is down");

        // A code and its words.
        let c = tokio::spawn({
            let r = r.clone();
            async move { r.call("owner.message", json!({})).await }
        });
        let third = rx.recv().await.unwrap();
        r.resolve(&json!({"type": "response", "id": third["id"], "error": "busy", "reason": "the last message is still running"}));
        assert_eq!(c.await.unwrap().unwrap_err(), "busy: the last message is still running");
    }

    #[tokio::test]
    async fn a_dropped_link_fails_what_was_waiting() {
        let r = std::sync::Arc::new(LinkRequests::default());
        let (mut rx, generation) = r.attach();
        r.set_supported(true);
        let waiting = tokio::spawn({
            let r = r.clone();
            async move { r.call("vault.list", json!({})).await }
        });
        rx.recv().await.unwrap();
        r.detach(generation);
        let e = waiting.await.unwrap().unwrap_err();
        assert!(e.contains("dropped"), "{e}");
        // And the next one is refused at once.
        assert!(r.call("vault.list", json!({})).await.unwrap_err().contains("not linked"));
    }

    #[tokio::test]
    async fn an_old_stream_ending_late_does_not_cut_the_new_one() {
        let r = LinkRequests::default();
        let (_old_rx, old) = r.attach();
        let (mut rx, _new) = r.attach();
        r.set_supported(true);
        r.detach(old);
        let r = std::sync::Arc::new(r);
        let call = tokio::spawn({
            let r = r.clone();
            async move { r.call("vault.list", json!({})).await }
        });
        let sent = rx.recv().await.expect("the new stream still carries requests");
        r.resolve(&json!({"type": "response", "id": sent["id"], "result": {}}));
        assert!(call.await.unwrap().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn no_answer_is_a_labeled_timeout() {
        let r = LinkRequests::default();
        let (_rx, _) = r.attach();
        r.set_supported(true);
        let e = r.call_within("vault.list", json!({}), Duration::from_secs(5)).await.unwrap_err();
        assert!(e.contains("did not answer in time"), "{e}");
        assert!(r.state.lock().unwrap().pending.is_empty());
    }
}
