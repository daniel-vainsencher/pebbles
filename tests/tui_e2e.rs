//! TUI end-to-end tests via termlens (pebble: termlens evaluation).
//! Real binary in a PTY against a throwaway tracker (PEBBLES_DB_PATH + .no_jj).
use termlens::{Key, Screen, Terminal};

use std::fs;
use std::path::PathBuf;

/// Fixture: one epic with a todo child, one proposed orphan, `.no_jj` mode.
fn fixture_with_issues() -> (tempfile::TempDir, PathBuf) {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-tui-e2e-")
        .tempdir()
        .expect("tempdir");
    let root = tempdir.path().to_path_buf();
    let pebbles = root.join(".pebbles");
    fs::create_dir_all(pebbles.join("issues")).expect("issues dir");
    fs::write(pebbles.join(".no_jj"), "").expect("no_jj marker");

    let mut index = String::from("{\"issues\":[");
    let mk = |name: &str, id: u128, itype: &str, status: &str, title: &str, rel: &str| {
        let uuid = format!("{:08x}-1111-4222-8333-{:012x}", id, id);
        let short = format!("{:08x}", id);
        let id = uuid.clone();
        let file = format!("{itype}-{short}-{name}.json");
        let rel_field: String = if rel.is_empty() {
            "\"relationships\":[]".to_string()
        } else {
            format!(
                "\"relationships\":[{{\"relation_type\":\"part_of\",\"target_id\":\"{:08x}-1111-4222-8333-{:012x}\"}}]",
                0x44444444, 0x44444444
            )
        };
        let issue = format!(
            "{{\"id\":\"{id}\",\"title\":\"{title}\",\"description\":null,\"status\":\"{status}\",\"issue_type\":\"{itype}\",\"tags\":[],{rel_field},\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n",
        );
        fs::write(pebbles.join("issues").join(&file), issue).expect("issue file");
        format!("{{\"id\":\"{id}\",\"file_name\":\"{file}\"}},")
    };
    index.push_str(&mk(
        "auth-epic",
        0x44444444,
        "epic",
        "todo",
        "Auth epic",
        "",
    ));
    index.push_str(&mk(
        "login-bug",
        0x22222222,
        "bug",
        "todo",
        "Login bug",
        "part",
    ));
    index.push_str(&mk(
        "proposed-idea",
        0x33333333,
        "usecase",
        "proposed",
        "Fresh idea",
        "",
    ));
    index.push_str(&mk(
        "other-idea",
        0x55555555,
        "design",
        "proposed",
        "Other idea",
        "",
    ));
    index.pop(); // trailing comma
    index.push_str("]}");
    fs::write(pebbles.join("index.json"), index).expect("index");

    (tempdir, root)
}

#[test]
fn tui_browse_approve_quit() -> termlens::Result<()> {
    let (_tempdir, root) = fixture_with_issues();

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&root)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    // Browser shows the epic; unfold it to see its child.
    t.wait_until(|s| s.alternate_screen() && s.contains("Auth epic"))?;
    t.send(Key::Enter)?;
    t.wait_until(|s| s.contains("Login bug"))?;

    // Review filter: two panes stay up, only proposed issues are listed.
    t.send(Key::Char('r'))?;
    t.wait_until(|s| s.contains("proposed (a approve") && s.contains("Fresh idea"))?;
    t.wait_until(|s| s.contains("Status:") && s.contains("Fresh idea"))?;

    // Approve the first proposal (design sorts before usecase: Other idea).
    t.send(Key::Char('a'))?;
    t.wait_until(|s| s.contains("Approved"))?;

    // Selection moves to the remaining proposal: close it with d then y.
    t.send(Key::Char('d'))?;
    t.wait_until(|s| s.contains("Press y to confirm"))?;
    t.send(Key::Char('y'))?;
    t.wait_until(|s| s.contains("Deleted"))?;

    // Clear the filter, quit, terminal restored.
    t.send(Key::Char('r'))?;
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");

    // The approval landed on disk (pure file I/O under .no_jj).
    let files: Vec<String> = root
        .join(".pebbles/issues")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    // The approved proposal is now todo on disk.
    let approved = files
        .iter()
        .find(|f| f.contains("other-idea"))
        .expect("approved issue file exists");
    let content = fs::read_to_string(root.join(".pebbles/issues").join(approved)).unwrap();
    assert!(
        content.contains("\"status\": \"todo\""),
        "approved: {content}"
    );

    // The closed (deleted) proposal is gone from disk and the index.
    assert!(
        !files.iter().any(|f| f.contains("proposed-idea")),
        "closed issue deleted: {files:?}"
    );
    let index = fs::read_to_string(root.join(".pebbles/index.json")).unwrap();
    assert!(
        !index.contains("33333333-"),
        "index no longer lists the deleted issue: {index}"
    );
    Ok(())
}

/// Build a project (`.pebbles` + `.no_jj`) from (name, id, type, status, title, part_of_epic) rows.
fn build_project(root: &PathBuf, rows: &[(&str, u128, &str, &str, &str, bool)]) {
    let pebbles = root.join(".pebbles");
    fs::create_dir_all(pebbles.join("issues")).expect("issues dir");
    fs::write(pebbles.join(".no_jj"), "").expect("no_jj marker");
    let mut index = String::from("{\"issues\":[");
    for (name, id, itype, status, title, part_of) in rows {
        let uuid = format!("{:08x}-1111-4222-8333-{:012x}", id, id);
        let short = format!("{:08x}", id);
        let file = format!("{itype}-{short}-{name}.json");
        let rel_field: String = if *part_of {
            format!(
                "\"relationships\":[{{\"relation_type\":\"part_of\",\"target_id\":\"{:08x}-1111-4222-8333-{:012x}\"}}]",
                0x44444444, 0x44444444
            )
        } else {
            "\"relationships\":[]".to_string()
        };
        let epic_row = if *part_of {
            // ensure the referenced epic exists
            let epic_uuid = format!("{:08x}-1111-4222-8333-{:012x}", 0x44444444, 0x44444444);
            let epic_file = format!("epic-{:08x}-epic.json", 0x44444444);
            let epic_issue = format!(
                "{{\"id\":\"{epic_uuid}\",\"title\":\"Epic\",\"description\":null,\"status\":\"todo\",\"issue_type\":\"epic\",\"tags\":[],\"relationships\":[],\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n",
            );
            fs::write(pebbles.join("issues").join(&epic_file), epic_issue).expect("epic file");
            format!("{{\"id\":\"{epic_uuid}\",\"file_name\":\"{epic_file}\"}},")
        } else {
            String::new()
        };
        let issue = format!(
            "{{\"id\":\"{uuid}\",\"title\":\"{title}\",\"description\":null,\"status\":\"{status}\",\"issue_type\":\"{itype}\",\"tags\":[],{rel_field},\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n",
        );
        fs::write(pebbles.join("issues").join(&file), issue).expect("issue file");
        index.push_str(&epic_row);
        index.push_str(&format!("{{\"id\":\"{uuid}\",\"file_name\":\"{file}\"}},"));
    }
    index.pop();
    index.push_str("]}");
    fs::write(pebbles.join("index.json"), index).expect("index");
}

#[test]
fn tui_multi_project_lists_and_mutates_in_own_project() -> termlens::Result<()> {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-tui-mp-")
        .tempdir()
        .expect("tempdir");
    let home = tempdir.path().to_path_buf();

    // Older project: epic + todo child. Written first, so its index mtime is older.
    build_project(
        &home.join("repos/older-proj"),
        &[("todo-child", 0x22222222, "bug", "todo", "Login bug", true)],
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Newer project: one proposed issue. Its index mtime is newest -> listed first.
    build_project(
        &home.join("repos/newer-proj"),
        &[(
            "proposed-idea",
            0x33333333,
            "usecase",
            "proposed",
            "Fresh idea",
            false,
        )],
    );

    let mut t = Terminal::builder()
        .size(90, 24)
        .env_clear()
        .env("HOME", home.to_string_lossy().to_string())
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&home)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    // Both projects listed; no PEBBLES_DB_PATH anywhere.
    t.wait_until(|s| s.contains("newer-proj") && s.contains("older-proj"))?;

    // Unfold the first (newest) project and see its issues.
    t.send(Key::Right)?;
    t.wait_until(|s| s.contains("Fresh idea"))?;

    // Review filter across projects, approve in the issue's own project.
    t.send(Key::Char('r'))?;
    t.wait_until(|s| s.contains("proposed (a approve"))?;
    t.send(Key::Char('a'))?;
    t.wait_until(|s| s.contains("Approved"))?;

    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");

    // The approval landed in newer-proj only; older-proj untouched.
    let files: Vec<String> = fs::read_dir(home.join("repos/newer-proj/.pebbles/issues"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let approved = files
        .iter()
        .find(|f| f.contains("fresh-idea"))
        .unwrap_or_else(|| panic!("newer-proj files: {files:?}"));
    let content =
        fs::read_to_string(home.join("repos/newer-proj/.pebbles/issues").join(approved)).unwrap();
    assert!(
        content.contains("\"status\": \"todo\""),
        "approved in newer-proj: {content}"
    );

    let older_index =
        fs::read_to_string(home.join("repos/older-proj/.pebbles/index.json")).unwrap();
    assert!(
        older_index.contains("22222222"),
        "older project untouched: {older_index}"
    );
    Ok(())
}

#[test]
fn tui_search_filters_and_clears() -> termlens::Result<()> {
    let (_tempdir, root) = fixture_with_issues();

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&root)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    t.wait_until(|s| s.contains("Auth epic"))?;

    // Multi-word terms are independent and order-insensitive: "idea fresh"
    // (words in the reverse order of the title) still finds Fresh idea.
    t.send(Key::Char('/'))?;
    t.wait_until(|s| s.row_text(23).trim_end() == "search:")?;
    for c in "idea fresh".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    t.wait_until(|s| {
        s.contains("search: idea fresh")
            && !s.contains("Other idea")
            && !s.contains("Login bug")
            && s.row_text(23).trim_end().is_empty()
    })?;

    // Tab leaves the search line without closing it: keys stop editing the
    // query (the prompt disappears), and '/' re-focuses it from anywhere.
    t.send(Key::Char('/'))?;
    t.wait_until(|s| s.row_text(23).contains("search: idea fresh"))?;
    t.send(Key::Tab)?;
    t.wait_until(|s| s.row_text(23).trim_end().is_empty())?;
    t.send(Key::Char('/'))?;
    t.wait_until(|s| s.row_text(23).contains("search: idea fresh"))?;
    t.send(Key::Esc)?;
    t.wait_until(|s| s.contains("Fresh idea") && s.contains("Other idea"))?;

    // Esc (already above) cleared the search; re-run the clear cycle for the
    // s:t phase below.

    // s:t keeps only todo issues (Login bug + Auth epic).
    t.send(Key::Char('/'))?;
    t.wait_until(|s| s.row_text(23).trim_end() == "search:")?;
    for c in "s:t".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    t.wait_until(|s| {
        s.contains("Login bug")
            && s.contains("Auth epic")
            && !s.contains("Fresh idea")
            && s.row_text(23).trim_end().is_empty()
    })?;

    // Quit, terminal restored.
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");
    Ok(())
}

/// Poll `pred` on the live screen for a few seconds; returns whether it ever
/// held, printing the screen (with `label`) on failure. termlens's PTY
/// delivers rapid key bursts unreliably, so bursts stay short and every state
/// transition is polled instead of assumed.
fn settled(t: &mut Terminal, label: &str, mut predicate: impl FnMut(&Screen) -> bool) -> bool {
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if predicate(&t.screen()) {
            return true;
        }
    }
    eprintln!("{label} did not settle:\n{}", t.screen());
    false
}

/// The left tree pane (rows 1..22, first 40 columns) as text, so filter
/// assertions check the list and not the detail pane (which may reference
/// hidden issues in relationship lines).
fn tree_text(s: &Screen) -> String {
    let mut out = String::new();
    for y in 1..=22 {
        out.push_str(&s.row_text(y).chars().take(40).collect::<String>());
        out.push('\n');
    }
    out
}

#[test]
fn tui_search_type_prefixes_and_negation() -> termlens::Result<()> {
    let (_tempdir, root) = fixture_with_issues();

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&root)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    t.wait_until(|s| s.contains("Auth epic"))?;

    // t:bug narrows to just the bug.
    t.send(Key::Char('/'))?;
    t.wait_until(|s| s.row_text(23).trim_end() == "search:")?;
    for c in "t:bug".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "t:bug committed", |s| {
            s.contains("search: t:bug")
                && tree_text(s).contains("Login bug")
                && !tree_text(s).contains("Auth epic")
                && !tree_text(s).contains("Fresh idea")
                && !tree_text(s).contains("Other idea")
                && s.row_text(23).trim_end().is_empty()
        }),
        "t:bug committed"
    );

    // Backspace removes the committed query, then s:!d excludes done
    // (nothing here is done, so everything comes back).
    t.send(Key::Char('/'))?;
    assert!(
        settled(&mut t, "edit-t:bug prompt", |s| s
            .row_text(23)
            .contains("search: t:bug")),
        "edit-t:bug prompt"
    );
    for _ in 0..6 {
        t.send(Key::Backspace)?;
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    assert!(
        settled(&mut t, "query cleared", |s| s.row_text(23).trim_end()
            == "search:"),
        "query cleared"
    );
    for c in "s:!d".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "s:!d committed", |s| {
            s.contains("search: s:!d")
                && s.contains("Auth epic")
                && s.contains("Login bug")
                && s.contains("Fresh idea")
                && s.contains("Other idea")
                && s.row_text(23).trim_end().is_empty()
        }),
        "s:!d committed"
    );

    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");
    Ok(())
}
