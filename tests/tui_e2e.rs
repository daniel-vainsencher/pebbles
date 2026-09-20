//! TUI end-to-end tests via termlens (pebble: termlens evaluation).
//! Real binary in a PTY against a throwaway tracker (PEBBLES_DB_PATH + .no_jj).
use termlens::{Key, Terminal};

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
    index.push_str(&mk("auth-epic", 0x44444444, "epic", "todo", "Auth epic", ""));
    index.push_str(&mk("login-bug", 0x22222222, "bug", "todo", "Login bug", "part"));
    index.push_str(&mk("proposed-idea", 0x33333333, "usecase", "proposed", "Fresh idea", ""));
    index.push_str(&mk("other-idea", 0x55555555, "design", "proposed", "Other idea", ""));
    index.pop(); // trailing comma
    index.push_str("]}");
    fs::write(pebbles.join("index.json"), index).expect("index");

    (tempdir, root)
}

#[test]
fn tui_browse_approve_quit() -> termlens::Result<()> {
    let (_tempdir, root) = fixture_with_issues();

    let mut t = Terminal::builder()
        .size(80, 24)
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
    assert!(content.contains("\"status\": \"todo\""), "approved: {content}");

    // The closed (deleted) proposal is gone from disk and the index.
    assert!(!files.iter().any(|f| f.contains("proposed-idea")), "closed issue deleted: {files:?}");
    let index = fs::read_to_string(root.join(".pebbles/index.json")).unwrap();
    assert!(!index.contains("33333333-"), "index no longer lists the deleted issue: {index}");
    Ok(())
}
