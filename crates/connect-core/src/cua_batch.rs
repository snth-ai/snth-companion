//! `run_steps`: several actions in one call, decided step by step.
//!
//! Cua Driver 0.35+ has `run_steps` (its old name `run_actions` still
//! dispatches, unlisted): one call that runs up to 64 action tools in
//! order inside the driver, with optional checks before and after each
//! (`wait_for`, `expect`) and one window read at the end (`observe`). The
//! driver calls it "the default way to act": the descriptions of click,
//! type_text and press_key point at it, and after a second direct action
//! in a row the driver adds a ready `run_steps` call to the result, with
//! no switch to turn that off (`batch_nudge.rs`). Refusing it would point
//! the synth at a refused tool every other action, so it is allowed, and
//! the pipe decides every part of it as the call that part stands for:
//!
//!  - every step: the tool it becomes (the driver's aliases and rewrites:
//!    `type` is type_text, `return` is press_key, `hover` is move_cursor,
//!    and so on; a name that is none of them is refused), the owner's tool
//!    denylist, the never list, the owner's look (`move_cursor` while the
//!    owner switched the cursor off), the place a file is written
//!    (`debug_image_out` is held to the shared folders and replaced by its
//!    resolved form, as on a direct call) and, while the owner limits the
//!    apps, the window it acts on;
//!  - the end `observe`: as the `get_window_state` it is (its window, a
//!    screenshot within the owner's limit, a screenshot file);
//!  - foreground delivery is the pipe's own gate (`cua::wants_foreground`
//!    sees a `delivery_mode` at any depth and `foreground_fallback`).
//!
//! While the owner limits the apps, a window is named only by pid and
//! window_id: a batch, step, check or observe that names it by app name
//! or window title (`app`, `window`) is refused, because the driver finds
//! that window only while the batch runs. The windows a step inherits are
//! the batch's own (`pid`/`window_id`) and those of the steps before it,
//! all of which are checked; a step that names no window and inherits
//! none is refused as a direct call without a window is.
//!
//! What the driver itself checks is unchanged: it runs every step through
//! the same authorization as a direct call (`batch_tools.rs`,
//! `registry.invoke`), the capability manifest of "only these" included.

use serde_json::{Map, Value};

use crate::cua_apps::{Directory, Scope, Verdict};
use crate::cua_guard::RefusalKind;
use crate::settings::Settings;

/// The batch tool.
pub const RUN_STEPS: &str = "run_steps";
/// Its old name: still dispatched by the driver, not listed. The pipe
/// forwards it under the new name, so there is one way in.
pub const RUN_ACTIONS: &str = "run_actions";

/// The tools a step may run (`batch_tools.rs` BATCHABLE_TOOLS).
pub const STEP_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "set_value",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
    "move_cursor",
];

/// The fields a step may carry besides its action (`STEP_KEYS`).
const STEP_KEYS: &[&str] = &["tool", "args", "wait_for", "expect", "timeout_ms"];

/// Key names the driver takes as a step's tool: a press_key of that key
/// (`KEY_TOOL_NAMES`, matched without case).
const KEY_TOOL_NAMES: &[&str] = &[
    "return", "enter", "tab", "escape", "esc", "space", "backspace", "up", "down", "left", "right",
    "home", "end", "pageup", "pagedown",
];

/// The tools one step name can become in the driver (`parse_action`,
/// `TOOL_ALIASES`, `rewrite_step`): press_key and hotkey can turn into
/// type_text (a symbol), hotkey into press_key (one key), `press` with an
/// element into click. Empty = not a step the driver runs.
pub fn step_tools(name: &str) -> &'static [&'static str] {
    let lower = name.to_ascii_lowercase();
    if lower == "triple_click" {
        return &["click"];
    }
    match name {
        "press" => return &["click", "press_key", "type_text"],
        "key" | "keypress" | "press_key" => return &["press_key", "type_text"],
        "hotkey" => return &["hotkey", "press_key", "type_text"],
        "type" | "type_text" => return &["type_text"],
        "set" | "set_value" => return &["set_value"],
        "hover" | "mouse_move" | "move_cursor" => return &["move_cursor"],
        "click" => return &["click"],
        "double_click" => return &["double_click"],
        "right_click" => return &["right_click"],
        "scroll" => return &["scroll"],
        "drag" => return &["drag"],
        _ => {}
    }
    if KEY_TOOL_NAMES.contains(&lower.as_str()) {
        return &["press_key", "type_text"];
    }
    &[]
}

/// A part of the batch the pipe refused: what the synth is told, the line
/// for the owner's activity list and the kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub tell: String,
    pub journal: &'static str,
    pub kind: RefusalKind,
}

fn refused(tell: String, journal: &'static str, kind: RefusalKind) -> Refused {
    Refused { tell, journal, kind }
}

/// Where a step keeps the arguments of its action.
enum Place {
    /// `{"tool": ..., "args": {...}}`
    Args,
    /// `{"tool": ..., <arguments beside tool>}`
    Flat,
    /// `{"<tool>": {...}}`
    Keyed(String),
    /// Only a check, no action.
    None,
}

/// The step's action: its name and where its arguments are. Mirrors the
/// driver's reading (`parse_step`); a step the driver would refuse to
/// read is refused here as well, before anything runs.
fn action_of(index: usize, step: &Map<String, Value>) -> Result<(Option<String>, Place), Refused> {
    let bad = |why: String| {
        refused(
            format!("bad argument: run_steps step {}: {why}", index + 1),
            "denied: a step of run_steps the app cannot read",
            RefusalKind::bad_argument("step_unreadable"),
        )
    };
    let shorthand: Vec<&String> = step
        .iter()
        .filter(|(key, _)| !STEP_KEYS.contains(&key.as_str()))
        .filter(|(_, value)| match value {
            Value::Null => false,
            Value::Object(fields) => !fields.is_empty() || step.get("tool").is_none(),
            _ => true,
        })
        .map(|(key, _)| key)
        .collect();
    let args_absent = matches!(step.get("args"), None | Some(Value::Null));
    match (step.get("tool"), shorthand.as_slice()) {
        (Some(tool), [_, ..]) if args_absent => {
            let name = tool.as_str().ok_or_else(|| bad("`tool` must be a string".into()))?;
            Ok((Some(name.to_string()), Place::Flat))
        }
        (Some(_), [extra, ..]) => Err(bad(format!("unknown field `{extra}` beside `args`"))),
        (Some(tool), []) => {
            let name = tool.as_str().ok_or_else(|| bad("`tool` must be a string".into()))?;
            Ok((Some(name.to_string()), Place::Args))
        }
        (None, [key]) => {
            if step.contains_key("args") {
                return Err(bad(format!("`{key}` already carries the arguments; drop `args`")));
            }
            Ok((Some((*key).clone()), Place::Keyed((*key).clone())))
        }
        (None, []) => {
            if step.contains_key("args") {
                return Err(bad("`args` without `tool`".into()));
            }
            Ok((None, Place::None))
        }
        (None, keys) => Err(bad(format!(
            "one action per step, got {}",
            keys.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// The arguments of a step's action, as an object (empty when absent).
fn read_args(step: &Map<String, Value>, place: &Place) -> Map<String, Value> {
    let from = |v: Option<&Value>| v.and_then(Value::as_object).cloned().unwrap_or_default();
    match place {
        Place::Args => from(step.get("args")),
        Place::Keyed(key) => from(step.get(key)),
        Place::Flat => step
            .iter()
            .filter(|(key, _)| !STEP_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        Place::None => Map::new(),
    }
}

/// Put checked arguments back where the step keeps them.
fn write_args(step: &mut Map<String, Value>, place: &Place, args: Map<String, Value>) {
    match place {
        Place::Args => {
            step.insert("args".into(), Value::Object(args));
        }
        Place::Keyed(key) => {
            step.insert(key.clone(), Value::Object(args));
        }
        Place::Flat => {
            step.retain(|key, _| STEP_KEYS.contains(&key.as_str()));
            step.extend(args);
        }
        Place::None => {}
    }
}

/// A window named by app name or title, which the driver finds only while
/// the batch runs.
fn names_window_by_name(fields: &Map<String, Value>) -> bool {
    ["app", "window"].iter().any(|key| fields.get(*key).is_some_and(|v| !v.is_null()))
}

fn by_name_refused(what: &str) -> Refused {
    refused(
        format!("denied: {what} names a window by app name or title; while the owner limits which apps you may use, name every window by pid and window_id (from list_windows), in run_steps too; the owner's setting is in SNTH Connect under Computer control"),
        "denied: run_steps named a window by name, while apps are limited",
        RefusalKind::bad_argument("window_by_name_while_limited"),
    )
}

/// The window a part acts on when it names none: (pid, window_id).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Window {
    pid: Option<i64>,
    window_id: Option<u64>,
}

impl Window {
    fn of(fields: &Map<String, Value>) -> Window {
        Window {
            pid: fields.get("pid").and_then(Value::as_i64),
            window_id: fields.get("window_id").and_then(Value::as_u64),
        }
    }
    fn is_empty(&self) -> bool {
        self.pid.is_none() && self.window_id.is_none()
    }
}

/// The call a part stands for, with the window it inherits filled in when
/// it names none. Returns the call and the keys that were filled in.
fn stand_in(tool: &str, args: &Map<String, Value>, inherited: Window) -> (Value, Vec<&'static str>) {
    let mut args = args.clone();
    let mut filled = Vec::new();
    if Window::of(&args).is_empty() {
        if let Some(pid) = inherited.pid {
            args.insert("pid".into(), Value::from(pid));
            filled.push("pid");
        }
        if let Some(window_id) = inherited.window_id {
            args.insert("window_id".into(), Value::from(window_id));
            filled.push("window_id");
        }
    }
    (
        serde_json::json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": tool, "arguments": Value::Object(args)}}),
        filled,
    )
}

/// The arguments of a stand-in call after the checks, without the window
/// keys that were only filled in for the decision.
fn checked_args(call: Value, filled: &[&str], original: &Map<String, Value>) -> Map<String, Value> {
    let mut args = call
        .pointer("/params/arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in filled {
        if !original.contains_key(*key) {
            args.remove(*key);
        }
    }
    args
}

/// One part as the call it stands for: denylist, never list, the owner's
/// apps, the owner's look, files. `call` comes back with its arguments as
/// the checks left them.
async fn check_part(
    what: &str,
    tool: &str,
    call: &mut Value,
    scope: &Scope,
    dir: &Directory,
    settings: &Settings,
) -> Result<(), Refused> {
    if settings.computer.tool_denylist.iter().any(|t| t == tool) {
        return Err(refused(
            format!("denied: {what} runs {tool:?}, which is on this computer's denylist"),
            "denied: a step on the denylist",
            RefusalKind::owner_policy("tool_denylist"),
        ));
    }
    if let Some(why) = crate::cua_guard::refusal(tool) {
        return Err(refused(why, "denied: never allowed for the synth", RefusalKind::owner_policy("never_allowed")));
    }
    match crate::cua_apps::decide(scope, tool, call, dir).await {
        Verdict::Pass => {}
        Verdict::Refuse { tell, journal, kind } => {
            return Err(refused(format!("{tell} ({what})"), journal, kind));
        }
        // Only the two lists are answered by the app, and neither is a
        // step or an observation.
        Verdict::Answer(_) => {
            return Err(refused(
                format!("bad argument: {what} cannot be answered inside run_steps; call {tool} on its own"),
                "denied: a list inside run_steps",
                RefusalKind::bad_argument("step_not_batchable"),
            ));
        }
    }
    // The same order as the pipe: files, then the owner's look.
    if let Err(why) = crate::cua_guard::confine_paths(call, &settings.files).await {
        return Err(refused(format!("{} ({what})", why.tell), "denied: outside the shared folders", why.kind));
    }
    if let crate::cua_look::Gate::Refuse(why, kind) = crate::cua_look::gate(tool, call, &settings.computer.look) {
        return Err(refused(format!("denied: {why} ({what})"), "denied: the look is the owner's to set", kind));
    }
    Ok(())
}

/// Decide a `run_steps` call part by part, and leave in `msg` the
/// arguments as the checks left them (a file path in its resolved form, a
/// screenshot within the owner's limit). Ok = forward it.
pub async fn check(msg: &mut Value, scope: &Scope, dir: &Directory, settings: &Settings) -> Result<(), Refused> {
    let Some(args) = msg.pointer_mut("/params/arguments").and_then(Value::as_object_mut) else {
        // No arguments: the driver answers that steps are required.
        return Ok(());
    };
    let limited = scope.restricted();
    if limited && names_window_by_name(args) {
        return Err(by_name_refused("run_steps"));
    }
    // The window steps inherit: the batch's own, then the last one a step
    // named.
    let mut context = Window::of(args);

    if let Some(Value::Array(steps)) = args.get_mut("steps") {
        for (index, raw) in steps.iter_mut().enumerate() {
            let Some(step) = raw.as_object_mut() else {
                // The driver refuses a step that is no object before
                // anything runs.
                continue;
            };
            let what = format!("step {} of run_steps", index + 1);
            if limited {
                for check in ["wait_for", "expect"] {
                    let named = match step.get(check) {
                        Some(Value::Object(fields)) => names_window_by_name(fields),
                        Some(Value::Array(items)) => {
                            items.iter().filter_map(Value::as_object).any(names_window_by_name)
                        }
                        _ => false,
                    };
                    if named {
                        return Err(by_name_refused(&format!("the {check} check of {what}")));
                    }
                }
            }
            let (name, place) = action_of(index, step)?;
            let Some(name) = name else {
                // Only a check: it reads the window the steps are in.
                continue;
            };
            let tools = step_tools(&name);
            if tools.is_empty() {
                return Err(refused(
                    format!(
                        "bad argument: {what}: `{name}` is not a tool a step can run; a step runs one of {}",
                        STEP_TOOLS.join(", ")
                    ),
                    "denied: a step of run_steps with an unknown tool",
                    RefusalKind::bad_argument("step_tool_unknown"),
                ));
            }
            let read = read_args(step, &place);
            let mut step_args = read.clone();
            if limited && names_window_by_name(&step_args) {
                return Err(by_name_refused(&what));
            }
            for tool in tools {
                let (mut call, filled) = stand_in(tool, &step_args, context);
                check_part(&what, tool, &mut call, scope, dir, settings).await?;
                step_args = checked_args(call, &filled, &step_args);
            }
            let own = Window::of(&step_args);
            // A step that names its whole window is the one later steps
            // inherit; one that names a part of it does so only while
            // nothing else was named.
            let whole = own.pid.is_some() && own.window_id.is_some();
            if whole || (context.is_empty() && !own.is_empty()) {
                context = own;
            }
            // Only what a check changed is written back: the step keeps its
            // own form otherwise.
            if step_args != read {
                write_args(step, &place, step_args);
            }
        }
    }

    if let Some(Value::Object(observe)) = args.get_mut("observe") {
        if limited && names_window_by_name(observe) {
            return Err(by_name_refused("the observe of run_steps"));
        }
        let original = observe.clone();
        let (mut call, filled) = stand_in("get_window_state", &original, context);
        check_part("the observe of run_steps", "get_window_state", &mut call, scope, dir, settings).await?;
        *observe = checked_args(call, &filled, &original);
    }
    Ok(())
}

/// Forward the old name under the new one (the driver runs the same
/// tool), so the pipe has one name to decide. True = renamed.
pub fn rename_old_name(msg: &mut Value) -> bool {
    let Some(name) = msg.pointer_mut("/params/name") else {
        return false;
    };
    if name.as_str() == Some(RUN_ACTIONS) {
        *name = Value::from(RUN_STEPS);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cua_apps::{AppRow, WinRow};
    use crate::settings::{AppRef, AppsPolicy};
    use serde_json::json;

    fn app(pid: i64, name: &str, bundle_id: &str) -> AppRow {
        AppRow {
            pid,
            name: name.into(),
            bundle_id: Some(bundle_id.into()),
            path: Some(format!("/Applications/{name}.app")),
            running: pid > 0,
        }
    }

    fn dir() -> Directory {
        Directory::fixed(
            vec![app(100, "Finder", "com.apple.finder"), app(200, "Safari", "com.apple.Safari"), app(300, "1Password", "com.1password.1password")],
            vec![WinRow { window_id: 11, pid: 100 }, WinRow { window_id: 21, pid: 200 }, WinRow { window_id: 31, pid: 300 }],
        )
    }

    fn scope(policy: AppsPolicy, ids: &[&str]) -> Scope {
        Scope {
            policy,
            apps: ids.iter().map(|id| AppRef { id: (*id).into(), name: (*id).into(), loose: false }).collect(),
            write_roots: vec![],
        }
    }

    fn all() -> Scope {
        scope(AppsPolicy::All, &[])
    }

    fn except() -> Scope {
        scope(AppsPolicy::Except, &["com.1password.1password"])
    }

    fn call(args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "run_steps", "arguments": args}})
    }

    fn settings() -> Settings {
        Settings::default()
    }

    async fn run(scope: &Scope, settings: &Settings, args: Value) -> (Result<(), Refused>, Value) {
        let mut msg = call(args);
        let out = check(&mut msg, scope, &dir(), settings).await;
        (out, msg)
    }

    fn code(r: &Result<(), Refused>) -> String {
        match r {
            Ok(()) => "ok".into(),
            Err(refused) => refused.kind.data()["code"].as_str().unwrap_or_default().to_string(),
        }
    }

    #[test]
    fn every_step_name_the_driver_runs_has_its_tools_and_others_have_none() {
        for tool in STEP_TOOLS {
            assert!(step_tools(tool).contains(tool), "{tool}");
        }
        assert_eq!(step_tools("hover"), ["move_cursor"]);
        assert_eq!(step_tools("Return"), ["press_key", "type_text"]);
        assert_eq!(step_tools("triple_click"), ["click"]);
        assert!(step_tools("press").contains(&"click"));
        assert!(step_tools("hotkey").contains(&"type_text"));
        for never in ["get_window_state", "launch_app", "kill_app", "run_script", "set_config", "Click", ""] {
            assert!(step_tools(never).is_empty(), "{never}");
        }
    }

    #[test]
    fn the_old_name_goes_on_under_the_new_one() {
        let mut msg = json!({"method": "tools/call", "params": {"name": "run_actions", "arguments": {}}});
        assert!(rename_old_name(&mut msg));
        assert_eq!(msg["params"]["name"], "run_steps");
        let mut other = json!({"method": "tools/call", "params": {"name": "click"}});
        assert!(!rename_old_name(&mut other));
    }

    #[tokio::test]
    async fn an_ordinary_batch_passes_in_every_form_the_driver_reads() {
        let args = json!({"pid": 200, "window_id": 21, "steps": [
            {"tool": "click", "args": {"element_token": "s1:3"}},
            {"tool": "hotkey", "keys": ["cmd", "a"]},
            {"type_text": {"text": "hello"}},
            {"expect": [{"role": "button", "name": "Send"}]},
            {"tool": "return"},
        ], "observe": {}});
        let (out, msg) = run(&except(), &settings(), args.clone()).await;
        assert_eq!(code(&out), "ok");
        // Nothing the checks did not change is changed; the window the
        // steps inherit is not written into them.
        assert_eq!(msg["params"]["arguments"]["steps"], args["steps"]);
        let (out, _) = run(&all(), &settings(), args).await;
        assert_eq!(code(&out), "ok");
    }

    #[tokio::test]
    async fn a_step_in_an_excluded_app_is_refused_whichever_way_it_gets_there() {
        // Its own window.
        let (out, _) = run(&except(), &settings(), json!({"steps": [
            {"click": {"pid": 200, "window_id": 21, "element_token": "s1:1"}},
            {"click": {"pid": 300, "window_id": 31, "element_token": "s2:1"}},
        ]})).await;
        assert_eq!(code(&out), "app_excluded", "{out:?}");
        // The batch's window.
        let (out, _) = run(&except(), &settings(), json!({"pid": 300, "window_id": 31, "steps": [{"type": {"text": "x"}}]})).await;
        assert!(out.is_err());
        // Inherited from an earlier step.
        let (out, _) = run(&except(), &settings(), json!({"steps": [
            {"click": {"pid": 300, "window_id": 31, "x": 1, "y": 1}},
            {"press_key": {"key": "return"}},
        ]})).await;
        assert!(out.is_err());
        // The end read.
        let (out, _) = run(&except(), &settings(), json!({"steps": [{"click": {"pid": 200, "window_id": 21, "x": 1, "y": 1}}], "observe": {"pid": 300, "window_id": 31}})).await;
        assert!(out.is_err());
        // A step with no window at all, while apps are limited.
        let (out, _) = run(&except(), &settings(), json!({"steps": [{"type_text": {"text": "secret"}}]})).await;
        assert_eq!(code(&out), "window_not_named");
        // With every app allowed nothing about apps is decided.
        let (out, _) = run(&all(), &settings(), json!({"steps": [{"click": {"pid": 300, "window_id": 31, "x": 1, "y": 1}}]})).await;
        assert_eq!(code(&out), "ok");
    }

    #[tokio::test]
    async fn a_window_by_name_is_refused_only_while_apps_are_limited() {
        for args in [
            json!({"app": "Safari", "steps": [{"click": {"role": "button", "name": "OK"}}]}),
            json!({"steps": [{"click": {"app": "1Password", "role": "button", "name": "OK"}}]}),
            json!({"steps": [{"tool": "click", "window": "Vault", "role": "button", "name": "OK"}]}),
            json!({"pid": 200, "window_id": 21, "steps": [{"wait_for": {"app": "1Password", "text": "code"}}]}),
            json!({"pid": 200, "window_id": 21, "steps": [{"click": {"x": 1, "y": 1}, "expect": [{"window": "Vault", "text": "x"}]}]}),
            json!({"pid": 200, "window_id": 21, "steps": [{"click": {"x": 1, "y": 1}}], "observe": {"app": "1Password"}}),
        ] {
            let (out, _) = run(&except(), &settings(), args.clone()).await;
            assert_eq!(code(&out), "window_by_name_while_limited", "{args}");
            let (out, _) = run(&all(), &settings(), args.clone()).await;
            assert_eq!(code(&out), "ok", "{args}");
        }
    }

    #[tokio::test]
    async fn the_owners_switches_hold_for_every_step() {
        let mut s = settings();
        s.computer.look.cursor = false;
        for step in [json!({"move_cursor": {"pid": 200, "window_id": 21, "x": 1, "y": 1}}), json!({"tool": "hover", "args": {"pid": 200, "window_id": 21, "x": 1, "y": 1}})] {
            let (out, _) = run(&all(), &s, json!({"steps": [step]})).await;
            assert_eq!(code(&out), "agent_cursor_off");
        }
        let mut s = settings();
        s.computer.tool_denylist = vec!["type_text".into()];
        // `press_key` of a symbol becomes type_text in the driver.
        for step in [json!({"type": {"pid": 200, "window_id": 21, "text": "x"}}), json!({"press_key": {"pid": 200, "window_id": 21, "key": "asterisk"}})] {
            let (out, _) = run(&all(), &s, json!({"steps": [step]})).await;
            assert_eq!(code(&out), "tool_denylist");
        }
        // A file written by a step is held to the shared folders.
        let (out, _) = run(&all(), &settings(), json!({"steps": [{"click": {"pid": 200, "window_id": 21, "x": 1, "y": 1, "debug_image_out": "/tmp/x.png"}}]})).await;
        assert!(code(&out).starts_with("write_arg_"), "{out:?}");
        let (out, _) = run(&all(), &settings(), json!({"steps": [{"click": {"pid": 200, "window_id": 21, "x": 1, "y": 1}}], "observe": {"screenshot_out_file": "/tmp/x.png"}})).await;
        assert!(code(&out).starts_with("write_arg_"), "{out:?}");
    }

    #[tokio::test]
    async fn the_end_read_gets_the_owners_screenshot_limit() {
        let mut s = settings();
        s.computer.look.screenshot_max = 800;
        let (out, msg) = run(&all(), &s, json!({"pid": 200, "window_id": 21, "steps": [{"click": {"x": 1, "y": 1}}], "observe": {"include_screenshot": true}})).await;
        assert_eq!(code(&out), "ok");
        let observe = &msg["params"]["arguments"]["observe"];
        assert_eq!(observe["max_image_dimension"], 800);
        // The inherited window is not written into it.
        assert!(observe.get("pid").is_none(), "{observe}");
    }

    #[tokio::test]
    async fn a_step_that_is_no_step_is_refused_before_anything_runs() {
        for (step, want) in [
            (json!({"get_window_state": {"pid": 200}}), "step_tool_unknown"),
            (json!({"launch_app": {"name": "Terminal"}}), "step_tool_unknown"),
            (json!({"run_script": {"script": "x"}}), "step_tool_unknown"),
            (json!({"tool": "click", "args": {}, "x": 1}), "step_unreadable"),
            (json!({"click": {}, "type_text": {"text": "x"}}), "step_unreadable"),
        ] {
            let (out, _) = run(&all(), &settings(), json!({"steps": [step.clone()]})).await;
            assert_eq!(code(&out), want, "{step}");
        }
    }
}
