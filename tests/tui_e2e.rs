//! TUI end-to-end tests via termlens (pebble: termlens evaluation).
//! Real binary in a PTY against a throwaway tracker (PEBBLES_DB_PATH + .no_jj).
use termlens::{Key, Screen, Terminal};

use std::fs;
use std::path::{Path, PathBuf};

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

    // The highlight stays on the next item (Fresh idea), not the top.
    t.wait_until(|s| highlighted_row_text(s).contains("Fresh idea"))?;

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

/// Approving keeps the highlight on the next item instead of teleporting to
/// the first one (r[tui-review]).
#[test]
fn tui_approve_keeps_selection_on_next_item() -> termlens::Result<()> {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-tui-keep-")
        .tempdir()
        .expect("tempdir");
    let root = tempdir.path().to_path_buf();
    build_project(
        &root,
        &[
            (
                "alpha-bug",
                0x11111111,
                "bug",
                "proposed",
                "Alpha idea",
                false,
            ),
            (
                "beta-design",
                0x22222222,
                "design",
                "proposed",
                "Beta idea",
                false,
            ),
            (
                "gamma-usecase",
                0x33333333,
                "usecase",
                "proposed",
                "Gamma idea",
                false,
            ),
        ],
    );

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&root)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    t.wait_until(|s| {
        s.contains("Alpha idea") && s.contains("Beta idea") && s.contains("Gamma idea")
    })?;

    // Review filter lists only proposed issues; Alpha (bug) sorts first.
    t.send(Key::Char('r'))?;
    assert!(
        settled(&mut t, "filter with Alpha selected", |s| {
            highlighted_row_text(s).contains("Alpha idea")
        }),
        "Alpha selected first"
    );

    // Approve the first issue: the highlight moves to the next item (Beta),
    // not back to the top of the list.
    t.send(Key::Char('a'))?;
    assert!(
        settled(&mut t, "approve Alpha", |s| {
            s.contains("Approved") && highlighted_row_text(s).contains("Beta idea")
        }),
        "selection follows to the next item"
    );

    // Move to the last item and approve it: the highlight lands on the new
    // last item (Beta), not the first.
    t.send(Key::Char('j'))?;
    assert!(
        settled(&mut t, "Gamma selected", |s| {
            highlighted_row_text(s).contains("Gamma idea")
        }),
        "j moves to the last item"
    );
    t.send(Key::Char('a'))?;
    assert!(
        settled(&mut t, "approve Gamma", |s| {
            s.contains("Approved") && highlighted_row_text(s).contains("Beta idea")
        }),
        "last approved: highlight moves to the new last item"
    );

    // Approving the last remaining item clears the list: no selection.
    t.send(Key::Char('a'))?;
    assert!(
        settled(&mut t, "approve Beta", |s| {
            s.contains("Approved") && s.contains("No selection")
        }),
        "last remaining approved: selection cleared"
    );

    // All three approved on disk; nothing proposed remains.
    let files: Vec<String> = root
        .join(".pebbles/issues")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files.len(), 3, "three issue files remain: {files:?}");
    for (needle, id) in [
        ("alpha-idea", "11111111"),
        ("beta-idea", "22222222"),
        ("gamma-idea", "33333333"),
    ] {
        let file = files
            .iter()
            .find(|f| f.contains(needle) && f.contains(id))
            .unwrap_or_else(|| panic!("{needle} file missing: {files:?}"));
        let content = fs::read_to_string(root.join(".pebbles/issues").join(file)).unwrap();
        assert!(
            content.contains("\"status\": \"todo\""),
            "{needle}: {content}"
        );
    }

    // Quit, terminal restored.
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");
    Ok(())
}

/// Build a project (`.pebbles` + `.no_jj`) from (name, id, type, status, title, part_of_epic) rows.
fn build_project(root: &Path, rows: &[(&str, u128, &str, &str, &str, bool)]) {
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

/// Externally add an issue - another pebbles process, an agent session, or
/// a manual edit - by writing the issue file and appending it to the index.
/// Used by the refresh e2e to shape external state while the TUI runs.
fn add_issue_externally(
    pebbles: &Path,
    name: &str,
    id: u128,
    itype: &str,
    status: &str,
    title: &str,
) {
    let uuid = format!("{:08x}-1111-4222-8333-{:012x}", id, id);
    let file = format!("{itype}-{:08x}-{name}.json", id);
    let issue = format!(
        "{{\"id\":\"{uuid}\",\"title\":\"{title}\",\"description\":null,\"status\":\"{status}\",\"issue_type\":\"{itype}\",\"tags\":[],\"relationships\":[],\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n",
    );
    fs::write(pebbles.join("issues").join(&file), issue).expect("issue file");
    // The index ends "<entries>]}"; splice the new entry in front of the "]}".
    let index_content = fs::read_to_string(pebbles.join("index.json")).unwrap();
    let mut updated = index_content[..index_content.len() - 2].to_string();
    updated.push_str(format!(",{{\"id\":\"{uuid}\",\"file_name\":\"{file}\"}}]}}").as_str());
    fs::write(pebbles.join("index.json"), updated).expect("index update");
}

/// Externally flip an issue's status by rewriting its file in place: what
/// an external `pebbles approve` looks like, with index.json untouched.
fn set_status_externally(pebbles: &Path, file_name: &str, status: &str) {
    let path = pebbles.join("issues").join(file_name);
    let content = fs::read_to_string(&path).unwrap();
    let updated = content.replace(
        "\"status\":\"proposed\"",
        format!("\"status\":\"{status}\"").as_str(),
    );
    fs::write(&path, updated).expect("status flip");
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

/// `p` (poll) re-runs project discovery and reloads every project, so
/// external state changes - another pebbles process, an agent session, a
/// manual edit, a new project - become visible without restarting, and the
/// selection re-settles in place (r[tui-refresh]). Nothing updates between
/// keys: the view stays stale until `p`.
#[test]
fn tui_refresh_shows_external_changes_and_keeps_selection() -> termlens::Result<()> {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-tui-refresh-")
        .tempdir()
        .expect("tempdir");
    let home = tempdir.path().to_path_buf();
    build_project(
        &home.join("repos/proj-a"),
        &[
            (
                "alpha-idea",
                0x11111111,
                "bug",
                "proposed",
                "Alpha idea",
                false,
            ),
            (
                "beta-idea",
                0x22222222,
                "usecase",
                "proposed",
                "Beta idea",
                false,
            ),
        ],
    );

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("HOME", home.to_string_lossy().to_string())
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&home)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    // The review filter lists both ideas with Alpha (bug sorts first)
    // selected.
    t.wait_until(|s| s.contains("proj-a"))?;
    t.send(Key::Char('r'))?;
    assert!(
        settled(&mut t, "filter with Alpha selected", |s| {
            s.contains("Alpha idea")
                && s.contains("Beta idea")
                && highlighted_row_text(s).contains("Alpha idea")
        }),
        "filter with Alpha selected"
    );

    // External state changes while the TUI runs: a new issue and an
    // in-place status flip in proj-a, plus a brand-new project appearing
    // in the registry.
    let pebbles_a = home.join("repos/proj-a/.pebbles");
    add_issue_externally(
        &pebbles_a,
        "gamma-idea",
        0x33333333,
        "design",
        "proposed",
        "Gamma idea",
    );
    set_status_externally(&pebbles_a, "usecase-22222222-beta-idea.json", "done");
    build_project(
        &home.join("repos/proj-b"),
        &[(
            "delta-idea",
            0x44444444,
            "usecase",
            "proposed",
            "Delta idea",
            false,
        )],
    );
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Load-once-per-invocation: with no key pressed the screen still shows
    // the startup issue set; the external changes are invisible.
    let stale = t.screen();
    assert!(
        !stale.contains("Gamma idea"),
        "stale before refresh: {stale}"
    );
    assert!(
        stale.contains("Beta idea"),
        "Beta still proposed before refresh: {stale}"
    );
    assert!(!stale.contains("proj-b"), "stale before refresh: {stale}");

    // `p` reloads the changed project: Gamma appears under proj-a (its
    // count grows 2 -> 3), Beta (now done) leaves the proposed filter, the
    // freshly discovered proj-b joins the list, Alpha keeps the highlight.
    t.send(Key::Char('p'))?;
    assert!(
        settled(&mut t, "refresh applied", |s| {
            s.contains("Gamma idea")
                && s.contains("proj-a (3)")
                && s.contains("proj-b (1)")
                && !tree_text(s).contains("Beta idea")
                && highlighted_row_text(s).contains("Alpha idea")
        }),
        "refresh applied"
    );

    // A second refresh is harmless: same data, selection still in place.
    t.send(Key::Char('p'))?;
    assert!(
        settled(&mut t, "second refresh", |s| {
            s.contains("Gamma idea") && highlighted_row_text(s).contains("Alpha idea")
        }),
        "second refresh"
    );

    // Quit, terminal restored.
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");

    // The external edits are what the TUI showed: new issue on disk, Beta
    // flipped to done.
    let files: Vec<String> = fs::read_dir(home.join("repos/proj-a/.pebbles/issues"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        files.iter().any(|f| f.contains("gamma-idea")),
        "gamma file written: {files:?}"
    );
    let beta =
        fs::read_to_string(pebbles_a.join("issues/usecase-22222222-beta-idea.json")).unwrap();
    assert!(beta.contains("\"status\":\"done\""), "{beta}");
    Ok(())
}

/// `p` works in the plain browser tree too (not just the review filter):
/// project counts, newly discovered projects and selection all update in
/// place, with the registry order re-applied (r[tui-refresh]).
#[test]
fn tui_refresh_in_plain_tree_adds_projects_and_keeps_selection() -> termlens::Result<()> {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-tui-refresh-")
        .tempdir()
        .expect("tempdir");
    let home = tempdir.path().to_path_buf();
    build_project(
        &home.join("repos/proj-a"),
        &[(
            "alpha-idea",
            0x11111111,
            "bug",
            "proposed",
            "Alpha idea",
            false,
        )],
    );

    let mut t = Terminal::builder()
        .size(100, 24)
        .env_clear()
        .env("HOME", home.to_string_lossy().to_string())
        .env("NO_COLOR", "1")
        .timeout(std::time::Duration::from_secs(10))
        .current_dir(&home)
        .arg("tui")
        .spawn(env!("CARGO_BIN_EXE_pebbles"))?;

    // Startup opens proj-a down to its first issue, Alpha, and selects it.
    assert!(
        settled(&mut t, "Alpha selected in plain tree", |s| {
            highlighted_row_text(s).contains("Alpha idea")
        }),
        "Alpha selected in plain tree"
    );

    // External state changes: a new issue in proj-a and a brand-new project.
    add_issue_externally(
        &home.join("repos/proj-a/.pebbles"),
        "gamma-idea",
        0x33333333,
        "design",
        "proposed",
        "Gamma idea",
    );
    build_project(
        &home.join("repos/proj-b"),
        &[(
            "delta-idea",
            0x44444444,
            "usecase",
            "proposed",
            "Delta idea",
            false,
        )],
    );
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Still stale until a key is pressed.
    let stale = t.screen();
    assert!(
        !stale.contains("Gamma idea"),
        "stale before refresh: {stale}"
    );
    assert!(!stale.contains("proj-b"), "stale before refresh: {stale}");

    // `p`: proj-a grows 1 -> 2 and shows Gamma, proj-b joins the registry,
    // and the highlight stays on Alpha.
    t.send(Key::Char('p'))?;
    assert!(
        settled(&mut t, "plain tree refresh applied", |s| {
            tree_text(s).contains("proj-a (2)")
                && tree_text(s).contains("Gamma idea")
                && s.contains("proj-b")
                && highlighted_row_text(s).contains("Alpha idea")
        }),
        "plain tree refresh applied"
    );

    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");
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

/// The text of the highlighted tree row, limited to the tree pane (columns
/// 0..40): the row whose line starts with the "> " selection marker (the
/// focus background style is not observable through termlens's text
/// snapshot). Panics when nothing is highlighted.
fn highlighted_row_text(s: &Screen) -> String {
    for y in 1..=22 {
        let row: String = s.row_text(y).chars().take(40).collect();
        if row.chars().nth(1) == Some('>') {
            return row;
        }
    }
    panic!("no highlighted row");
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

#[test]
fn tui_edit_pane_updates_fields_and_esc_cancels() -> termlens::Result<()> {
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

    // Focus the detail pane (BackTab from the tree) and open the edit pane
    // on the selected issue (the first leaf: Login bug, id 22222222).
    t.send(Key::BackTab)?;
    t.wait_until(|s| s.row_text(23).trim_end().is_empty())?;
    t.send(Key::Char('e'))?;
    assert!(
        settled(&mut t, "edit pane open", |s| {
            s.contains("edit: 22222222")
                && s.contains("> Title: Login bug")
                && s.contains("Status: todo")
        }),
        "edit pane open"
    );

    // Title: edit in place; Enter applies through the operations layer.
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "title edit", |s| s
            .contains("Enter applies, Esc cancels")
            && s.contains("Login bug")),
        "title edit"
    );
    for c in " fixed".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "title applied", |s| {
            s.row_text(23).contains("Updated title") && s.contains("Title: Login bug fixed")
        }),
        "title applied"
    );

    // Status: step to the field, cycle todo -> done in navigation, one
    // Enter applies (commit-and-stay: cycled fields never arm).
    for (needle, _label) in [
        ("> Description:", "desc"),
        ("> Type:", "type"),
        ("> Status:", "status"),
    ] {
        t.send(Key::Char('j'))?;
        assert!(
            settled(&mut t, "field", |s| s.contains(needle)),
            "step to {needle}"
        );
    }
    assert!(
        settled(&mut t, "status hint", |s| s
            .contains("Left/Right cycles, Enter applies")),
        "status hint"
    );
    t.send(Key::Right)?;
    t.send(Key::Right)?;
    assert!(
        settled(&mut t, "status cycled", |s| s.contains("> Status: done")),
        "status cycled"
    );
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "status applied", |s| s
            .row_text(23)
            .contains("Status: done")),
        "status applied"
    );
    // The apply stays in the pane: the field is still highlighted, ready
    // to cycle again, without an arming state.
    assert!(
        settled(&mut t, "status stays", |s| {
            s.contains("> Status: done") && !s.contains("Enter applies, Esc cancels")
        }),
        "status stays"
    );

    // Type: back up one field, cycle bug -> usecase, apply on the first
    // Enter after the cycle.
    t.send(Key::Char('k'))?;
    assert!(
        settled(&mut t, "back to type", |s| s.contains("> Type:")),
        "back to type"
    );
    t.send(Key::Right)?;
    t.send(Key::Right)?;
    assert!(
        settled(&mut t, "type cycled", |s| s.contains("> Type: usecase")),
        "type cycled"
    );
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "type applied", |s| s
            .row_text(23)
            .contains("Converted to usecase")),
        "type applied"
    );

    // Description: back up one field, type a value; Enter inserts a newline
    // (multi-line buffer), Tab applies.
    t.send(Key::Char('k'))?;
    assert!(
        settled(&mut t, "back to description", |s| s
            .contains("> Description:")),
        "back to description"
    );
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "description edit", |s| s
            .contains("Enter: new line, Tab: apply")),
        "description edit"
    );
    for c in "Fixes login".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    // The label row ("Description:") and the multi-line widget content
    // ("Fixes login") render as separate rows under the rect layout.
    assert!(
        settled(&mut t, "description newline", |s| {
            let desc = s.contains("Description:");
            let fixes = s.contains("Fixes login");
            let enter_hint = s.contains("Enter: new line, Tab: apply");
            let ok = desc && fixes;
            if !ok {
                eprintln!("DEBUG desc={desc} fixes={fixes} enter_hint={enter_hint}");
            }
            ok
        }),
        "description newline"
    );
    t.send(Key::Tab)?;
    assert!(
        settled(&mut t, "description applied", |s| {
            s.row_text(23).contains("Updated description") && s.contains("Fixes login")
        }),
        "description applied"
    );

    // Esc cancels without writing: reopen, type into the title, Esc twice
    // (once for the field edit, once for the pane) and verify the disk
    // still holds the committed title.
    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "pane closed", |s| !s.contains("edit: 22222222")),
        "pane closed"
    );
    t.send(Key::Char('e'))?;
    assert!(
        settled(&mut t, "pane reopened", |s| s.contains("edit: 22222222")),
        "pane reopened"
    );
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "title edit again", |s| s
            .contains("Enter applies, Esc cancels")),
        "title edit again"
    );
    t.send(Key::Char('!'))?;
    assert!(
        settled(&mut t, "buffer shows", |s| {
            s.contains("> Title:") && s.contains("Login bug fixed!")
        }),
        "buffer shows"
    );
    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "field edit cancelled", |s| {
            s.contains("j/k move, Enter edits") && s.contains("Title: Login bug fixed")
        }),
        "field edit cancelled"
    );
    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "pane closed again", |s| !s
            .contains("edit: 22222222")),
        "pane closed again"
    );

    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");
    let after = t.screen();
    assert!(!after.alternate_screen(), "{after}");
    assert!(after.cursor_visible(), "{after}");

    // The operations landed on disk; the Esc'd title edit did not.
    let files: Vec<String> = root
        .join(".pebbles/issues")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let edited = files
        .iter()
        .find(|f| f.contains("22222222"))
        .unwrap_or_else(|| panic!("edited issue file: {files:?}"));
    assert!(
        edited.contains("login-bug-fixed"),
        "file renamed by the title change: {files:?}"
    );
    let content = fs::read_to_string(root.join(".pebbles/issues").join(edited)).unwrap();
    assert!(
        content.contains("\"title\": \"Login bug fixed\""),
        "{content}"
    );
    assert!(content.contains("\"status\": \"done\""), "{content}");
    assert!(content.contains("\"issue_type\": \"usecase\""), "{content}");
    assert!(
        content.contains("\"description\": \"Fixes login\\n\""),
        "the description keeps its typed trailing newline: {content}"
    );
    assert!(
        !content.contains("Login bug!"),
        "the cancelled title edit must not be written: {content}"
    );
    Ok(())
}
/// While editing one field the other rows keep showing the issue's live
/// values (regression: the shared empty buffer + cursor used to take over
/// every text row), and cycled fields apply on the FIRST Enter without an
/// arming step (r[tui-edit]).
#[test]
fn tui_edit_pane_live_rows_and_cycle_applies_on_first_enter() -> termlens::Result<()> {
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
    // Focus the detail pane and open the edit pane on Login bug.
    t.send(Key::BackTab)?;
    t.wait_until(|s| s.row_text(23).trim_end().is_empty())?;
    t.send(Key::Char('e'))?;
    assert!(
        settled(&mut t, "edit pane open", |s| {
            s.contains("edit: 22222222") && s.contains("> Title: Login bug")
        }),
        "edit pane open"
    );

    // Arm the Title edit: the cursor glyph appears on exactly one row, and
    // no non-active row shows it - those rows keep live values.
    t.send(Key::Enter)?;
    // The widget owns its reserved slot: the title content renders in the
    // slot while every other row keeps the issue's live value.
    assert!(
        settled(&mut t, "live non-active rows", |s| {
            s.contains("> Title:")
                && s.contains("Login bug")
                && s.contains("Description:")
                && s.contains("Type: bug")
                && s.contains("Status: todo")
                && !s.contains("Login bug\u{258f}")
        }),
        "live non-active rows"
    );
    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "field edit cancelled", |s| {
            s.contains("j/k move, Enter edits") && s.contains("Status: todo")
        }),
        "field edit cancelled"
    );

    // Status: step to it (three j presses wrap Title to Status), cycle
    // todo -> done, and apply with the FIRST Enter - no arm first.
    for _ in 0..3 {
        t.send(Key::Char('j'))?;
    }
    assert!(
        settled(&mut t, "on status", |s| s.contains("> Status: todo")),
        "on status"
    );
    t.send(Key::Right)?;
    t.send(Key::Right)?;
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "status applied", |s| s
            .row_text(23)
            .contains("Status: done")),
        "status applied"
    );
    // One Enter applied and stayed: no second arming state, the pane is
    // still open on the same field.
    assert!(
        settled(&mut t, "no re-arm", |s| {
            s.contains("> Status: done")
                && s.contains("Left/Right cycles, Enter applies")
                && s.row_text(23).contains("Status: done")
        }),
        "no re-arm"
    );

    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "pane closed", |s| !s.contains("edit: 22222222")),
        "pane closed"
    );
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");

    // The single Enter landed on disk.
    let files: Vec<String> = root
        .join(".pebbles/issues")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let edited = files
        .iter()
        .find(|f| f.contains("22222222"))
        .unwrap_or_else(|| panic!("edited issue file: {files:?}"));
    let content = fs::read_to_string(root.join(".pebbles/issues").join(edited)).unwrap();
    assert!(content.contains("\"status\": \"done\""), "{content}");
    assert!(
        content.contains("\"title\": \"Login bug\""),
        "the cancelled title edit must not be written: {content}"
    );
    Ok(())
}

// Widget-specific e2e: the TextArea is the editor inside the pane (d31).
// Typing, cursor moves (Home/End/Left), Enter-newline in the multi-line
// Description, Tab apply, and Esc-cancel all go through the widget.
#[test]
fn tui_edit_pane_widget_editing_types_moves_and_multiline() -> termlens::Result<()> {
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
    t.send(Key::BackTab)?;
    t.wait_until(|s| s.row_text(23).trim_end().is_empty())?;
    t.send(Key::Char('e'))?;
    assert!(
        settled(&mut t, "edit pane open", |s| s.contains("edit: 22222222")
            && s.contains("> Title: Login bug")),
        "edit pane open"
    );

    // Typing inserts into the widget; the cursor starts at the end so
    // appended text lands last.
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "title armed", |s| s
            .contains("Enter applies, Esc cancels")),
        "title armed"
    );
    for c in " fixed".chars() {
        t.send(Key::Char(c))?;
    }
    assert!(
        settled(&mut t, "typed", |s| s.contains("Login bug fixed")),
        "typed"
    );
    // Home moves the cursor to the line head; typing inserts there.
    t.send(Key::Home)?;
    for c in "[x] ".chars() {
        t.send(Key::Char(c))?;
    }
    assert!(
        settled(&mut t, "cursor moved by Home", |s| s
            .contains("[x] Login bug fixed")),
        "cursor moved by Home"
    );
    // End + Left moves back one char from the end; a typed char lands
    // before the final 'd'.
    t.send(Key::End)?;
    t.send(Key::Left)?;
    t.send(Key::Char('-'))?;
    assert!(
        settled(&mut t, "cursor moved by End+Left", |s| s
            .contains("[x] Login bug fixe-d")),
        "cursor moved by End+Left"
    );
    // Esc cancels the field edit: nothing is written, the pane returns to
    // navigation showing the live title.
    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "field edit canceled", |s| {
            s.contains("j/k move, Enter edits") && !s.contains("[x] Login bug")
        }),
        "field edit canceled"
    );

    // Description: two typed lines with Enter between, Tab applies.
    t.send(Key::Char('j'))?;
    assert!(
        settled(&mut t, "on description", |s| s.contains("> Description:")),
        "on description"
    );
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "description armed", |s| s
            .contains("Enter: new line, Tab: apply")),
        "description armed"
    );
    for c in "First line".chars() {
        t.send(Key::Char(c))?;
    }
    t.send(Key::Enter)?;
    assert!(
        settled(&mut t, "newline inserted", |s| s.contains("First line")),
        "newline inserted"
    );
    for c in "Second line".chars() {
        t.send(Key::Char(c))?;
    }
    assert!(
        settled(&mut t, "second line typed", |s| s.contains("Second line")),
        "second line typed"
    );
    t.send(Key::Tab)?;
    assert!(
        settled(&mut t, "multiline applied", |s| s
            .row_text(23)
            .contains("Updated description")),
        "multiline applied"
    );

    t.send(Key::Esc)?;
    assert!(
        settled(&mut t, "pane closed", |s| !s.contains("edit: 22222222")),
        "pane closed"
    );
    t.send(Key::Char('q'))?;
    let status = t.wait_exit()?;
    assert!(status.success(), "exit: {status}");

    let files: Vec<String> = root
        .join(".pebbles/issues")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let edited = files
        .iter()
        .find(|f| f.contains("22222222"))
        .unwrap_or_else(|| panic!("edited issue file: {files:?}"));
    let content = fs::read_to_string(root.join(".pebbles/issues").join(edited)).unwrap();
    assert!(
        content.contains("\"title\": \"Login bug\""),
        "canceled title edit not written: {content}"
    );
    assert!(
        content.contains("\"description\": \"First line\\nSecond line\""),
        "multiline description written with the newline: {content}"
    );
    Ok(())
}
