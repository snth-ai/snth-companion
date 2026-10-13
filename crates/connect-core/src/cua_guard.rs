//! What the synth is never allowed to trigger through the driver, whatever
//! the owner's settings say, and the one rule for where the driver may
//! write a file.
//!
//! Cua Driver's catalog (58 tools at the pinned 0.34.1) is built for an
//! agent on its own machine. A few of its tools do things on the OWNER's
//! computer that the owner has no switch for in this app: they install
//! software, change the driver's own settings, keep pictures of the screen
//! on disk, or write a file wherever the caller says. Those are decided
//! here, in one list, and the mcp pipe (`mcp::filter_line`) refuses them
//! with a sentence the synth can pass on to the owner.
//!
//! The list was made by reading every tool of the pinned driver, its
//! description and its source. `connectd selftest cua` holds it against the
//! real catalog: a tool whose name looks like one of these kinds, or an
//! argument that looks like a place to write, that is not classified here
//! fails the self-test. So a new pin cannot bring one in unseen.

use serde_json::Value;

use crate::settings::FilesSettings;

/// One tool the synth may never call.
pub struct Never {
    pub tool: &'static str,
    /// Why, in one line, for whoever reads this list.
    pub why: &'static str,
    /// What the synth is told: what it cannot do here and what to say to
    /// the owner. Sent after `denied: `.
    pub tell: &'static str,
}

/// THE LIST. One line of reason each; the reasons were checked in the
/// driver's source at the pinned tag.
pub const NEVER: &[Never] = &[
    Never {
        tool: "start_recording",
        why: "keeps screenshots, window contents and optionally a video of the owner's screen in a folder on disk; the owner has no switch for it yet",
        tell: "recording the screen is not available through SNTH Connect; tell the owner you cannot record their screen from here, and that their system's own screen recorder can",
    },
    Never {
        tool: "install_ffmpeg",
        why: "installs a program on the owner's computer with the system's package manager",
        tell: "installing software on this computer is not something you may do through SNTH Connect; ffmpeg is only needed for recording, which is not available either, so tell the owner that if they asked for it",
    },
    Never {
        tool: "install_extension",
        why: "downloads and installs a component of the driver on the owner's computer",
        tell: "installing software on this computer is not something you may do through SNTH Connect; work from the screenshot and the accessibility tree, and tell the owner the visual parser is not part of SNTH Connect if they ask",
    },
    Never {
        tool: "check_for_update",
        why: "asks GitHub from the owner's computer, writes its answer into ~/.cua-driver (the home of a driver the owner may run themselves), and returns an install command that would replace that driver",
        tell: "the helper on this computer is kept at the version SNTH Connect was tested with and is updated together with the app; never run an installer for it, and if the owner asks about updates tell them to update SNTH Connect",
    },
    Never {
        tool: "set_config",
        why: "changes the driver's own settings: its experimental_pip keys and (since 0.34) its cursor.motion defaults are written to ~/.cua-driver/config.json, which a driver the owner runs themselves reads too, and max_image_dimension would lift the owner's screenshot limit",
        tell: "the helper's settings on this computer are its owner's, in SNTH Connect under Computer control; for a smaller screenshot pass max_image_dimension on the call itself, and for anything else tell the owner where the setting is",
    },
    Never {
        tool: "replay_trajectory",
        why: "runs stored actions inside the driver, where the app cannot check them one by one (blocked apps, taking over the screen), from a folder that can only come from a recording",
        tell: "replaying a recording is not available through SNTH Connect; do the steps one call at a time, and tell the owner so if they asked for a replay",
    },
];

/// Arguments that name a place where the driver WRITES a file: a
/// screenshot (`get_window_state`, `get_desktop_state`), a marked-up
/// screenshot (`click`), a download (`browser_download`). They pass only
/// when the place is inside a folder the owner shared for writing (see
/// `confine_paths`). Matched by name on any tool, so a tool that gains one
/// of them is covered too.
pub const WRITE_ARGS: [&str; 3] = ["screenshot_out_file", "debug_image_out", "destination_root"];

/// Tools whose names look like the kinds above and that MAY be called,
/// each with the reason it is safe. Read in the driver's source.
pub const REVIEWED_TOOLS: &[(&str, &str)] = &[
    ("stop_recording", "ends a recording; nothing can start one, and stopping writes nothing new"),
    ("get_recording_state", "read-only"),
    ("get_config", "read-only"),
    ("check_permissions", "read-only here: an embedded daemon never raises a system prompt, whatever `prompt` says (check_permissions.rs, should_prompt_permissions)"),
    ("escalate_session", "deprecated; changes one session's capture scope in memory, which every call can already choose for itself; nothing is stored and nobody is asked"),
    ("browser_download", "saves a file into `destination_root`, which is held to the shared folders (WRITE_ARGS)"),
];

/// Arguments whose names look like a place on disk and that are NOT a
/// place the driver writes to, each with what they are.
pub const REVIEWED_ARGS: &[(&str, &str, &str)] = &[
    ("clipboard_write", "file_path", "a file to put on the clipboard: read, not written"),
    ("clipboard_write", "image_path", "an image to put on the clipboard: read, not written"),
    ("browser_set_input_files", "files", "files handed to a page's file field: read, not written"),
    ("browser_pointer", "destination_ref", "an element of the page, not a place on disk"),
    ("invoke_menu", "path", "the names of a menu and its item, not a place on disk"),
    ("launch_app", "launch_path", "the program to start, as list_apps gave it (Windows, Linux): started, not written"),
    ("launch_app", "path", "the program to start (Windows): started, not written"),
    ("drag", "path", "the points a drag passes through (Linux), not a place on disk"),
    ("start_recording", "output_dir", "the tool itself is never allowed"),
    ("replay_trajectory", "dir", "the tool itself is never allowed"),
];

/// What in a tool's name makes it one of the kinds this list is about
/// (anywhere in the name: `uninstall`, `reconfigure` and `recordings`
/// count). A tool with one of them must be in `NEVER` or `REVIEWED_TOOLS`.
const TOOL_MARKS: [&str; 16] = [
    "install", "record", "updat", "upgrad", "config", "permission", "histor", "extension",
    "download", "escalat", "replay", "telemetr", "autostart", "skill", "grant", "revok",
];

/// The words in an argument's name that make it look like a place on disk.
const ARG_WORDS: [&str; 12] = [
    "file", "files", "filename", "path", "paths", "dir", "directory", "out", "output",
    "destination", "save", "folder",
];

fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(['_', '-', '.']).filter(|w| !w.is_empty())
}

/// The sentence for the synth when it calls a tool it may never call.
pub fn refusal(tool: &str) -> Option<String> {
    NEVER
        .iter()
        .find(|n| n.tool == tool)
        .map(|n| format!("denied: {}", n.tell))
}

/// Hold a driver catalog (the `tools` array of `tools/list`) against the
/// lists above. Returns what is not classified: a tool whose name has one
/// of `TOOL_MARKS`, or an argument whose name has one of `ARG_WORDS`, that
/// no list mentions. Empty = every such tool and argument was read and
/// decided. The self-test fails on anything else.
pub fn unclassified(tools: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let never = NEVER.iter().any(|n| n.tool == name);
        let reviewed = REVIEWED_TOOLS.iter().any(|(t, _)| *t == name);
        if !never && !reviewed && TOOL_MARKS.iter().any(|mark| name.contains(mark)) {
            out.push(format!("tool {name}"));
        }
        let Some(props) = tool
            .pointer("/inputSchema/properties")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for arg in props.keys() {
            let known = WRITE_ARGS.contains(&arg.as_str())
                || REVIEWED_ARGS.iter().any(|(t, a, _)| *t == name && a == arg);
            if !known && words(arg).any(|w| ARG_WORDS.contains(&w)) {
                out.push(format!("argument {name}.{arg}"));
            }
        }
    }
    out
}

/// The one rule for a place the driver writes to: it must be inside a
/// folder the owner shared for writing, checked after symlinks are
/// resolved (`files::resolve_write`, the files tool's own check). The
/// argument is replaced by the resolved path, so the driver writes where
/// the check looked. Refused when files are off, when nothing is shared
/// for writing there, when the path is not absolute (`~` is not ours to
/// expand), and when the owner is asked before every write: the driver
/// cannot ask.
///
/// Ok(true) = an argument was checked and put in its resolved form.
pub async fn confine_paths(msg: &mut Value, files: &FilesSettings) -> Result<bool, String> {
    let Some(args) = msg
        .get_mut("params")
        .and_then(|p| p.get_mut("arguments"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(false);
    };
    let mut changed = false;
    for name in WRITE_ARGS {
        let Some(value) = args.get(name) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let hint = "leave the argument out to get the picture in the answer, or tell the owner which folder to share for writing in SNTH Connect under Files";
        let Some(path) = value.as_str() else {
            return Err(format!("denied: {name} must be a path; {hint}"));
        };
        if !files.enabled {
            return Err(format!(
                "denied: {name}: files are turned off on this computer, so nothing may be written to its disk; {hint}"
            ));
        }
        if files.ask_on_write {
            return Err(format!(
                "denied: {name}: the owner is asked before every file written on this computer, and the helper cannot ask; write the file with the files tool instead, or {hint}"
            ));
        }
        let resolved = crate::files::resolve_write(path, &files.roots)
            .await
            .map_err(|why| format!("{why}; {name} may only point into a folder shared for writing; {hint}"))?;
        args.insert(name.into(), Value::from(resolved.to_string_lossy().to_string()));
        changed = true;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Root;
    use serde_json::json;

    fn tool(name: &str, args: &[&str]) -> Value {
        let props: serde_json::Map<String, Value> =
            args.iter().map(|a| (a.to_string(), json!({"type": "string"}))).collect();
        json!({"name": name, "inputSchema": {"type": "object", "properties": props}})
    }

    fn call(name: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": args}})
    }

    #[test]
    fn every_never_tool_has_a_reason_and_words_for_the_owner() {
        let mut seen = std::collections::BTreeSet::new();
        for n in NEVER {
            assert!(seen.insert(n.tool), "{} is listed twice", n.tool);
            assert!(n.why.len() > 20, "{}", n.tool);
            assert!(n.tell.contains("owner"), "{}: the synth must be told what to say to the owner", n.tool);
            let said = refusal(n.tool).unwrap();
            assert!(said.starts_with("denied: "), "{said}");
            // A tool is in one list only.
            assert!(!REVIEWED_TOOLS.iter().any(|(t, _)| *t == n.tool), "{}", n.tool);
        }
        for must in ["start_recording", "install_ffmpeg", "install_extension", "check_for_update", "set_config", "replay_trajectory"] {
            assert!(refusal(must).is_some(), "{must}");
        }
        for may in ["stop_recording", "get_recording_state", "click", "get_window_state", "get_config"] {
            assert!(refusal(may).is_none(), "{may}");
        }
    }

    #[test]
    fn the_pinned_catalog_is_fully_classified() {
        // The tools of 0.34.1 whose names or arguments fall under the lists
        // (0.34 added no tool; its new arguments, `display_only` on
        // `get_window_state`, `cursor_motion` on `start_session` and the
        // `cursor.motion.*` keys of `set_config`, name no place on disk).
        let catalog = vec![
            tool("click", &["pid", "x", "y", "debug_image_out", "timeout_ms"]),
            tool("get_window_state", &["pid", "screenshot_out_file", "max_image_dimension", "display_only"]),
            tool("get_desktop_state", &["screenshot_out_file"]),
            tool("clipboard_write", &["text", "file_path", "image_path"]),
            tool("browser_set_input_files", &["files", "ref"]),
            tool("browser_download", &["destination_root", "ref"]),
            tool("browser_pointer", &["destination_ref", "ref"]),
            tool("invoke_menu", &["pid", "path"]),
            // Windows and Linux.
            tool("launch_app", &["name", "launch_path", "path"]),
            tool("drag", &["from_x", "path"]),
            tool("start_recording", &["output_dir", "record_video"]),
            tool("stop_recording", &["session"]),
            tool("get_recording_state", &["session"]),
            tool("replay_trajectory", &["dir"]),
            tool("install_ffmpeg", &["confirm"]),
            tool("install_extension", &["name", "confirm"]),
            tool("check_for_update", &[]),
            tool("check_permissions", &["prompt"]),
            tool("get_config", &[]),
            tool("set_config", &["key", "value", "cursor.motion", "cursor.motion.style", "cursor.motion.effects.trail"]),
            tool("start_session", &["session", "cursor_theme", "cursor_motion"]),
            tool("set_agent_cursor_motion", &["session", "style", "timing", "effects", "glide_duration_ms"]),
            tool("escalate_session", &["reason"]),
            tool("get_browser_state", &["snapshot_format"]),
        ];
        assert_eq!(unclassified(&catalog), Vec::<String>::new());
    }

    #[test]
    fn a_new_tool_or_argument_of_these_kinds_is_noticed() {
        let catalog = vec![
            tool("install_browser", &[]),
            tool("update_driver", &[]),
            tool("start_screen_recording", &[]),
            tool("set_runtime_config", &[]),
            tool("enable_history", &[]),
            tool("uninstall_extensions", &[]),
            tool("zoom", &["save_path"]),
            tool("get_window_state", &["tree_out_file"]),
            // Not of these kinds.
            tool("type_text", &["text", "timeout_ms", "profile"]),
            tool("list_windows", &["on_screen_only"]),
        ];
        assert_eq!(
            unclassified(&catalog),
            [
                "tool install_browser",
                "tool update_driver",
                "tool start_screen_recording",
                "tool set_runtime_config",
                "tool enable_history",
                "tool uninstall_extensions",
                "argument zoom.save_path",
                "argument get_window_state.tree_out_file",
            ]
        );
    }

    #[tokio::test]
    async fn the_driver_writes_only_into_a_folder_shared_for_writing() {
        let base = std::env::temp_dir().join(format!("connect-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (rw, ro, outside) = (base.join("rw"), base.join("ro"), base.join("outside"));
        for d in [&rw, &ro, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        let files = FilesSettings {
            enabled: true,
            roots: vec![Root { path: rw.clone(), write: true }, Root { path: ro.clone(), write: false }],
            ask_on_write: false,
        };
        let shot = |dir: &std::path::Path| call("get_window_state", json!({"pid": 1, "screenshot_out_file": dir.join("a.png").to_string_lossy()}));

        // Inside a folder shared for writing: passes, in its resolved form.
        let mut ok = shot(&rw);
        assert_eq!(confine_paths(&mut ok, &files).await, Ok(true));
        let resolved = ok["params"]["arguments"]["screenshot_out_file"].as_str().unwrap().to_string();
        assert!(resolved.ends_with("a.png"));
        assert_eq!(
            std::path::Path::new(&resolved).parent().unwrap(),
            crate::files::plain(&std::fs::canonicalize(&rw).unwrap())
        );
        assert_eq!(ok["params"]["arguments"]["pid"], 1);

        // Read-only folder, a folder that is not shared, a path that is not absolute.
        for mut bad in [
            shot(&ro),
            shot(&outside),
            call("get_desktop_state", json!({"screenshot_out_file": "~/Desktop/a.png"})),
            call("click", json!({"pid": 1, "x": 1, "y": 1, "debug_image_out": outside.join("c.png").to_string_lossy()})),
            call("browser_download", json!({"ref": "p1:2", "destination_root": outside.to_string_lossy()})),
            call("get_desktop_state", json!({"screenshot_out_file": 7})),
        ] {
            let why = confine_paths(&mut bad, &files).await.unwrap_err();
            assert!(why.starts_with("denied: "), "{why}");
            assert!(why.contains("owner"), "the synth must be told what to say to the owner: {why}");
        }
        // A download into the shared folder itself.
        let mut download = call("browser_download", json!({"ref": "p1:2", "destination_root": rw.to_string_lossy()}));
        assert_eq!(confine_paths(&mut download, &files).await, Ok(true));

        // A link out of the shared folder does not help.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, rw.join("way-out")).unwrap();
            let mut escape = call("get_desktop_state", json!({"screenshot_out_file": rw.join("way-out").join("a.png").to_string_lossy()}));
            assert!(confine_paths(&mut escape, &files).await.is_err());
        }

        // Files off, or the owner is asked before every write: nothing is written.
        let off = FilesSettings { enabled: false, ..files.clone() };
        assert!(confine_paths(&mut shot(&rw), &off).await.unwrap_err().contains("turned off"));
        let ask = FilesSettings { ask_on_write: true, ..files.clone() };
        assert!(confine_paths(&mut shot(&rw), &ask).await.unwrap_err().contains("asked before every file"));

        // A call without such an argument is not touched.
        let mut plain = call("get_window_state", json!({"pid": 1, "screenshot_out_file": null}));
        assert_eq!(confine_paths(&mut plain, &off).await, Ok(false));
        let mut click = call("click", json!({"pid": 1, "x": 1, "y": 1}));
        assert_eq!(confine_paths(&mut click, &off).await, Ok(false));
        let _ = std::fs::remove_dir_all(&base);
    }
}
