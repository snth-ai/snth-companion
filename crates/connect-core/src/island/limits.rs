//! The synth's subscription limits on the island (TZ tz-island-limits.md):
//! how much of the main model's subscription (Claude Max, ChatGPT Plus...)
//! the 5-hour and the weekly windows have used, and when they reset.
//!
//! A synth whose main runtime is on a subscription announces the hello_ack
//! feature `island_limits` and pushes `island.event kind=limits` (no
//! message_id, like `face`):
//!
//! ```json
//! {"kind":"limits","limits":{
//!    "provider":"claude","plan":"Claude Max",
//!    "windows":[{"id":"five_hour","label_ru":"5ч","label_en":"5h",
//!                "used_pct":34,"resets_at":"2026-10-28T10:20:00Z"}],
//!    "blocked":false,"fetched_at":"2026-10-28T08:01:02Z"}}
//! ```
//!
//! `limits: null` (or no `limits` at all) = nothing to show. The parse is
//! defensive: unknown fields are ignored, a window without a usable
//! `used_pct` is dropped, missing strings are empty, text is clipped. The
//! last snapshot is kept per synth in memory only, and dropped when the
//! link goes down (the synth sends its last one again right after the
//! next hello_ack). Times stay as the synth wrote them (ISO 8601): the
//! island reads them in the computer's own time zone.

use serde::Serialize;
use serde_json::Value;

use crate::core::{CoreEvent, Shared};
use crate::slot::LinkSlot;

/// The hello_ack feature of a synth that pushes its subscription limits.
pub const FEATURE_LIMITS: &str = "island_limits";

/// At most this many windows are kept (the synth sends two or three).
const MAX_WINDOWS: usize = 6;

/// One window of a subscription limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitWindow {
    /// "five_hour", "seven_day", "seven_day_opus"... (the synth's).
    pub id: String,
    /// The synth's words for it ("5ч", "5h"); either may be empty.
    pub label_ru: String,
    pub label_en: String,
    /// Used, 0..=100 (clamped; may be fractional, the island rounds).
    pub used_pct: f64,
    /// When the window resets, ISO 8601 as sent ("" = unknown).
    pub resets_at: String,
}

/// The last snapshot of a synth's subscription limits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limits {
    /// "claude", "codex" (the synth's; may be something else later).
    pub provider: String,
    /// "Claude Max", "ChatGPT Plus"... ("" = the island names the provider).
    pub plan: String,
    pub windows: Vec<LimitWindow>,
    /// The provider refuses the subscription right now.
    pub blocked: bool,
    /// When the synth read it, ISO 8601 as sent ("" = unknown).
    pub fetched_at: String,
    /// When this computer received it (unix ms): the age when the synth
    /// did not say `fetched_at`.
    pub received_ms: u64,
}

impl Limits {
    /// The same numbers (the receipt time aside): no news for the island.
    fn same_as(&self, other: &Limits) -> bool {
        Limits { received_ms: 0, ..self.clone() } == Limits { received_ms: 0, ..other.clone() }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A string field, clipped (anything else = "").
fn text(v: &Value, key: &str, max: usize) -> String {
    v.get(key).and_then(Value::as_str).map(|s| super::clip(s, max)).unwrap_or_default()
}

/// `params.limits` -> the snapshot. Ok(None): nothing to show (null or
/// absent). Err: not an object at all (the event is refused, the last
/// snapshot stays).
pub fn parse(params: &Value) -> Result<Option<Limits>, String> {
    let l = match params.get("limits") {
        None | Some(Value::Null) => return Ok(None),
        Some(l) if l.is_object() => l,
        Some(_) => return Err("bad request: params.limits must be an object or null".into()),
    };
    let windows = l
        .get("windows")
        .and_then(Value::as_array)
        .map(|ws| {
            ws.iter()
                .filter_map(|w| {
                    let pct = w.get("used_pct").and_then(Value::as_f64).filter(|p| p.is_finite())?;
                    Some(LimitWindow {
                        id: text(w, "id", 40),
                        label_ru: text(w, "label_ru", 24),
                        label_en: text(w, "label_en", 24),
                        used_pct: pct.clamp(0.0, 100.0),
                        resets_at: text(w, "resets_at", 40),
                    })
                })
                .take(MAX_WINDOWS)
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Limits {
        provider: text(l, "provider", 24),
        plan: text(l, "plan", 40),
        windows,
        blocked: l.get("blocked").and_then(Value::as_bool).unwrap_or(false),
        fetched_at: text(l, "fetched_at", 40),
        received_ms: now_ms(),
    }))
}

/// Does this link's synth push its subscription limits?
pub fn supports_limits(slot: &LinkSlot) -> bool {
    super::supports(slot) && slot.synth.read().unwrap().as_ref().is_some_and(|ack| ack.has(FEATURE_LIMITS))
}

/// Keep (or drop, None) this synth's snapshot. Emits
/// `CoreEvent::Limits` when what the island shows changed.
pub(crate) fn store(shared: &Shared, synth_id: &str, limits: Option<Limits>) {
    if synth_id.is_empty() {
        return;
    }
    let changed = {
        let mut all = shared.island_limits.lock().unwrap();
        let changed = match (all.get(synth_id), &limits) {
            (None, None) => false,
            (Some(a), Some(b)) => !a.same_as(b),
            _ => true,
        };
        match limits {
            Some(l) => {
                all.insert(synth_id.to_string(), l);
            }
            None => {
                all.remove(synth_id);
            }
        }
        changed
    };
    if changed {
        shared.emit(CoreEvent::Limits { synth_id: synth_id.to_string() });
    }
}

/// The last snapshot of this synth (None: nothing to show).
pub fn of(shared: &Shared, synth_id: &str) -> Option<Limits> {
    shared.island_limits.lock().unwrap().get(synth_id).cloned()
}
