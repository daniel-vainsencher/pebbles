//! Interactive TUI: two-pane browser (epic/pebble tree + detail) and a
//! review-queue mode. Pure helpers are separated from the terminal shell so
//! they are unit-testable without I/O (ARCHITECTURE.md Testing Strategy).
//!
//! Design: pebble 94223b04. Widget: tui-tree-widget (pebble: tui-tree-widget eval). E2E: termlens.
//! r[impl tui] r[impl tui-review]

use anyhow::{Context, Result};
use crossterm::event::{read, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, stdout};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;
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

fn epic_line(epic: &Issue, counts: &HashMap<Uuid, (usize, usize)>) -> Line<'static> {
    let (total, done) = counts
        .get(&epic.id)
        .copied()
        .unwrap_or((0, 0));
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

/// Build the browser tree: epics (children newest-first, with progress) then
/// orphaned pebbles. Node identifiers are issue UUID strings.
pub fn tree_items(
    issues: &[Issue],
    counts: &HashMap<Uuid, (usize, usize)>,
) -> std::io::Result<Vec<TreeItem<'static, String>>> {
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
        children.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
        let children: Vec<TreeItem<'static, String>> =
            children.into_iter().map(|c| TreeItem::new_leaf(c.id.to_string(), leaf_line(c))).collect();
        items.push(TreeItem::new(
            epic.id.to_string(),
            epic_line(epic, counts),
            children,
        )?);
    }

    let mut orphans: Vec<&Issue> = issues.iter().filter(|i| is_orphan(i)).collect();
    orphans.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    for orphan in orphans {
        items.push(TreeItem::new_leaf(orphan.id.to_string(), leaf_line(orphan)));
    }
    Ok(items)
}

/// The review queue: proposed issues, newest first.
pub fn review_entries(issues: &[Issue]) -> Vec<&Issue> {
    let mut proposed: Vec<&Issue> = issues.iter().filter(|i| i.status == Status::Proposed).collect();
    proposed.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    proposed
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
        lines.push(Line::from(vec![
            Span::styled("Description: ", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(desc.clone()),
        ]));
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
    Review,
    Help,
}

pub struct App {
    pub mode: Mode,
    pub issues: Vec<Issue>,
    pub related_titles: HashMap<Uuid, String>,
    pub incoming: HashMap<Uuid, Vec<(Uuid, String, RelationType)>>,
    pub items: Vec<TreeItem<'static, String>>,
    pub tree_state: TreeState<String>,
    pub review_sel: usize,
    pub message: Option<String>,
    pub confirm_delete: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        App {
            mode: Mode::Browser,
            issues: Vec::new(),
            related_titles: HashMap::new(),
            incoming: HashMap::new(),
            items: Vec::new(),
            tree_state: TreeState::default(),
            review_sel: 0,
            message: None,
            confirm_delete: false,
            should_quit: false,
        }
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
        let counts = build_epic_counts(&issues);
        self.items = tree_items(&issues, &counts)?;
        self.issues = issues;
        if self.tree_state.selected().is_empty() || self.selected_issue().is_none() {
            self.tree_state.select_first();
        }
        Ok(())
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        let sel = self.tree_state.selected();
        let uuid = Uuid::parse_str(sel.last()?).ok()?;
        self.issues.iter().find(|i| i.id == uuid)
    }

    pub fn review_clamped(&self) -> usize {
        let n = review_entries(&self.issues).len();
        self.review_sel.min(n.saturating_sub(1))
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HELP_LINES: &[&str] = &[
    "j/k or Up/Down  move",
    "Enter or f      fold/unfold",
    "r               review queue (proposed issues)",
    "a               approve proposed issue",
    "d then y        close (delete) issue",
    "?               toggle this help",
    "q               quit",
];

pub fn render_app(f: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::Browser => render_browser(f, app),
        Mode::Review => render_review(f, app),
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

fn render_browser(f: &mut Frame, app: &mut App) {
    let [main_area, message_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .areas(main_area);

    let tree = Tree::new(&app.items)
        .expect("unique identifiers")
        .block(Block::bordered().title("pebbles"))
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
    let detail = Paragraph::new(detail_lines).block(Block::bordered().title(title));
    f.render_widget(detail, detail_area);
    f.render_widget(message_paragraph(app), message_area);
}

fn render_review(f: &mut Frame, app: &mut App) {
    let [main_area, message_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let entries = review_entries(&app.issues);
    let items: Vec<ListItem> = entries
        .iter()
        .map(|i| ListItem::new(leaf_line(i)))
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title("review - proposed issues (a approve, d close, Esc back)"))
        .highlight_symbol("> ")
        .highlight_style(Style::new().bg(Color::Rgb(60, 60, 60)));
    let mut state = ListState::default();
    state.select(Some(app.review_clamped()));
    f.render_stateful_widget(list, main_area, &mut state);
    f.render_widget(message_paragraph(app), message_area);
}

fn render_help(f: &mut Frame, app: &mut App) {
    render_browser(f, app);
    let area = centered_rect(f.area(), 60, HELP_LINES.len() as u16 + 2);
    let lines: Vec<Line> = HELP_LINES.iter().map(|l| Line::raw(*l)).collect();
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("help (any key to close)")),
        area,
    );
}

fn centered_rect(area: ratatui::layout::Rect, pct_x: u16, height: u16) -> ratatui::layout::Rect {
    let w = area.width * pct_x / 100;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    ratatui::layout::Rect { x, y, width: w, height: height.min(area.height) }
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
                Mode::Browser => browser_key(app, key.code)?,
                Mode::Review => review_key(app, key.code, ops)?,
                Mode::Help => app.mode = Mode::Browser,
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn browser_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree_state.key_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree_state.key_up();
        }
        KeyCode::Enter | KeyCode::Char('f') => {
            app.tree_state.toggle_selected();
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('r') => {
            app.mode = Mode::Review;
            app.review_sel = 0;
            app.confirm_delete = false;
        }
        _ => {}
    }
    Ok(())
}

fn review_key(app: &mut App, code: KeyCode, ops: &Operations) -> Result<()> {
    let entries = review_entries(&app.issues);
    let last = entries.len().saturating_sub(1);
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc | KeyCode::Char('r') => app.mode = Mode::Browser,
        KeyCode::Char('j') | KeyCode::Down => {
            app.review_sel = (app.review_clamped() + 1).min(last);
            app.confirm_delete = false;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.review_sel = app.review_clamped().saturating_sub(1);
            app.confirm_delete = false;
        }
        KeyCode::Char('a') => approve_selected(app, ops)?,
        KeyCode::Char('d') => {
            app.confirm_delete = !entries.is_empty();
            if !app.confirm_delete {
                app.message = Some("Nothing proposed".to_string());
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
    let entries = review_entries(&app.issues);
    let Some(issue) = entries.get(app.review_clamped()) else {
        app.message = Some("Nothing proposed".to_string());
        return Ok(());
    };
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
    let entries = review_entries(&app.issues);
    let Some(issue) = entries.get(app.review_clamped()) else {
        app.message = Some("Nothing proposed".to_string());
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

    #[test]
    fn tree_groups_epics_with_children_then_orphans() {
        let issues = fixture();
        let counts = build_epic_counts(&issues);
        let items = tree_items(&issues, &counts).unwrap();
        assert_eq!(items.len(), 2, "one epic + one orphan");
        let epic = issues.iter().find(|i| i.issue_type == IssueType::Epic).unwrap();
        assert!(epic_line(epic, &counts).to_string().contains("The epic"));
        assert_eq!(items[0].children().len(), 2, "epic has both children");
        let orphan = issues.iter().find(|i| is_orphan(i)).unwrap();
        assert!(
            leaf_line(orphan).to_string().contains("Orphan"),
            "orphan is a top-level leaf"
        );
    }

    #[test]
    fn epic_line_shows_progress() {
        let issues = fixture();
        let counts = build_epic_counts(&issues);
        let epic = issues.iter().find(|i| i.issue_type == IssueType::Epic).unwrap();
        let line = epic_line(epic, &counts).to_string();
        assert!(line.contains("The epic (1/2)"), "got: {line}");
    }

    #[test]
    fn review_entries_filter_proposed_only() {
        let issues = fixture();
        let entries = review_entries(&issues);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Orphan");
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
        let counts = build_epic_counts(&issues);
        let items = tree_items(&issues, &counts).unwrap();
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
        let related: HashMap<Uuid, String> = issues.iter().map(|i| (i.id, i.title.clone())).collect();
        let mut app = App {
            issues: issues.clone(),
            related_titles: related,
            incoming,
            items,
            ..App::new()
        };
        app.tree_state.select(vec![issues[0].id.to_string()]); // first issue (child_todo)
        app.tree_state.open(vec![Uuid::from_u128(0x4444).to_string()]);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render_app(f, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .clone()
            .content
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(text.contains("The epic"), "tree pane: {text}");
        assert!(text.contains("Todo child"), "tree pane child: {text}");
        assert!(text.contains("Orphan"), "tree pane orphan: {text}");
        assert!(text.contains("Todo child"), "detail pane shows selection: {text}");
        assert!(text.contains("Status:"), "detail pane fields: {text}");
    }
}
