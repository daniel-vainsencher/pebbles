//! Interactive TUI: two-pane browser (epic/pebble tree + detail) with a
//! proposed-issues review filter. Pure helpers are separated from the
//! terminal shell so they are unit-testable without I/O (ARCHITECTURE.md
//! Testing Strategy).
//!
//! Design: pebble 94223b04. Widget: tui-tree-widget (pebble: tui-tree-widget eval). E2E: termlens.
//! r[impl tui] r[impl tui-review]

use anyhow::{Context, Result};
use crossterm::event::{read, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use std::collections::HashMap;
use std::io::{Stdout, stdout};
use tui_tree_widget::{Tree, TreeItem, TreeState};
use uuid::Uuid;

use crate::models::{
    build_epic_counts, build_incoming_relationships_map, Issue, IssueType, RelationType, Status,
};
use crate::operations::Operations;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn status_span(status: &Status) -> Span<'static> {
    let (label, color) = match status {
        Status::Proposed => ("PROPOSED", Color::White),
        Status::Todo => ("TODO", Color::Yellow),
        Status::InProgress => ("IN PROGRESS", Color::Blue),
        Status::Done => ("DONE", Color::Green),
        Status::Blocked => ("BLOCKED", Color::Red),
    };
    Span::styled(format!("[{label}] "), Style::new().fg(color))
}

fn type_span(issue_type: &IssueType) -> Span<'static> {
    let (label, color) = match issue_type {
        IssueType::Bug => ("bug", Color::Red),
        IssueType::UxProblem => ("ux", Color::Magenta),
        IssueType::Usecase => ("uc", Color::Cyan),
        IssueType::Design => ("des", Color::Blue),
        IssueType::Validation => ("val", Color::Green),
        IssueType::Epic => ("epic", Color::Yellow),
        IssueType::Milestone => ("mile", Color::Yellow),
    };
    Span::styled(format!("[{label}] "), Style::new().fg(color))
}

/// Users read from the top: actionable statuses first, done last.
fn status_rank(status: &Status) -> u8 {
    match status {
        Status::Proposed => 0,
        Status::Todo => 1,
        Status::InProgress => 2,
        Status::Blocked => 3,
        Status::Done => 4,
    }
}

/// Within a status: bugs first, then designs, then the rest.
fn type_rank(issue_type: &IssueType) -> u8 {
    match issue_type {
        IssueType::Bug => 0,
        IssueType::Design => 1,
        _ => 2,
    }
}

fn sort_key(issue: &Issue) -> (u8, u8, std::cmp::Reverse<chrono::DateTime<chrono::Utc>>) {
    (
        status_rank(&issue.status),
        type_rank(&issue.issue_type),
        std::cmp::Reverse(issue.updated_at),
    )
}

fn epic_line(epic: &Issue, counts: &HashMap<Uuid, (usize, usize)>) -> Line<'static> {
    let (total, done) = counts.get(&epic.id).copied().unwrap_or((0, 0));
    Line::from(vec![
        Span::styled(
            epic.title.clone(),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" ({done}/{total})")),
    ])
}

fn leaf_line(issue: &Issue) -> Line<'static> {
    Line::from(vec![
        type_span(&issue.issue_type),
        status_span(&issue.status),
        Span::raw(issue.title.clone()),
    ])
}

fn is_orphan(issue: &Issue) -> bool {
    issue.issue_type.can_be_part_of_epic()
        && !issue
            .relationships
            .iter()
            .any(|rel| rel.relation_type == RelationType::PartOf)
}

/// Build the browser tree: epics (children by sort_key, with progress) then
/// orphaned pebbles. With a status filter, a flat leaf list of matching
/// issues instead. Node identifiers are issue UUID strings.
pub fn tree_items(
    issues: &[Issue],
    counts: &HashMap<Uuid, (usize, usize)>,
    filter: Option<Status>,
) -> std::io::Result<Vec<TreeItem<'static, String>>> {
    if let Some(status) = filter {
        let mut matching: Vec<&Issue> = issues.iter().filter(|i| i.status == status).collect();
        matching.sort_by_key(|i| sort_key(i));
        return Ok(matching
            .into_iter()
            .map(|i| TreeItem::new_leaf(i.id.to_string(), leaf_line(i)))
            .collect());
    }

    let mut epics: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.issue_type == IssueType::Epic)
        .collect();
    epics.sort_by_key(|i| std::cmp::Reverse(i.updated_at));

    let mut items = Vec::new();
    for epic in &epics {
        let mut children: Vec<&Issue> = issues
            .iter()
            .filter(|i| {
                i.issue_type != IssueType::Epic
                    && i.relationships
                        .iter()
                        .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == epic.id)
            })
            .collect();
        children.sort_by_key(|i| sort_key(i));
        let children: Vec<TreeItem<'static, String>> =
            children.into_iter().map(|c| TreeItem::new_leaf(c.id.to_string(), leaf_line(c))).collect();
        items.push(TreeItem::new(
            epic.id.to_string(),
            epic_line(epic, counts),
            children,
        )?);
    }

    let mut orphans: Vec<&Issue> = issues.iter().filter(|i| is_orphan(i)).collect();
    orphans.sort_by_key(|i| sort_key(i));
    for orphan in orphans {
        items.push(TreeItem::new_leaf(orphan.id.to_string(), leaf_line(orphan)));
    }
    Ok(items)
}

/// Detail-pane lines for the highlighted issue; mirrors the `show` output.
pub fn detail_lines(
    issue: &Issue,
    related_titles: &HashMap<Uuid, String>,
    incoming: &HashMap<Uuid, Vec<(Uuid, String, RelationType)>>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("ID: ", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(issue.id.to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Type: ", Style::new().add_modifier(Modifier::BOLD)),
        type_span(&issue.issue_type),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::new().add_modifier(Modifier::BOLD)),
        status_span(&issue.status),
    ]));
    if let Some(desc) = &issue.description {
        for (i, part) in desc.split('\n').enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("Description: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(part.to_string()),
                ]));
            } else {
                lines.push(Line::raw(format!("  {part}")));
            }
        }
    }
    if !issue.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Tags: ", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(issue.tags.join(", ")),
        ]));
    }
    if !issue.relationships.is_empty() {
        lines.push(Line::from(Span::styled(
            "Relationships:",
            Style::new().add_modifier(Modifier::BOLD),
        )));
        for rel in &issue.relationships {
            let rel_str = match rel.relation_type {
                RelationType::PartOf => "part of",
                RelationType::RequiredFor => "required for",
                RelationType::ScheduledBefore => "scheduled before",
            };
            let id_display = rel.target_id.to_string()[..8].to_string();
            let line = match related_titles.get(&rel.target_id) {
                Some(title) => format!("  {rel_str} {id_display} ({title})"),
                None => format!("  {rel_str} {id_display}"),
            };
            lines.push(Line::raw(line));
        }
    }
    if let Some(sources) = incoming.get(&issue.id) {
        if !sources.is_empty() {
            lines.push(Line::from(Span::styled(
                "Referenced by:",
                Style::new().add_modifier(Modifier::BOLD),
            )));
            for (source_id, source_title, rel_type) in sources {
                let rel_str = match rel_type {
                    RelationType::PartOf => "has part",
                    RelationType::RequiredFor => "requires",
                    RelationType::ScheduledBefore => "scheduled after",
                };
                let id_display = source_id.to_string()[..8].to_string();
                lines.push(Line::raw(format!("  {rel_str} {id_display} ({source_title})")));
            }
        }
    }
    lines.push(Line::from(format!(
        "Updated: {}",
        issue.updated_at.format("%Y-%m-%d %H:%M")
    )));
    lines
}

// ---------------------------------------------------------------------------
// App state (pure; mutations go through Operations)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Browser,
    Help,
}

pub struct App {
    pub mode: Mode,
    pub filter: Option<Status>,
    pub issues: Vec<Issue>,
    pub related_titles: HashMap<Uuid, String>,
    pub incoming: HashMap<Uuid, Vec<(Uuid, String, RelationType)>>,
    pub items: Vec<TreeItem<'static, String>>,
    pub tree_state: TreeState<String>,
    pub message: Option<String>,
    pub confirm_delete: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        App {
            mode: Mode::Browser,
            filter: None,
            issues: Vec::new(),
            related_titles: HashMap::new(),
            incoming: HashMap::new(),
            items: Vec::new(),
            tree_state: TreeState::default(),
            message: None,
            confirm_delete: false,
            should_quit: false,
        }
    }

    pub fn rebuild_items(&mut self) {
        let counts = build_epic_counts(&self.issues);
        self.items = tree_items(&self.issues, &counts, self.filter)
            .expect("unique identifiers");
    }

    pub fn reload(&mut self, ops: &Operations) -> Result<()> {
        let issues = ops.list(None, None)?;
        self.related_titles = issues
            .iter()
            .map(|i| (i.id, i.title.clone()))
            .collect();
        self.incoming = build_incoming_relationships_map(&issues)
            .into_iter()
            .map(|(uuid, rels)| {
                (
                    uuid,
                    rels.into_iter()
                        .map(|r| (r.source_id, r.source_title, r.relation_type))
                        .collect(),
                )
            })
            .collect();
        self.issues = issues;
        self.rebuild_items();
        if !self.selection_visible() {
            self.reset_selection_to_first();
        }
        Ok(())
    }

    /// The selection must point at an issue that is actually visible in the
    /// current item list (under a status filter, stale selections would let
    /// a/d act on filtered-out issues).
    fn selection_visible(&self) -> bool {
        let Some(uuid) = self.tree_state.selected().last().and_then(|s| Uuid::parse_str(s).ok())
        else {
            return false;
        };
        match self.filter {
            Some(_) => self.items.iter().any(|it| it.identifier().as_str() == uuid.to_string()),
            None => self.issues.iter().any(|i| i.id == uuid),
        }
    }

    fn reset_selection_to_first(&mut self) {
        match self.items.first() {
            Some(first) => {
                self.tree_state.select(vec![first.identifier().clone()]);
            }
            None => {
                self.tree_state.select(Vec::new());
            }
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        let sel = self.tree_state.selected();
        let uuid = Uuid::parse_str(sel.last()?).ok()?;
        self.issues.iter().find(|i| i.id == uuid)
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HELP_LINES: &[&str] = &[
    "j/k or Up/Down  move",
    "Right           unfold (or move to first child)",
    "Left            fold (or move to parent)",
    "Enter or f      toggle fold",
    "r               toggle the proposed-issues filter",
    "a               approve selected (in proposed filter)",
    "d then y        close selected = delete (in filter)",
    "?               toggle this help",
    "q               quit",
];

pub fn render_app(f: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::Browser => render_browser(f, app),
        Mode::Help => render_help(f, app),
    }
}

fn message_paragraph(app: &App) -> Paragraph<'static> {
    let text = app.message.clone().unwrap_or_default();
    Paragraph::new(Line::from(Span::styled(
        text,
        Style::new().fg(Color::DarkGray),
    )))
}

fn tree_title(app: &App) -> String {
    match app.filter {
        Some(Status::Proposed) => {
            "pebbles - proposed (a approve, d+y close, Esc/r clear)".to_string()
        }
        _ => "pebbles".to_string(),
    }
}

fn render_browser(f: &mut Frame, app: &mut App) {
    let [main_area, message_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .areas(main_area);

    let tree = Tree::new(&app.items)
        .expect("unique identifiers")
        .block(Block::bordered().title(tree_title(app)))
        .highlight_symbol("> ")
        .highlight_style(Style::new().bg(Color::Rgb(60, 60, 60)));
    f.render_stateful_widget(tree, tree_area, &mut app.tree_state);

    let detail_lines = match app.selected_issue() {
        Some(issue) => detail_lines(issue, &app.related_titles, &app.incoming),
        None => vec![Line::raw("No selection")],
    };
    let title = app
        .selected_issue()
        .map(|i| i.title.clone())
        .unwrap_or_else(|| "detail".to_string());
    let detail = Paragraph::new(detail_lines)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .block(Block::bordered().title(title));
    f.render_widget(detail, detail_area);
    f.render_widget(message_paragraph(app), message_area);
}

fn render_help(f: &mut Frame, app: &mut App) {
    render_browser(f, app);
    let area = centered_rect(f.area(), 60, HELP_LINES.len() as u16 + 2);
    // Opaque overlay: clear the underlying frame first, solid background.
    f.render_widget(Clear, area);
    let lines: Vec<Line> = HELP_LINES.iter().map(|l| Line::raw(*l)).collect();
    let block = Block::bordered()
        .title("help (any key to close)")
        .style(Style::new().bg(Color::Black));
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(Color::Black)).block(block), area);
}

fn centered_rect(area: Rect, pct_x: u16, height: u16) -> Rect {
    let w = area.width * pct_x / 100;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect { x, y, width: w, height: height.min(area.height) }
}

// ---------------------------------------------------------------------------
// Imperative shell: terminal setup, event loop, mutations
// ---------------------------------------------------------------------------

pub fn run(ops: &Operations) -> Result<()> {
    let mut app = App::new();
    app.reload(ops)?;

    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app, ops);

    disable_raw_mode()?;
    let stdout = terminal.backend_mut();
    stdout.execute(LeaveAlternateScreen)?;
    stdout.execute(crossterm::cursor::Show)?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    ops: &Operations,
) -> Result<()> {
    loop {
        // TreeState.select_first() needs identifiers from a previous render,
        // so re-select when empty and let the next draw pick it up.
        if app.tree_state.selected().is_empty() && !app.items.is_empty() {
            app.tree_state.select_first();
        }
        terminal.draw(|f| render_app(f, app))?;
        if let Event::Key(key) = read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.mode {
                Mode::Browser => browser_key(app, key.code, ops)?,
                Mode::Help => app.mode = Mode::Browser,
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn browser_key(app: &mut App, code: KeyCode, ops: &Operations) -> Result<()> {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree_state.key_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree_state.key_up();
        }
        KeyCode::Right => {
            app.tree_state.key_right();
        }
        KeyCode::Left => {
            app.tree_state.key_left();
        }
        KeyCode::Enter | KeyCode::Char('f') => {
            app.tree_state.toggle_selected();
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('r') | KeyCode::Esc => {
            app.filter = if app.filter.is_some() { None } else { Some(Status::Proposed) };
            app.confirm_delete = false;
            app.rebuild_items();
            // Select the first item of the (possibly filtered) list so the
            // detail pane and a/d act on what is visible.
            app.reset_selection_to_first();
        }
        KeyCode::Char('a') => {
            if app.filter.is_some() {
                approve_selected(app, ops)?;
            } else {
                app.message = Some("Press r to enter the proposed filter to approve".to_string());
            }
        }
        KeyCode::Char('d') => {
            if app.filter.is_some() {
                if app.selected_issue().is_some() {
                    app.confirm_delete = true;
                    app.message =
                        Some("Press y to confirm closing (delete)".to_string());
                } else {
                    app.message = Some("No selection".to_string());
                }
            } else {
                app.message = Some("Press r to enter the proposed filter to close".to_string());
            }
        }
        KeyCode::Char('y') if app.confirm_delete => {
            app.confirm_delete = false;
            delete_selected(app, ops)?;
        }
        _ => app.confirm_delete = false,
    }
    Ok(())
}

fn approve_selected(app: &mut App, ops: &Operations) -> Result<()> {
    let Some(issue) = app.selected_issue() else {
        app.message = Some("No selection".to_string());
        return Ok(());
    };
    if issue.status != Status::Proposed {
        app.message = Some("Only proposed issues can be approved".to_string());
        return Ok(());
    }
    let uuid = issue.id;
    match ops.approve(&uuid.to_string()) {
        Ok(()) => {
            app.reload(ops)?;
            app.message = Some(format!("\u{2713} Approved {}", ops.appropriate_length_id(&uuid)?));
        }
        Err(e) => app.message = Some(format!("Error: {e}")),
    }
    Ok(())
}

fn delete_selected(app: &mut App, ops: &Operations) -> Result<()> {
    let Some(issue) = app.selected_issue() else {
        app.message = Some("No selection".to_string());
        return Ok(());
    };
    let uuid = issue.id;
    match ops.delete(&uuid.to_string(), false) {
        Ok(short) => {
            app.reload(ops)?;
            app.message = Some(format!("\u{2713} Deleted {short}"));
        }
        Err(e) => app.message = Some(format!("Error: {e}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Relationship;
    use ratatui::backend::TestBackend;

    fn issue(uuid_suffix: u128, title: &str, status: Status, issue_type: IssueType) -> Issue {
        let mut i = Issue::new(title.to_string(), None, issue_type);
        i.id = Uuid::from_u128(uuid_suffix);
        i.status = status;
        i
    }

    fn rel(source: &mut Issue, relation_type: RelationType, target: Uuid) {
        source.relationships.push(Relationship { relation_type, target_id: target });
    }

    fn fixture() -> Vec<Issue> {
        let epic_id = Uuid::from_u128(0x4444);
        let child_done = issue(0x1111, "Done child", Status::Done, IssueType::Usecase);
        let mut child_todo = issue(0x2222, "Todo child", Status::Todo, IssueType::Bug);
        rel(&mut child_todo, RelationType::PartOf, epic_id);
        let mut done_child2 = child_done;
        rel(&mut done_child2, RelationType::PartOf, epic_id);
        let epic = issue(0x4444, "The epic", Status::Todo, IssueType::Epic);
        let orphan = issue(0x3333, "Orphan", Status::Proposed, IssueType::Design);
        vec![child_todo, done_child2, epic, orphan]
    }

    fn make_app(issues: &[Issue], filter: Option<Status>) -> App {
        let counts = build_epic_counts(issues);
        let items = tree_items(issues, &counts, filter).unwrap();
        let incoming = build_incoming_relationships_map(issues)
            .into_iter()
            .map(|(uuid, rels)| {
                (
                    uuid,
                    rels.into_iter()
                        .map(|r| (r.source_id, r.source_title, r.relation_type))
                        .collect(),
                )
            })
            .collect();
        let related: HashMap<Uuid, String> = issues.iter().map(|i| (i.id, i.title.clone())).collect();
        let mut app = App {
            issues: issues.to_vec(),
            related_titles: related,
            incoming,
            items,
            filter,
            ..App::new()
        };
        app.tree_state.select_first();
        app
    }

    fn draw_once(app: &mut App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render_app(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buf.area.width {
                    line.push_str(buf[(x, y)].symbol());
                }
                line.trim_end().to_string()
            })
            .collect()
    }

    /// Draw, then settle the selection (select_first needs a prior render's
    /// identifiers, mirroring the event loop's post-draw re-select).
    fn draw(app: &mut App) -> Vec<String> {
        draw_once(app);
        if app.tree_state.selected().is_empty() && !app.items.is_empty() {
            app.tree_state.select_first();
        }
        draw_once(app)
    }

    fn row_of(rows: &[String], needle: &str) -> Option<usize> {
        rows.iter().position(|r| r.contains(needle))
    }

    #[test]
    fn tree_groups_epics_with_children_then_orphans() {
        let issues = fixture();
        let counts = build_epic_counts(&issues);
        let items = tree_items(&issues, &counts, None).unwrap();
        assert_eq!(items.len(), 2, "one epic + one orphan");
        let epic = issues.iter().find(|i| i.issue_type == IssueType::Epic).unwrap();
        assert!(epic_line(epic, &counts).to_string().contains("The epic"));
        assert_eq!(items[0].children().len(), 2, "epic has both children");
        let orphan = issues.iter().find(|i| is_orphan(i)).unwrap();
        assert!(leaf_line(orphan).to_string().contains("Orphan"));
    }

    #[test]
    fn children_sort_proposed_before_todo_before_done_then_bug_before_rest() {
        // Within an epic: todo-bug before done-usecase; a proposed child would
        // come first. Bug ranks before non-bug within the same status.
        let epic_id = Uuid::from_u128(0x4444);
        let mut bug_design_done = issue(0x7777, "Done design", Status::Done, IssueType::Design);
        rel(&mut bug_design_done, RelationType::PartOf, epic_id);
        let mut issues = fixture();
        issues.push(bug_design_done);
        let counts = build_epic_counts(&issues);
        let items = tree_items(&issues, &counts, None).unwrap();
        assert_eq!(items[0].children().len(), 3);
        // Render and assert row order: todo-bug < done-design < done-usecase.
        let mut app = make_app(&issues, None);
        app.tree_state.open(vec![Uuid::from_u128(0x4444).to_string()]);
        let rows = draw(&mut app);
        let todo_bug = row_of(&rows, "Todo child").unwrap();
        let done_design = row_of(&rows, "Done design").unwrap();
        let done_usecase = row_of(&rows, "Done child").unwrap();
        assert!(todo_bug < done_design, "{rows:?}");
        assert!(done_design < done_usecase, "{rows:?}");
    }

    #[test]
    fn proposed_filter_flat_list_sorted_bug_before_design() {
        let mut bug = issue(0x6666, "Bug fix", Status::Proposed, IssueType::Bug);
        bug.updated_at = bug.updated_at - chrono::Duration::hours(1);
        let design = issue(0x8888, "Des fix", Status::Proposed, IssueType::Design);
        // design is newer, but bug still sorts first within the status.
        let issues = vec![design.clone(), bug.clone()];
        let counts = build_epic_counts(&issues);
        let items = tree_items(&issues, &counts, Some(Status::Proposed)).unwrap();
        assert_eq!(items.len(), 2, "flat list, no epics");
        let mut app = make_app(&issues, Some(Status::Proposed));
        let rows = draw(&mut app);
        let bug_row = row_of(&rows, "Bug fix").unwrap_or_else(|| panic!("rows: {rows:?}"));
        let design_row = row_of(&rows, "Des fix").unwrap_or_else(|| panic!("rows: {rows:?}"));
        assert!(bug_row < design_row, "{rows:?}");
        assert!(!rows.iter().any(|r| r.contains("The epic")), "filter hides epics");
    }

    #[test]
    fn detail_lines_split_multiline_description() {
        let mut i = issue(0x1234, "Titled", Status::Todo, IssueType::Bug);
        i.description = Some("first line\nsecond line".to_string());
        let lines = detail_lines(&i, &HashMap::new(), &HashMap::new())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        let first = lines.iter().position(|l| l.contains("Description: first line")).expect("first");
        assert!(lines[first + 1].contains("second line"), "{lines:?}");
    }

    #[test]
    fn detail_lines_show_relationships_both_ways() {
        let issues = fixture();
        let epic = issues.iter().find(|i| i.issue_type == IssueType::Epic).unwrap();
        let related: HashMap<Uuid, String> =
            issues.iter().map(|i| (i.id, i.title.clone())).collect();
        let incoming = build_incoming_relationships_map(&issues)
            .into_iter()
            .map(|(uuid, rels)| {
                (
                    uuid,
                    rels.into_iter()
                        .map(|r| (r.source_id, r.source_title, r.relation_type))
                        .collect(),
                )
            })
            .collect();
        let lines = detail_lines(epic, &related, &incoming).iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert!(lines.iter().any(|l| l.contains("ID:")), "has id line");
        assert!(lines.iter().any(|l| l.contains("part of") || l.contains("has part")), "has relationship");
    }

    #[test]
    fn browser_renders_tree_and_detail_on_test_backend() {
        let issues = fixture();
        let mut app = make_app(&issues, None);
        app.tree_state.open(vec![Uuid::from_u128(0x4444).to_string()]);
        // Long description: must wrap, not truncate at the pane edge.
        let epic = app.issues.iter_mut().find(|i| i.issue_type == IssueType::Epic).unwrap();
        epic.description =
            Some("alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo".to_string());
        app.rebuild_items();
        let rows = draw(&mut app);
        assert!(rows.iter().any(|r| r.contains("The epic")), "tree pane: {rows:?}");
        assert!(rows.iter().any(|r| r.contains("Todo child")), "tree pane child: {rows:?}");
        assert!(rows.iter().any(|r| r.contains("Orphan")), "tree pane orphan: {rows:?}");
        assert!(rows.iter().any(|r| r.contains("Status:")), "detail pane fields: {rows:?}");
        assert!(rows.iter().any(|r| r.contains("kilo")), "long description wraps: {rows:?}");
    }

    #[test]
    fn right_unfolds_and_left_folds() {
        let issues = fixture();
        let mut app = make_app(&issues, None);
        let tree_side = |rows: &[String]| {
            rows.iter()
                .map(|r| r.chars().take(31).collect::<String>())
                .collect::<Vec<_>>()
        };
        let rows_closed = tree_side(&draw(&mut app)); // also populates identifiers
        assert!(!rows_closed.iter().any(|r| r.contains("Todo child")), "starts folded");
        app.tree_state.key_right(); // unfold selected epic
        let rows_open = tree_side(&draw(&mut app));
        assert!(rows_open.iter().any(|r| r.contains("Todo child")), "Right unfolds: {rows_open:?}");
        app.tree_state.key_left(); // fold again
        let rows_folded = tree_side(&draw(&mut app));
        assert!(!rows_folded.iter().any(|r| r.contains("Todo child")), "Left folds");
    }

    #[test]
    fn help_overlay_is_opaque() {
        let issues = fixture();
        let mut app = make_app(&issues, None);
        app.mode = Mode::Help;
        let rows = draw(&mut app);
        let joined = rows.join("\n");
        assert!(joined.contains("help (any key to close)"), "help shown: {joined}");
        // Rows under the opaque overlay must not leak the browser underneath
        // (the overlay spans the vertical middle of the frame).
        let under: Vec<&String> = rows.iter().enumerate().filter(|(i, _)| (7..=16).contains(i)).map(|(_, r)| r).collect();
        assert!(under.iter().any(|r| r.contains("move")), "help body: {under:?}");
        assert!(!under.iter().any(|r| r.contains("No selection") || r.contains("Status:")), "browser leaked through: {under:?}");
    }
}
