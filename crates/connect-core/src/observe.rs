//! Observe: what this computer's sources see, reported to the synth for
//! the owner's User Context (connect.md "Observe: User Context reports",
//! UC source contract P2).
//!
//! Three sources, each switched on by the owner per synth and off by
//! default: a call running (`device.call`: yes or no and the app, never
//! audio), the app in front with its window title and a category
//! (`device.activity`, password managers and private windows as
//! `private`), and the named place (`place.current`: the owner's own name
//! for the Wi-Fi network; the network's name never leaves the computer).
//! The decisions live in `sources`; the shell reads the system and hands
//! the current value of each source to `Core::observe_report`.
//!
//! The core holds:
//!
//!  - the owner's choices (`ObserveSettings`, in settings.json next to the
//!    policy but outside it, like the island: no preset touches them and
//!    the synth is told nothing of them): which source goes to which
//!    synth, the apps the owner excluded, the names of the networks;
//!  - per link, what the synth offers (`hello_ack` `features` with
//!    `observe`, `observe_resources`, and the descriptors from the
//!    `observe.resources` request, asked at every link and every 10
//!    minutes), in `LinkSlot::observe`;
//!  - the current value of each source (`Hub`), and per link a task
//!    (`run_link`) that sends it: right after hello_ack (the synth forgets
//!    every report of a link that drops), on every change, the unchanged
//!    value again every 60 s, `value: null` once a source the synth had a
//!    value of is switched off or has nothing any more, nothing at all
//!    while the app is paused (the synth drops a paused computer's
//!    reports itself).
//!
//! Additive: a synth without the feature is sent nothing and asked
//! nothing; an older app sends nothing.

pub mod sources;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::core::{Core, CoreEvent, LinkState, Shared};
use crate::slot::LinkSlot;

/// The hello_ack feature of a synth that reads `observe` messages and
/// answers `observe.resources`.
pub const FEATURE: &str = "observe";
pub const CALL: &str = "device.call";
pub const ACTIVITY: &str = "device.activity";
pub const PLACE: &str = "place.current";
/// The resources this app has a source for, in the order they are shown.
pub const SOURCES: [&str; 3] = [CALL, ACTIVITY, PLACE];

/// The unchanged value goes again this often while a source is on.
pub const REPEAT_EVERY: Duration = Duration::from_secs(60);
/// The synth is asked for its descriptors this often (and at every link).
pub const RESOURCES_EVERY: Duration = Duration::from_secs(600);
/// A value may be this long as JSON when the descriptor says nothing.
pub const DEFAULT_MAX_BYTES: usize = 4096;

/// The schema's limits of the P2 items (the descriptor's `maxLength` wins
/// when it is smaller).
pub const MAX_APP_BYTES: usize = 64;
pub const MAX_TITLE_BYTES: usize = 200;
/// The relay keeps a place name to this many bytes.
pub const MAX_PLACE_BYTES: usize = 128;
/// The app label of an excluded app (and its category).
pub const PRIVATE: &str = "private";

/// The descriptor words shown in the UI are cut to this many characters.
const MAX_WORDS_CHARS: usize = 600;

// --- clean strings ---

/// A Unicode format character (general category Cf): bidi overrides and
/// isolates, zero-width characters, the byte order mark, invisible
/// operators, tags. The relay drops them (the context service refuses
/// them); so does the app, before they leave.
fn format_char(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// A string as the synth's relay wants it: `<`, `>`, control and format
/// characters dropped, every run of whitespace one space, trimmed, cut to
/// `max_bytes` of UTF-8 on a character boundary.
pub fn clean(s: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_bytes));
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c == '<' || c == '>' || c.is_control() || format_char(c) {
            continue;
        }
        let need = c.len_utf8() + usize::from(space);
        if out.len() + need > max_bytes {
            break;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// RFC 3339 in UTC, as `observed_at` and `since` go.
pub fn rfc3339(unix: i64) -> String {
    crate::apps::time::rfc3339_utc(unix)
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- the owner's choices ---

/// An app the owner excluded: its id (macOS bundle id, Windows file name)
/// and the name shown for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMark {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A Wi-Fi network the owner named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedNetwork {
    /// The network's name (SSID). Never sent.
    pub ssid: String,
    /// The owner's name for the place ("дом", "зал", "у Миши").
    pub name: String,
}

/// What the owner chose (settings.json `observe`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveSettings {
    /// Per synth id, the sources that go to it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub on: BTreeMap<String, BTreeSet<String>>,
    /// The owner's own exclusions (the password managers and private
    /// windows are always excluded, `sources::excluded`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<AppMark>,
    /// The named networks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<NamedNetwork>,
}

impl ObserveSettings {
    pub fn is_empty(&self) -> bool {
        self.on.is_empty() && self.excluded.is_empty() && self.places.is_empty()
    }

    pub fn is_on(&self, synth_id: &str, resource: &str) -> bool {
        self.on.get(synth_id).is_some_and(|s| s.contains(resource))
    }

    /// Switch one source for one synth. Only the sources this app has.
    pub fn set_on(&mut self, synth_id: &str, resource: &str, on: bool) -> Result<(), String> {
        if !SOURCES.contains(&resource) {
            return Err("unknown_resource".into());
        }
        if on {
            self.on.entry(synth_id.to_string()).or_default().insert(resource.to_string());
        } else if let Some(set) = self.on.get_mut(synth_id) {
            set.remove(resource);
            if set.is_empty() {
                self.on.remove(synth_id);
            }
        }
        Ok(())
    }

    /// The sources some of these synths have on.
    pub fn wanted(&self, synth_ids: &[String]) -> BTreeSet<String> {
        synth_ids.iter().filter_map(|id| self.on.get(id)).flatten().cloned().collect()
    }

    /// Exclude an app (by id; by name when it has no id). False when it
    /// is excluded already or names nothing.
    pub fn exclude(&mut self, id: &str, name: &str) -> bool {
        let mark = AppMark { id: id.trim().to_string(), name: clean(name, MAX_APP_BYTES) };
        if mark.id.is_empty() && mark.name.is_empty() {
            return false;
        }
        if self.excluded.iter().any(|m| same_app(m, &mark)) {
            return false;
        }
        self.excluded.push(mark);
        true
    }

    /// Take an app off the owner's exclusions (by id, or by name for a
    /// mark without an id).
    pub fn unexclude(&mut self, id: &str, name: &str) -> bool {
        let mark = AppMark { id: id.trim().to_string(), name: name.trim().to_string() };
        let before = self.excluded.len();
        self.excluded.retain(|m| !same_app(m, &mark));
        before != self.excluded.len()
    }

    /// Name a network (again: renames it). An empty name forgets it. Err
    /// "bad_network" for an empty network name.
    pub fn name_network(&mut self, ssid: &str, name: &str) -> Result<(), String> {
        if ssid.is_empty() || ssid.len() > 32 * 4 {
            return Err("bad_network".into());
        }
        let name = clean(name, MAX_PLACE_BYTES);
        if name.is_empty() {
            self.forget_network(ssid);
            return Ok(());
        }
        match self.places.iter_mut().find(|p| p.ssid == ssid) {
            Some(p) => p.name = name,
            None => self.places.push(NamedNetwork { ssid: ssid.to_string(), name }),
        }
        Ok(())
    }

    pub fn forget_network(&mut self, ssid: &str) -> bool {
        let before = self.places.len();
        self.places.retain(|p| p.ssid != ssid);
        before != self.places.len()
    }

    /// A file edited by hand: unknown sources and empty entries dropped,
    /// nothing twice.
    pub fn normalize(&mut self) {
        for set in self.on.values_mut() {
            set.retain(|r| SOURCES.contains(&r.as_str()));
        }
        self.on.retain(|id, set| !id.is_empty() && !set.is_empty());
        let mut marks: Vec<AppMark> = Vec::new();
        for m in std::mem::take(&mut self.excluded) {
            let m = AppMark { id: m.id.trim().to_string(), name: clean(&m.name, MAX_APP_BYTES) };
            if (m.id.is_empty() && m.name.is_empty()) || marks.iter().any(|x| same_app(x, &m)) {
                continue;
            }
            marks.push(m);
        }
        self.excluded = marks;
        let mut places: Vec<NamedNetwork> = Vec::new();
        for p in std::mem::take(&mut self.places) {
            let name = clean(&p.name, MAX_PLACE_BYTES);
            if p.ssid.is_empty() || name.is_empty() || places.iter().any(|x| x.ssid == p.ssid) {
                continue;
            }
            places.push(NamedNetwork { ssid: p.ssid, name });
        }
        self.places = places;
    }
}

fn same_app(a: &AppMark, b: &AppMark) -> bool {
    if !a.id.is_empty() || !b.id.is_empty() {
        a.id.eq_ignore_ascii_case(&b.id)
    } else {
        a.name.to_lowercase() == b.name.to_lowercase()
    }
}

// --- what the synth offers ---

/// Words in the two languages of the descriptors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Words {
    pub en: String,
    pub ru: String,
}

impl Words {
    fn of(v: Option<&Value>) -> Words {
        let pick = |k: &str| {
            let s = v.and_then(|v| v.get(k)).and_then(Value::as_str).unwrap_or("");
            let mut out = clean(s, MAX_WORDS_CHARS * 4);
            if let Some((cut, _)) = out.char_indices().nth(MAX_WORDS_CHARS) {
                out.truncate(cut);
            }
            out
        };
        Words { en: pick("en"), ru: pick("ru") }
    }
}

/// One resource as the synth describes it (the context service's
/// descriptor). Only what the app uses; unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Descriptor {
    pub resource: String,
    pub revision: u64,
    /// The switch's label.
    pub title: Words,
    pub description: Words,
    /// What the owner reads when switching the source on (empty when the
    /// descriptor has none: the app's own words then).
    pub consent: Words,
    pub privacy_class: String,
    /// The longest value as JSON (0: not said, `DEFAULT_MAX_BYTES`).
    pub max_bytes: usize,
    /// `maxLength` of the item's string keys.
    #[serde(skip)]
    pub max_len: BTreeMap<String, usize>,
}

impl Descriptor {
    pub fn parse(v: &Value) -> Option<Descriptor> {
        let resource = v.get("resource").and_then(Value::as_str)?.trim().to_string();
        if resource.is_empty() || resource.len() > 128 {
            return None;
        }
        let privacy = v.get("privacy");
        let mut max_len = BTreeMap::new();
        if let Some(props) = v.pointer("/value/properties").and_then(Value::as_object) {
            for (k, p) in props {
                if let Some(n) = p.get("maxLength").and_then(Value::as_u64) {
                    max_len.insert(k.clone(), n as usize);
                }
            }
        }
        Some(Descriptor {
            resource,
            revision: v.get("revision").and_then(Value::as_u64).unwrap_or(0),
            title: Words::of(v.get("title")),
            description: Words::of(v.get("description")),
            consent: Words::of(privacy.and_then(|p| p.get("consent"))),
            privacy_class: privacy.and_then(|p| p.get("class")).and_then(Value::as_str).map(|s| clean(s, 64)).unwrap_or_default(),
            max_bytes: v.get("max_bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
            max_len,
        })
    }
}

/// The descriptors of an `observe.resources` answer
/// (`{"resources":[...]}`); entries without a resource are skipped.
pub fn parse_resources(result: &Value) -> Vec<Descriptor> {
    let mut out: Vec<Descriptor> = Vec::new();
    for d in result.get("resources").and_then(Value::as_array).into_iter().flatten().filter_map(Descriptor::parse) {
        if !out.iter().any(|x| x.resource == d.resource) {
            out.push(d);
        }
    }
    out
}

/// What one synth offers right now (`LinkSlot::observe`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkObserve {
    /// The synth announced `observe` in its last hello_ack.
    pub feature: bool,
    /// The resources it may relay now (`observe_resources`, then the
    /// resources of the last `observe.resources` answer).
    pub offered: Vec<String>,
    /// The descriptors of the last answer (kept while the link is down).
    pub descriptors: Vec<Descriptor>,
}

/// `hello_ack` arrived: what it says about observe. The descriptors of an
/// earlier answer stay until the next one.
pub(crate) fn note_ack(slot: &LinkSlot, ack: &crate::wire::HelloAck) {
    let mut o = slot.observe.lock().unwrap();
    o.feature = ack.has(FEATURE);
    o.offered = if o.feature { names(&ack.observe_resources) } else { Vec::new() };
}

fn names(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in list.iter().map(|r| r.trim()).filter(|r| !r.is_empty() && r.len() <= 128) {
        if !out.iter().any(|x| x == r) {
            out.push(r.to_string());
        }
    }
    out
}

// --- the current values ---

/// The current value of one source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub value: Value,
    /// When the source last read it (unix seconds).
    pub observed_at: i64,
}

/// The current value of every source that has one, and a counter that
/// moves whenever a value or the owner's choices change (the link tasks
/// watch it).
pub struct Hub {
    values: Mutex<BTreeMap<String, Report>>,
    changed: watch::Sender<u64>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub { values: Mutex::new(BTreeMap::new()), changed: watch::channel(0).0 }
    }
}

impl Hub {
    /// A source's reading. Its time is always kept; the link tasks hear of
    /// it only when the value changed.
    pub fn report(&self, resource: &str, value: Option<Value>, now: i64) {
        let changed = {
            let mut values = self.values.lock().unwrap();
            match value {
                None => values.remove(resource).is_some(),
                Some(v) => match values.get_mut(resource) {
                    Some(r) if r.value == v => {
                        r.observed_at = now;
                        false
                    }
                    _ => {
                        values.insert(resource.to_string(), Report { value: v, observed_at: now });
                        true
                    }
                },
            }
        };
        if changed {
            self.bump();
        }
    }

    pub fn values(&self) -> BTreeMap<String, Report> {
        self.values.lock().unwrap().clone()
    }

    pub(crate) fn bump(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

// --- what goes to one synth ---

/// What a link sent last per resource (`Some(v)`: this value, `None`:
/// null), so a change, a switch-off and a repeat are told apart.
pub type Sent = BTreeMap<String, Option<Value>>;

/// One `observe` message. `value` None = this computer's report cleared.
pub fn message(resource: &str, value: Option<&Value>, observed_at: i64) -> Value {
    match value {
        Some(v) => json!({"type": "observe", "resource": resource, "value": v, "observed_at": rfc3339(observed_at)}),
        None => json!({"type": "observe", "resource": resource, "value": null}),
    }
}

/// A value fitted to the synth's descriptor: string keys cut to its
/// `maxLength` where that is shorter than the app's own limits, and the
/// whole within `max_bytes` (None when it cannot be).
pub fn fit(resource: &str, value: &Value, d: Option<&Descriptor>) -> Option<Value> {
    let mut v = value.clone();
    if let (Some(d), Some(obj)) = (d, v.as_object_mut()) {
        for (k, x) in obj.iter_mut() {
            if let (Some(n), Some(s)) = (d.max_len.get(k), x.as_str()) {
                if s.len() > *n {
                    *x = json!(clean(s, *n));
                }
            }
        }
    }
    if !v.is_object() {
        return None;
    }
    let max = d.map(|d| d.max_bytes).filter(|n| *n > 0).unwrap_or(DEFAULT_MAX_BYTES);
    let len = serde_json::to_vec(&v).map(|b| b.len()).unwrap_or(usize::MAX);
    if len > max {
        crate::logln!("observe: {resource}: the value is {len} bytes, the synth takes {max}; not sent");
        return None;
    }
    Some(v)
}

/// What one link sends now. `full`: the link just came up, the app was
/// resumed, or the minute is up (every current value goes again); else
/// only what changed. Paused: nothing, and `sent` stays as it was.
///
/// A source goes only to a synth that offers its resource now and that
/// the owner switched it on for. `null` goes once, when a source the
/// synth has a value of is switched off or has nothing to say any more.
pub fn plan(offered: &[String], on: &BTreeSet<String>, values: &BTreeMap<String, Report>, descriptors: &[Descriptor], sent: &mut Sent, full: bool, paused: bool) -> Vec<Value> {
    if paused {
        return Vec::new();
    }
    let mut out = Vec::new();
    for r in SOURCES {
        if !offered.iter().any(|o| o == r) {
            // The synth would drop it; it forgot the old one anyway.
            sent.remove(r);
            continue;
        }
        let d = descriptors.iter().find(|d| d.resource == r);
        let current = if on.contains(r) {
            values.get(r).and_then(|rep| fit(r, &rep.value, d).map(|v| (v, rep.observed_at)))
        } else {
            None
        };
        match current {
            Some((v, at)) => {
                if full || sent.get(r) != Some(&Some(v.clone())) {
                    out.push(message(r, Some(&v), at));
                    sent.insert(r.to_string(), Some(v));
                }
            }
            None => {
                if matches!(sent.get(r), Some(Some(_))) {
                    out.push(message(r, None, 0));
                    sent.insert(r.to_string(), None);
                }
            }
        }
    }
    out
}

/// Ask the synth for its descriptors and keep them with the link. The
/// resources of the answer are what it offers now.
pub(crate) async fn refresh(shared: &Arc<Shared>, slot: &Arc<LinkSlot>) -> Result<(), String> {
    let answer = slot.requests.call("observe.resources", json!({})).await?;
    let descriptors = parse_resources(&answer);
    {
        let mut o = slot.observe.lock().unwrap();
        o.offered = descriptors.iter().map(|d| d.resource.clone()).collect();
        o.descriptors = descriptors;
    }
    shared.observe.bump();
    shared.emit(CoreEvent::Observe { synth_id: slot.synth_id.clone() });
    Ok(())
}

/// The sending of one link, from its hello_ack until the control stream
/// ends (`out` is the stream's writer queue: it closes with the stream).
pub(crate) async fn run_link(shared: Arc<Shared>, slot: Arc<LinkSlot>, out: mpsc::UnboundedSender<Value>) {
    let mut changed = shared.observe.subscribe();
    let mut control = shared.control_tx.subscribe();
    let mut sent = Sent::new();
    let ask = {
        let (shared, slot) = (shared.clone(), slot.clone());
        move || {
            let (shared, slot) = (shared.clone(), slot.clone());
            tokio::spawn(async move {
                if let Err(e) = refresh(&shared, &slot).await {
                    crate::logln!("link {}: observe.resources: {e}", slot.synth_id);
                }
            });
        }
    };
    let flush = |sent: &mut Sent, full: bool| -> bool {
        let (offered, descriptors) = {
            let o = slot.observe.lock().unwrap();
            (o.offered.clone(), o.descriptors.clone())
        };
        let on = shared.persisted.read().unwrap().observe.on.get(&slot.synth_id).cloned().unwrap_or_default();
        let msgs = plan(&offered, &on, &shared.observe.values(), &descriptors, sent, full, shared.paused());
        msgs.into_iter().all(|m| out.send(m).is_ok())
    };
    ask();
    if !flush(&mut sent, true) {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut repeat = tokio::time::interval_at(start + REPEAT_EVERY, REPEAT_EVERY);
    repeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut again = tokio::time::interval_at(start + RESOURCES_EVERY, RESOURCES_EVERY);
    again.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let full = tokio::select! {
            _ = out.closed() => return,
            r = changed.changed() => {
                if r.is_err() {
                    return;
                }
                false
            }
            r = control.changed() => {
                if r.is_err() {
                    return;
                }
                // Resumed: everything again at once.
                true
            }
            _ = repeat.tick() => true,
            _ = again.tick() => {
                ask();
                continue;
            }
        };
        if !flush(&mut sent, full) {
            return;
        }
    }
}

// --- the UI ---

/// One link on the "what the synth sees" page.
#[derive(Debug, Clone, Serialize)]
pub struct ObserveLinkView {
    pub synth_id: String,
    pub synth_name: Option<String>,
    pub linked: bool,
    #[serde(flatten)]
    pub observe: LinkObserve,
    /// The sources the owner switched on for this synth.
    pub on: Vec<String>,
}

/// The page's data.
#[derive(Debug, Clone, Serialize)]
pub struct ObserveView {
    pub links: Vec<ObserveLinkView>,
    pub settings: ObserveSettings,
    /// What each source would send now ("now sending").
    pub values: BTreeMap<String, Report>,
}

impl Core {
    pub fn observe_settings(&self) -> ObserveSettings {
        self.shared().persisted.read().unwrap().observe.clone()
    }

    /// Change the owner's choices. The synths hear only of what follows
    /// from them (a value, or null for a source switched off).
    pub fn set_observe(&self, f: impl FnOnce(&mut ObserveSettings)) -> ObserveSettings {
        let shared = self.shared();
        let now = {
            let mut p = shared.persisted.write().unwrap();
            f(&mut p.observe);
            p.observe.normalize();
            shared.save_persisted(&p);
            p.observe.clone()
        };
        shared.observe.bump();
        shared.emit(CoreEvent::Observe { synth_id: String::new() });
        now
    }

    /// A source's current value (None: nothing to report now). The shell
    /// calls this every round; the links hear only of a change.
    pub fn observe_report(&self, resource: &str, value: Option<Value>) {
        self.shared().observe.report(resource, value, now_unix());
    }

    /// The sources some linked synth has switched on: the ones the shell
    /// reads (none of them while the app is paused: the shell checks
    /// `paused` and keeps the last values, so a resume sends no null).
    pub fn observe_wanted(&self) -> BTreeSet<String> {
        let shared = self.shared();
        let ids: Vec<String> = shared.slots().iter().map(|s| s.synth_id.clone()).collect();
        let wanted = shared.persisted.read().unwrap().observe.wanted(&ids);
        wanted
    }

    pub fn observe_view(&self) -> ObserveView {
        let shared = self.shared();
        let settings = self.observe_settings();
        let links = shared
            .slots()
            .iter()
            .filter(|s| !s.synth_id.is_empty())
            .map(|s| ObserveLinkView {
                synth_id: s.synth_id.clone(),
                synth_name: Some(s.display_name()).filter(|n| !n.is_empty()),
                linked: s.state() == LinkState::Linked,
                observe: s.observe.lock().unwrap().clone(),
                on: settings.on.get(&s.synth_id).map(|set| set.iter().cloned().collect()).unwrap_or_default(),
            })
            .collect();
        ObserveView { links, values: shared.observe.values(), settings }
    }

    /// Ask a linked synth for its descriptors now (the page opened).
    pub async fn observe_refresh(&self, synth_id: &str) -> Result<(), String> {
        let shared = self.shared();
        let slot = shared.slot(synth_id).ok_or("unknown_synth")?;
        if !slot.observe.lock().unwrap().feature {
            return Err("unavailable: the synth does not take observe reports".into());
        }
        refresh(&shared, &slot).await
    }
}
