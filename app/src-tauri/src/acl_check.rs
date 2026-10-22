//! Every command the app registers must also be in build.rs's list and
//! allowed by some window's capability. A command missing there fails at
//! run time with "not allowed" (0.1.23: the speech engine choice).

fn handler_commands() -> Vec<String> {
    let main = include_str!("main.rs");
    let start = main.find("generate_handler![").expect("generate_handler! in main.rs");
    let rest = &main[start..];
    let body = &rest[..rest.find("])").expect("end of generate_handler!")];
    body.split(',')
        .filter_map(|item| item.trim().rsplit("::").next().map(str::trim).filter(|n| !n.is_empty() && !n.contains('[')).map(String::from))
        .collect()
}

#[test]
fn every_command_is_listed_and_allowed() {
    let names = handler_commands();
    assert!(names.len() > 50, "the handler list was not parsed ({} names)", names.len());
    let build = include_str!("../build.rs");
    let caps = [
        include_str!("../capabilities/main.json"),
        include_str!("../capabilities/island.json"),
        include_str!("../capabilities/approval.json"),
    ]
    .concat();
    for n in &names {
        assert!(build.contains(&format!("\"{n}\"")), "{n} is registered but not in build.rs's command list");
        let allow = format!("\"allow-{}\"", n.replace('_', "-"));
        assert!(caps.contains(&allow), "{n} is registered but no capability allows it ({allow})");
    }
}
