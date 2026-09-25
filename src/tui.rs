//! Interactive TUI: two-pane browser (epic/pebble tree + detail) with a
//! proposed-issues review filter and a quick search line (`/`). Pure
//! helpers are separated from the terminal shell so they are unit-testable
//! without I/O (ARCHITECTURE.md Testing Strategy).
//!
//! Design: pebble 94223b04. Widget: tui-tree-widget (pebble: tui-tree-widget eval). E2E: termlens.
//! r[impl tui] r[impl tui-review] r[impl tui-search] r[impl tui-background]

use anyhow::{Context, Result};
use crossterm::event::{read, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use ratatui_textarea::{Input, TextArea, WrapMode};
use std::collections::HashMap;
use std::io::{stdout, Stdout};
use std::path::{Path, PathBuf};
use tui_tree_widget::{Tree, TreeItem, TreeState};
use uuid::Uuid;

use crate::models::{
    build_epic_counts, build_incoming_relationships_map, Issue, IssueType, RelationType, Status,
};
use crate::operations::Operations;
use crate::storage::LoadWarning;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Fold load/write warnings into the TUI's one-line message row
/// (r[load-warnings-as-data]): `"<message>; WARNING: <first> (+N more)"`.
/// The message row is a single line, so only the first warning's text is
/// shown; the detail pane lists every warning in full.
fn fold_warnings(message: String, warnings: &[LoadWarning]) -> String {
    if warnings.is_empty() {
        return message;
    }
    let first = &warnings[0].message;
    let more = warnings.len() - 1;
    if more > 0 {
        format!("{}; WARNING: {} (+{} more)", message, first, more)
    } else {
        format!("{}; WARNING: {}", message, first)
    }
}

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

// ---------------------------------------------------------------------------
// Search query (r[tui-search])
// ---------------------------------------------------------------------------

/// A compiled search term. Terms combine with AND: an issue is visible only
/// when every term in the query matches.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryTerm {
    /// Bare alphanumerics: case-insensitive exact-substring match against the
    /// short id, title, or description; the bool negates (`!pane` = no pane).
    Text(String, bool),
    /// `s:` status filter; the bool negates (`s:!d` = anything but done).
    Status(Status, bool),
    /// `t:` type filter; the bool negates (`t:!bug` = anything but bugs).
    Type(IssueType, bool),
    /// Unparseable spec (`s:x`, `t:9`, bare `!`): matches nothing.
    Dead,
}

/// A compiled search query: the AND of its terms.
#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub terms: Vec<QueryTerm>,
}

/// Parse the status code after `s:`: one letter (p/t/i/b/d) or a full name.
fn parse_status_code(s: &str) -> Option<Status> {
    match s.to_lowercase().as_str() {
        "p" | "proposed" => Some(Status::Proposed),
        "t" | "todo" => Some(Status::Todo),
        "i" | "in_progress" | "inprogress" | "in-progress" => Some(Status::InProgress),
        "b" | "blocked" => Some(Status::Blocked),
        "d" | "done" => Some(Status::Done),
        _ => None,
    }
}

/// Parse the type code after `t:`: a short code or a full name.
fn parse_type_code(s: &str) -> Option<IssueType> {
    match s.to_lowercase().as_str() {
        "bug" => Some(IssueType::Bug),
        "ux" | "ux_problem" | "uxproblem" | "ux-problem" => Some(IssueType::UxProblem),
        "uc" | "usecase" | "use-case" | "use_case" => Some(IssueType::Usecase),
        "des" | "design" => Some(IssueType::Design),
        "val" | "validation" => Some(IssueType::Validation),
        "epic" => Some(IssueType::Epic),
        "mile" | "milestone" => Some(IssueType::Milestone),
        _ => None,
    }
}

/// Split a `s:`/`t:` value into an optional `!` negation and the code.
fn split_negation(value: &str) -> (bool, &str) {
    if let Some(rest) = value.strip_prefix("!") {
        (true, rest)
    } else {
        (false, value)
    }
}

/// Compile a raw search line into its AND-ed terms (r[tui-search]).
pub fn parse_search_query(raw: &str) -> SearchQuery {
    let mut terms: Vec<QueryTerm> = Vec::new();
    for token in raw.split_whitespace() {
        let lower = token.to_lowercase();
        if let Some(code) = lower.strip_prefix("s:") {
            let (negated, code) = split_negation(code);
            match parse_status_code(code) {
                Some(status) => terms.push(QueryTerm::Status(status, negated)),
                None => terms.push(QueryTerm::Dead),
            }
        } else if let Some(code) = lower.strip_prefix("t:") {
            let (negated, code) = split_negation(code);
            match parse_type_code(code) {
                Some(issue_type) => terms.push(QueryTerm::Type(issue_type, negated)),
                None => terms.push(QueryTerm::Dead),
            }
        } else if let Some(rest) = token.strip_prefix("!") {
            // A bare `!` with nothing after it matches nothing (like s:x).
            if rest.is_empty() {
                terms.push(QueryTerm::Dead);
            } else {
                terms.push(QueryTerm::Text(rest.to_string(), true));
            }
        } else {
            terms.push(QueryTerm::Text(token.to_string(), false));
        }
    }
    SearchQuery { terms }
}

/// Case-insensitive exact-substring match of `term` inside `text`: "pan"
/// finds "pane"; "pane edit" is two independent terms which the title
/// "Edit pane" contains in either order.
fn text_matches(term: &str, text: &str) -> bool {
    !term.is_empty() && text.to_lowercase().contains(term.to_lowercase().as_str())
}

/// Case-insensitive exact-substring match against the 8-char short id.
fn short_id_matches(short_id: &str, term: &str) -> bool {
    !term.is_empty()
        && short_id
            .to_lowercase()
            .contains(term.to_lowercase().as_str())
}

fn term_matches(issue: &Issue, term: &QueryTerm) -> bool {
    match term {
        QueryTerm::Text(text, negated) => {
            let description_matches = if let Some(d) = &issue.description {
                text_matches(text.as_str(), d.as_str())
            } else {
                false
            };
            let hit = !text.is_empty()
                && (short_id_matches(&issue.short_id(), text.as_str())
                    || text_matches(text.as_str(), issue.title.as_str())
                    || description_matches);
            if *negated {
                !hit
            } else {
                hit
            }
        }
        QueryTerm::Status(status, negated) => {
            let equal = issue.status == *status;
            if *negated {
                !equal
            } else {
                equal
            }
        }
        QueryTerm::Type(issue_type, negated) => {
            let equal = issue.issue_type == *issue_type;
            if *negated {
                !equal
            } else {
                equal
            }
        }
        QueryTerm::Dead => false,
    }
}

/// Does `issue` pass the whole query? Every term must match (AND).
pub fn issue_matches_query(issue: &Issue, query: &SearchQuery) -> bool {
    query.terms.iter().all(|term| term_matches(issue, term))
}

/// Build the browser tree: epics (children by sort_key, with progress) then
/// orphaned pebbles. With a status filter and/or a non-empty search query, a
/// flat leaf list of the matching issues instead. Node identifiers are issue
/// UUID strings.
pub fn tree_items(
    issues: &[Issue],
    counts: &HashMap<Uuid, (usize, usize)>,
    filter: Option<Status>,
    search: &str,
) -> std::io::Result<Vec<TreeItem<'static, String>>> {
    let query = parse_search_query(search);
    let search_active = !query.terms.is_empty();
    if filter.is_some() || search_active {
        let mut matching: Vec<&Issue>;
        if let Some(status) = filter {
            matching = issues
                .iter()
                .filter(|i| i.status == status && issue_matches_query(i, &query))
                .collect();
        } else {
            matching = issues
                .iter()
                .filter(|i| issue_matches_query(i, &query))
                .collect();
        }
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
                    && i.relationships.iter().any(|rel| {
                        rel.relation_type == RelationType::PartOf && rel.target_id == epic.id
                    })
            })
            .collect();
        children.sort_by_key(|i| sort_key(i));
        let children: Vec<TreeItem<'static, String>> = children
            .into_iter()
            .map(|c| TreeItem::new_leaf(c.id.to_string(), leaf_line(c)))
            .collect();
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
                lines.push(Line::raw(format!(
                    "  {rel_str} {id_display} ({source_title})"
                )));
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

/// One project's loaded data (r[project-registry]).
#[derive(Clone, Debug)]
pub struct ProjectData {
    pub root: PathBuf,
    pub name: String,
    pub issues: Vec<Issue>,
    pub related_titles: HashMap<Uuid, String>,
    pub incoming: HashMap<Uuid, Vec<(Uuid, String, RelationType)>>,
    /// Load warnings for this project's issue set, folded into the message
    /// and detail lines instead of raw stderr (r[load-warnings-as-data]).
    pub warnings: Vec<LoadWarning>,
    pub index_mtime: Option<std::time::SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Browser,
    /// The inline edit pane owns the keyboard (r[tui-edit]).
    Edit,
    Help,
}

/// Which browser pane currently owns the keyboard (Tab / BackTab cycle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    /// The left tree: move/select, `r` review filter, `a`/`d` mutations.
    Tree,
    /// The search line at the bottom: keys edit the query (`/`).
    Search,
    /// The right detail pane: `j`/`k` scroll, `e` opens the edit pane,
    /// `Esc` returns to the tree.
    Detail,
}

/// A field of the inline edit pane, in display order (r[tui-edit]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditField {
    Title,
    Description,
    Type,
    Status,
}

/// Pending state for the inline edit pane: which field is highlighted, the
/// text buffer, and the pending Type/Status values that `Enter` applies
/// through the operations layer (r[tui-edit]).
#[derive(Clone, Debug)]
pub struct EditState {
    /// The issue the pane edits, pinned so tree reloads cannot redirect it.
    pub issue_id: Uuid,
    /// The highlighted field.
    pub field: EditField,
    /// True while editing a typed field (Title/Description); cycled fields
    /// (Type/Status) apply directly from navigation, so they never arm.
    /// While false, `j`/`k` move between fields and `Esc` closes the pane.
    pub editing: bool,
    /// The editing widget for Title/Description, pre-filled from the live
    /// issue. While not editing it holds the last committed text, so
    /// `text()` is the pane's committed value for the active typed field
    /// (r[tui-edit], peg d31)
    textarea: TextArea<'static>,
    /// Pending type; `Enter` applies it via `Operations::convert`.
    pub pending_type: IssueType,
    /// Pending status; `Enter` applies it via `Operations::update_status`.
    pub pending_status: Status,
}

impl EditState {
    fn for_issue(issue: &Issue) -> Self {
        EditState {
            issue_id: issue.id,
            field: EditField::Title,
            editing: false,
            textarea: TextArea::default(),
            pending_type: issue.issue_type.clone(),
            pending_status: issue.status.clone(),
        }
    }

    /// The buffer text, lines joined by `\n` (matches the store's format
    /// and the pre-adoption `EditBuffer::to_string` contract).
    fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// (row, col) of the widget cursor, in characters from the line head.
    fn cursor(&self) -> (usize, usize) {
        let cursor = self.textarea.cursor();
        (cursor.0 as usize, cursor.1 as usize)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }
}

/// The label shown for the edited field while its widget is active.
fn field_label(field: EditField) -> &'static str {
    match field {
        EditField::Title => "Title",
        EditField::Description => "Description",
        EditField::Type => "Type",
        EditField::Status => "Status",
    }
}

/// The edited textarea's content height in terminal rows: the number of
/// wrapped display lines of `lines` at `width` columns (`WrapMode::WordOrGlyph`,
/// matching the widget's render), never below 1 so the empty edit still
/// reserves a slot. The pane reserves exactly this height for the widget
/// and lays the other fields around it (rect-based layout, d31).
fn edit_area_height(textarea: &TextArea<'_>, width: usize) -> usize {
    let rows = wrapped_row_count(textarea.lines(), width, textarea.tab_length());
    rows.max(1)
}

/// Display width of one logical line: tabs pad to `tab_len`, wide
/// east-asian characters (common ranges) take two columns, everything else
/// one. A simplification of `unicode_width` (which the widget uses) for the
/// pure height estimate above.
fn display_width(line: &str, tab_len: u8) -> usize {
    let mut cols = 0;
    for c in line.chars() {
        match c {
            '\t' => cols += tab_len as usize,
            c if is_wide_char(c) => cols += 2,
            _ => cols += 1,
        }
    }
    cols
}

/// East-asian wide character ranges (CJK, fullwidth forms, kana, hangul).
fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    (0x1100..=0x115f).contains(&cp)
        || (0x2e80..=0x303e).contains(&cp)
        || (0x3040..=0x33ff).contains(&cp)
        || (0x3400..=0x4dbf).contains(&cp)
        || (0x4e00..=0x9fff).contains(&cp)
        || (0xa000..=0xa4cf).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp)
        || (0xf900..=0xfaff).contains(&cp)
        || (0xfe30..=0xfe4f).contains(&cp)
        || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
}

/// Greedy word-or-glyph wrap estimate: each logical line breaks into words,
/// a word wider than the area occupies ceil(cols/width) rows, and empty
/// lines count one row. Mirrors `WrapMode::WordOrGlyph` rendering closely
/// enough that the reserved slot never clips content (it may overallocate
/// by a row on grapheme-cluster edge cases).
fn wrapped_row_count(lines: &[String], width: usize, tab_len: u8) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                return 1;
            }
            let words: Vec<String> = line.split(' ').map(str::to_string).collect();
            // A word wider than the area breaks glyph-wide; the others
            // share rows, so the per-word ceiling over-approximates only
            // for the wide-word case.
            words
                .iter()
                .map(|w| (display_width(w, tab_len) + width - 1) / width)
                .sum::<usize>()
                .max(1)
        })
        .sum()
}

/// The fields in order; `j`/`k` (and Tab) step through them, wrapping.
const EDIT_FIELDS: [EditField; 4] = [
    EditField::Title,
    EditField::Description,
    EditField::Type,
    EditField::Status,
];

fn field_index(field: EditField) -> usize {
    EDIT_FIELDS
        .iter()
        .position(|f| *f == field)
        .expect("known field")
}

/// The field `delta` steps away, wrapping around both ends.
fn step_field(field: EditField, delta: i8) -> EditField {
    let next = (field_index(field) as i8 + delta).rem_euclid(EDIT_FIELDS.len() as i8) as usize;
    EDIT_FIELDS[next].clone()
}

/// The types the edit pane cycles through, in display order.
const TYPE_CYCLE: [IssueType; 7] = [
    IssueType::Bug,
    IssueType::UxProblem,
    IssueType::Usecase,
    IssueType::Design,
    IssueType::Validation,
    IssueType::Epic,
    IssueType::Milestone,
];

/// The statuses the edit pane cycles through, matching `status_rank`.
const STATUS_CYCLE: [Status; 5] = [
    Status::Proposed,
    Status::Todo,
    Status::InProgress,
    Status::Done,
    Status::Blocked,
];

fn cycle_type(issue_type: IssueType, delta: i8) -> IssueType {
    let pos = TYPE_CYCLE
        .iter()
        .position(|t| *t == issue_type)
        .expect("known type");
    let next = (pos as i8 + delta).rem_euclid(TYPE_CYCLE.len() as i8) as usize;
    TYPE_CYCLE[next].clone()
}

fn cycle_status(status: Status, delta: i8) -> Status {
    let pos = STATUS_CYCLE
        .iter()
        .position(|s| *s == status)
        .expect("known status");
    let next = (pos as i8 + delta).rem_euclid(STATUS_CYCLE.len() as i8) as usize;
    STATUS_CYCLE[next].clone()
}

pub struct App {
    pub mode: Mode,
    /// Which pane owns the keyboard while browsing (r[tui-focus]).
    pub focus: Focus,
    /// Scroll offset of the detail pane while it has focus.
    pub detail_offset: usize,
    /// The open inline edit pane, if any (r[tui-edit]).
    pub edit: Option<EditState>,
    pub filter: Option<Status>,
    /// Raw search line; non-empty means the search filter is active.
    pub search: String,
    pub projects: Vec<ProjectData>,
    pub ops_map: HashMap<PathBuf, Operations>,
    /// Derived render cache: the project/epic/issue tree for the current
    /// filter + search. Never set directly - every mutation of filter,
    /// search or projects rebuilds this inside the App methods in lock-step,
    /// so the cache stays fresh by construction. Reads go through `items()`
    /// (only `render_browser` touches the fields directly, because the Tree
    /// widget needs a live slice next to a mutable `tree_state`, and only
    /// disjoint field borrows satisfy the borrow checker). Key handlers
    /// reach the interactive state via `tree_state_mut()`.
    items: Vec<TreeItem<'static, String>>,
    /// The tree widget's interactive state (open/selected paths). This must
    /// SURVIVE `items` rebuilds so the selection can re-settle (r[tui-review]);
    /// it is deliberately not part of the derived cache.
    tree_state: TreeState<String>,
    pub message: Option<String>,
    pub confirm_delete: bool,
    pub should_quit: bool,
    /// Refresh config (r[tui-refresh]): where `p` re-runs project discovery.
    pub home: PathBuf,
    pub extra_dirs: Vec<PathBuf>,
}

pub fn project_total(pd: &ProjectData) -> usize {
    pd.issues.len()
}

/// Left-pane line for a project node: name + total issue count.
pub fn project_line(pd: &ProjectData) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            pd.name.clone(),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" ({})", project_total(pd))),
    ])
}

/// Detail-pane lines for a highlighted project node (r[project-registry]).
pub fn project_detail_lines(pd: &ProjectData) -> Vec<Line<'static>> {
    let mut by_status: HashMap<&str, usize> = HashMap::new();
    for issue in &pd.issues {
        let label = match issue.status {
            Status::Proposed => "proposed",
            Status::Todo => "todo",
            Status::InProgress => "in progress",
            Status::Done => "done",
            Status::Blocked => "blocked",
        };
        *by_status.entry(label).or_default() += 1;
    }
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Path: ", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(pd.root.to_string_lossy().to_string()),
    ]));
    lines.push(Line::from(format!("Issues: {}", project_total(pd))));
    lines.push(Line::from(format!(
        "proposed {} | todo {} | in progress {} | done {} | blocked {}",
        by_status.get("proposed").copied().unwrap_or(0),
        by_status.get("todo").copied().unwrap_or(0),
        by_status.get("in progress").copied().unwrap_or(0),
        by_status.get("done").copied().unwrap_or(0),
        by_status.get("blocked").copied().unwrap_or(0),
    )));
    let mtime_line = pd
        .index_mtime
        .map(|m| {
            let dt: chrono::DateTime<chrono::Utc> = m.into();
            format!("Last index update: {}", dt.format("%Y-%m-%d %H:%M"))
        })
        .unwrap_or_else(|| "Last index update: unknown".to_string());
    lines.push(Line::from(mtime_line));
    // Load warnings as data, rendered into the detail pane
    // (r[load-warnings-as-data]): raw stderr would corrupt the alternate
    // screen while the TUI is in raw mode.
    for warning in &pd.warnings {
        lines.push(Line::from(format!("WARNING: {}", warning.message)));
    }
    lines
}

/// Recursively check whether any node in the tree carries `identifier`.
pub fn items_contain(items: &[TreeItem<'static, String>], identifier: &str) -> bool {
    items.iter().any(|item| {
        item.identifier().as_str() == identifier || items_contain(item.children(), identifier)
    })
}

impl App {
    pub fn new() -> Self {
        App {
            mode: Mode::Browser,
            focus: Focus::Tree,
            detail_offset: 0,
            edit: None,
            filter: None,
            search: String::new(),
            projects: Vec::new(),
            ops_map: HashMap::new(),
            items: Vec::new(),
            tree_state: TreeState::default(),
            message: None,
            confirm_delete: false,
            should_quit: false,
            home: PathBuf::from(""),
            extra_dirs: Vec::new(),
        }
    }

    /// Load every project root (registry order) and build the tree.
    pub fn load(roots: &[PathBuf]) -> Result<Self> {
        let mut app = App::new();
        let mut projects: Vec<ProjectData> = Vec::new();
        for root in roots {
            let ops = Operations::new(root)?;
            let pd = load_project_data(root, &ops)?;
            app.ops_map.insert(root.clone(), ops);
            projects.push(pd);
        }
        app.set_projects(projects);
        if !app.selection_visible() {
            app.reset_selection_to_first();
        }
        // Fold each project's first load warning into the message row: a
        // degraded store is visible on startup without raw stderr
        // (r[load-warnings-as-data]).
        for pd in &app.projects {
            if !pd.warnings.is_empty() {
                app.message = Some(format!("WARNING: {}", pd.warnings[0].message));
                break;
            }
        }
        Ok(app)
    }

    /// The derived render cache for the current filter/search: always
    /// freshly rebuilt, because every mutation of filter, search or
    /// projects goes through the locking App methods (`set_filter`,
    /// `set_search`, `set_projects`, `settle_selection`) which rebuild in
    /// the same step. Callers can never observe a stale tree.
    pub fn items(&self) -> &[TreeItem<'static, String>] {
        &self.items
    }

    /// The tree widget's interactive state (open/selected paths), read
    /// side.
    pub fn tree_state(&self) -> &TreeState<String> {
        &self.tree_state
    }

    /// The tree widget's interactive state (open/selected paths), write
    /// side: rendering and the key handlers mutate selection/folding here.
    /// The state is independent of `items` rebuilds on purpose, so a
    /// reload can re-settle the selection (r[tui-review]).
    pub fn tree_state_mut(&mut self) -> &mut TreeState<String> {
        &mut self.tree_state
    }

    /// Replace the project list and rebuild the derived tree in the same
    /// step (the pure, I/O-free half of `load`). The per-project mutations
    /// (`reload_project`, `refresh_projects`) keep the same lock-step
    /// through `settle_selection`.
    pub fn set_projects(&mut self, projects: Vec<ProjectData>) {
        self.projects = projects;
        self.rebuild_items();
    }

    /// Set the review filter (r[tui-review]), rebuilding the derived tree
    /// in the same step: the filter and `items` can never disagree.
    pub fn set_filter(&mut self, filter: Option<Status>) {
        self.filter = filter;
        self.rebuild_items();
    }

    /// Replace the search line (r[tui-search]), rebuilding the derived
    /// tree in the same step. Rebuilding an unchanged search is a no-op
    /// (the tree is a pure function of filter/search/projects), so this is
    /// called on every keystroke without a change check, and never
    /// reloads from disk (load-once-per-invocation, r[tui-refresh]).
    pub fn set_search(&mut self, search: String) {
        self.search = search;
        self.rebuild_items();
    }

    fn rebuild_items(&mut self) {
        let search_active = !self.search.is_empty();
        let mut items = Vec::new();
        for pd in &self.projects {
            let counts = build_epic_counts(&pd.issues);
            let children = tree_items(&pd.issues, &counts, self.filter, &self.search)
                .expect("unique identifiers");
            if (self.filter.is_some() || search_active) && children.is_empty() {
                continue; // hide projects without matching issues (r[project-registry])
            }
            let children = match children.is_empty() {
                true => children,
                false => children,
            };
            let _ = children.is_empty();
            items.push(
                TreeItem::new(
                    pd.root.to_string_lossy().to_string(),
                    project_line(pd),
                    children,
                )
                .expect("unique identifiers"),
            );
        }
        self.items = items;
    }

    /// Reload a single project after a mutation and re-settle the selection
    /// (r[tui-review]): keep the highlight on the same item while it is
    /// still visible, otherwise move it by exactly one position - to the
    /// item that followed it before the reload, or to the new last item when
    /// the removed one was last. Never teleport to the first item.
    pub fn reload_project(&mut self, root: &Path) -> Result<()> {
        let ops = self
            .ops_map
            .get(root)
            .context("no operations for project")?;
        let pd = load_project_data(root, ops)?;
        let (same, next) = self.selection_targets();
        if let Some(slot) = self.projects.iter_mut().find(|p| p.root == root) {
            *slot = pd;
        }
        self.settle_selection(same, next);
        Ok(())
    }

    /// Re-settle the selection after the issue set changed (r[tui-review]):
    /// keep the same item while it is still visible, otherwise move exactly
    /// one position - to the item that followed it before the change, or to
    /// the new last item when the removed one was last. Never teleport to
    /// the first item. With no prior selection (startup edge) re-select the
    /// first visible item.
    fn settle_selection(&mut self, same: Option<String>, next: Option<String>) {
        self.rebuild_items();
        if let Some(same) = same {
            if select_item(self, &same) {
                return;
            }
            if let Some(next) = next {
                if select_item(self, &next) {
                    return;
                }
            }
            self.reset_selection_to_last();
        } else if !self.selection_visible() {
            self.reset_selection_to_first();
        }
    }

    /// r[tui-refresh]: re-run project discovery with the startup config and
    /// reload every project from disk - external edits (another pebbles
    /// process, an agent session, a manual edit, a jj checkout) become
    /// visible, newly discovered projects are added and vanished ones
    /// dropped, keeping the registry order. The selection re-settles exactly
    /// like after a mutation (r[tui-review]). The project count may grow
    /// without bound, so this stays manual and on-demand: load-once-per-
    /// invocation is the rule everywhere else, and nothing polls between
    /// keys. Every project reloads (not just index.json mtime winners)
    /// because in-place issue edits, like an external approve, do not touch
    /// the index; several file reads per project on an explicit key are
    /// trivial next to the hot path.
    pub fn refresh_projects(&mut self) -> Result<()> {
        let roots = discover_roots(&self.home, &self.extra_dirs)?;
        let (same, next) = self.selection_targets();
        for root in &roots {
            if self.projects.iter().any(|p| p.root == *root) {
                let ops = self
                    .ops_map
                    .get(root)
                    .context("no operations for project")?;
                let pd = load_project_data(root.as_path(), ops)?;
                if let Some(slot) = self.projects.iter_mut().find(|p| p.root == *root) {
                    *slot = pd;
                }
            } else {
                let ops = Operations::new(root.as_path())?;
                let pd = load_project_data(root.as_path(), &ops)?;
                self.ops_map.insert(root.clone(), ops);
                self.projects.push(pd);
            }
        }
        // Drop projects that are no longer discovered, then reorder to the
        // registry order (fresh discovery sorts by index mtime, r[project-registry]).
        self.projects.retain(|p| roots.iter().any(|r| *r == p.root));
        let mut reordered: Vec<ProjectData> = Vec::new();
        for root in &roots {
            let pos = self
                .projects
                .iter()
                .position(|p| p.root == *root)
                .expect("discovered root");
            reordered.push(self.projects.remove(pos));
        }
        self.projects = reordered;
        self.ops_map
            .retain(|root, _| self.projects.iter().any(|p| p.root == *root));
        self.settle_selection(same, next);
        Ok(())
    }

    /// The pre-reload selection targets: the currently selected identifier,
    /// and the identifier of the item that follows it in tree order.
    fn selection_targets(&self) -> (Option<String>, Option<String>) {
        let Some(current) = self.tree_state.selected().last().cloned() else {
            return (None, None);
        };
        let order = tree_order(self.items());
        let pos = order.iter().position(|id| id == &current);
        let next = pos.and_then(|i| order.get(i + 1)).cloned();
        (Some(current), next)
    }

    /// The project owning `uuid` (mutations route here).
    pub fn project_of(&self, uuid: &Uuid) -> Option<&ProjectData> {
        self.projects
            .iter()
            .find(|pd| pd.issues.iter().any(|i| i.id == *uuid))
    }

    fn selection_visible(&self) -> bool {
        let Some(last) = self.tree_state.selected().last() else {
            return false;
        };
        if !items_contain(self.items(), last) {
            return false;
        }
        // Issue selections must still resolve to a live issue.
        if let Ok(uuid) = Uuid::parse_str(last) {
            return self.project_of(&uuid).is_some();
        }
        true
    }

    fn reset_selection_to_first(&mut self) {
        // Prefer the first ISSUE (deep-first), opening its ancestors so the
        // selection is visible; fall back to the first project node.
        match first_leaf_path(self.items(), &mut Vec::new()) {
            Some(mut full_path) => {
                let leaf = full_path.pop();
                // Open every ancestor so the whole path is visible.
                for len in 1..=full_path.len() {
                    self.tree_state.open(full_path[..len].to_vec());
                }
                self.tree_state.select(full_path_leaf(leaf, &full_path));
            }
            None => {
                self.tree_state.select(Vec::new());
            }
        }
    }

    /// Select the last visible leaf (the mirror of reset_selection_to_first):
    /// the new last item after the previously last one was removed
    /// (r[tui-review]). Falls back to no selection when the tree is empty.
    fn reset_selection_to_last(&mut self) {
        match last_leaf_path(self.items(), &mut Vec::new()) {
            Some(mut full_path) => {
                let leaf = full_path.pop();
                for len in 1..=full_path.len() {
                    self.tree_state.open(full_path[..len].to_vec());
                }
                self.tree_state.select(full_path_leaf(leaf, &full_path));
            }
            None => {
                self.tree_state.select(Vec::new());
            }
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        self.selected_with_project().map(|(issue, _)| issue)
    }

    pub fn selected_with_project(&self) -> Option<(&Issue, &ProjectData)> {
        let uuid = Uuid::parse_str(self.tree_state.selected().last()?).ok()?;
        let pd = self.project_of(&uuid)?;
        let issue = pd.issues.iter().find(|i| i.id == uuid)?;
        Some((issue, pd))
    }
}

fn load_project_data(root: &Path, ops: &Operations) -> Result<ProjectData> {
    let (issues, warnings) = ops.list(None, None)?;
    let related_titles = issues.iter().map(|i| (i.id, i.title.clone())).collect();
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
    Ok(ProjectData {
        root: root.to_path_buf(),
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string()),
        issues,
        related_titles,
        incoming,
        warnings,
        index_mtime: crate::projects::index_mtime(root),
    })
}

/// Deep-first search for the first leaf node path (an issue, not a container).
fn first_leaf_path(
    items: &[TreeItem<'static, String>],
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    for item in items {
        prefix.push(item.identifier().clone());
        if item.children().is_empty() {
            return Some(prefix.clone());
        }
        if let Some(found) = first_leaf_path(item.children(), prefix) {
            return Some(found);
        }
        prefix.pop();
    }
    None
}

fn full_path_leaf(leaf: Option<String>, prefix: &[String]) -> Vec<String> {
    let mut full = prefix.to_vec();
    if let Some(leaf) = leaf {
        full.push(leaf);
    }
    full
}

/// Deep-last search for the last leaf node path (an issue, not a container):
/// the mirror of `first_leaf_path`, for re-selecting the new last item after
/// the previously last one was removed (r[tui-review]).
fn last_leaf_path(
    items: &[TreeItem<'static, String>],
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    for item in items.iter().rev() {
        prefix.push(item.identifier().clone());
        if item.children().is_empty() {
            return Some(prefix.clone());
        }
        if let Some(found) = last_leaf_path(item.children(), prefix) {
            return Some(found);
        }
        prefix.pop();
    }
    None
}

/// The identifiers of every node in pre-order traversal (`j`/`k` move
/// through an unfolded tree in this order; the proposed-issues filter, where
/// mutations happen, is a flat leaf list so pre-order is exactly the
/// rendered order). Used to find the item that follows the current selection
/// when a mutation rebuilds the tree (r[tui-review]).
fn tree_order(items: &[TreeItem<'static, String>]) -> Vec<String> {
    fn walk(items: &[TreeItem<'static, String>], out: &mut Vec<String>) {
        for item in items {
            out.push(item.identifier().clone());
            walk(item.children(), out);
        }
    }
    let mut out = Vec::new();
    walk(items, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HELP_LINES: &[&str] = &[
    "j/k or Up/Down  tree: move; detail: scroll",
    "Tab/BackTab     switch focus: tree / search / detail",
    "Right           unfold (or move to first child)",
    "Left            fold (or move to parent)",
    "Enter or f      toggle fold (tree)",
    "/               focus the search line; live filter",
    "                typing: Enter accepts, Esc clears,",
    "                Backspace edits; s:!d = status not done,",
    "                t:bug = type bug, !term = not term",
    "r               toggle the proposed-issues filter",
    "p               refresh: re-discover, reload projects",
    "a               approve selected (in proposed filter)",
    "d then y        close selected = delete (in filter)",
    "e               edit title/description/type/status (detail pane)",
    "?               toggle this help",
    "q               quit",
];

pub fn render_app(f: &mut Frame, app: &mut App) {
    // Clear the whole frame first so cells left behind by previous terminal
    // contents (or a previous, larger frame) never survive into a draw:
    // every unpainted cell comes out blank (r[tui-background]).
    f.render_widget(Clear, f.area());
    match app.mode {
        Mode::Browser | Mode::Edit => render_browser(f, app),
        Mode::Help => render_help(f, app),
    }
}

fn message_paragraph(app: &App) -> Paragraph<'static> {
    let text = if app.focus == Focus::Search {
        format!("search: {}", app.search)
    } else {
        app.message.clone().unwrap_or_default()
    };
    Paragraph::new(Line::from(Span::styled(
        text,
        Style::new().fg(Color::DarkGray),
    )))
}

fn tree_title(app: &App) -> String {
    let search_part = if app.search.is_empty() {
        None
    } else {
        Some(format!("search: {}", app.search))
    };
    match app.filter {
        Some(Status::Proposed) => {
            let filter_part = "proposed (a approve, d+y close, Esc/r clear)".to_string();
            match search_part {
                Some(s) => format!("pebbles - {s} | {filter_part}"),
                None => format!("pebbles - {filter_part}"),
            }
        }
        _ => match search_part {
            Some(s) => format!("pebbles - {s}"),
            None => "pebbles - projects".to_string(),
        },
    }
}

fn render_browser(f: &mut Frame, app: &mut App) {
    let [main_area, message_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .areas(main_area);

    // The focused pane's border is cyan; the other stays neutral. The search
    // line is indicated by the prompt shown in the message row instead.
    let focus_style = Style::new().fg(Color::Cyan);
    let tree_block = match app.focus {
        Focus::Tree => Block::bordered()
            .title(tree_title(app))
            .border_style(focus_style),
        _ => Block::bordered().title(tree_title(app)),
    };
    let detail_block = match app.focus {
        Focus::Detail => Block::bordered().border_style(focus_style),
        _ => Block::bordered(),
    };

    // Borrow the fields directly, not through the accessors: the Tree
    // widget keeps a slice of `items` alive while the same call mutates
    // `tree_state`, and only disjoint field borrows satisfy the borrow
    // checker. Reads never re-sync anything, so this is safe; the fields
    // stay private to this module.
    let tree = Tree::new(&app.items)
        .expect("unique identifiers")
        .block(tree_block)
        .highlight_symbol("> ")
        .highlight_style(Style::new().bg(Color::Rgb(60, 60, 60)));
    f.render_stateful_widget(tree, tree_area, &mut app.tree_state);

    // The inline edit pane takes over the detail area (r[tui-edit]); the
    // tree stays visible so edits land next to the issue they belong to.
    // The edit pane is rect-based (d31): the armed field's TextArea owns a
    // dedicated slot with the other fields laid out around it, instead of
    // the old row-list paragraph that the widget would have painted over.
    if app.mode == Mode::Edit {
        render_edit_pane(f, app, detail_area);
    } else {
        let (all_lines, title) = if let Some((issue, pd)) = app.selected_with_project() {
            (
                detail_lines(issue, &pd.related_titles, &pd.incoming),
                issue.title.clone(),
            )
        } else if let Some(pd) = selected_project(app) {
            (project_detail_lines(pd), pd.name.clone())
        } else {
            (vec![Line::raw("No selection")], "detail".to_string())
        };
        // The detail pane scrolls while it has focus (j/k): render a window
        // of the lines instead of always showing the head.
        let visible = detail_area.height.saturating_sub(2) as usize; // borders
        let offset = app.detail_offset.min(all_lines.len().saturating_sub(1));
        let window: Vec<Line<'static>> = all_lines
            .iter()
            .skip(offset)
            .take(visible.max(1))
            .cloned()
            .collect();
        let detail = Paragraph::new(window)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(detail_block.title(title));
        f.render_widget(detail, detail_area);
    }
    f.render_widget(message_paragraph(app), message_area);
}

fn selected_project(app: &App) -> Option<&ProjectData> {
    let sel = app.tree_state().selected().last()?;
    if Uuid::parse_str(sel).is_ok() {
        return None; // an issue is selected, not a project node
    }
    app.projects
        .iter()
        .find(|pd| pd.root.to_string_lossy().to_string() == *sel)
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
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::new().bg(Color::Black))
            .block(block),
        area,
    );
}

fn centered_rect(area: Rect, pct_x: u16, height: u16) -> Rect {
    let w = area.width * pct_x / 100;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: height.min(area.height),
    }
}

// ---------------------------------------------------------------------------
// Imperative shell: terminal setup, event loop, mutations
// ---------------------------------------------------------------------------

/// r[project-registry]: scan ~/repos (+ config extras), always include the
/// current directory when it is a project; sort by index mtime. Shared by
/// startup and the `p` refresh key (r[tui-refresh]).
fn discover_roots(home: &Path, extra_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut candidates: Vec<(PathBuf, Option<std::time::SystemTime>)> =
        crate::projects::discover(home, extra_dirs)?
            .into_iter()
            .map(|p| (p.root.clone(), crate::projects::index_mtime(&p.root)))
            .collect();
    let cwd = std::env::current_dir()?;
    if cwd.join(".pebbles").is_dir() {
        let canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        if !candidates.iter().any(|(root, _)| {
            std::fs::canonicalize(root)
                .map(|c| c == canon)
                .unwrap_or(false)
        }) {
            candidates.push((cwd.clone(), crate::projects::index_mtime(&cwd)));
        }
    }
    Ok(crate::projects::sort_projects(candidates)
        .into_iter()
        .map(|p| p.root)
        .collect())
}

pub fn run() -> Result<()> {
    let extra_dirs = {
        let path = PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".config/pebbles/projects.json");
        crate::projects::parse_extra_dirs(std::fs::read_to_string(path).ok().as_deref())
    };
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let roots = discover_roots(&home, &extra_dirs)?;
    if roots.is_empty() {
        anyhow::bail!("Pebbles not initialized. Run 'pebbles init' first.");
    }

    let mut app = App::load(&roots)?;
    app.home = home;
    app.extra_dirs = extra_dirs;

    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = stdout();
    stdout.execute(EnterAlternateScreen)?;
    // Entering the alternate screen does not blank it on every terminal, and
    // ratatui's first frame only repaints cells its widgets touch. Clear the
    // (alternate) screen outright so cells from previous terminal contents
    // never show through unpainted areas (r[tui-background]). This uses the
    // escape-sequence Clear on purpose: Terminal::clear() would query the
    // cursor position, which blocks when no terminal answers.
    stdout.execute(crossterm::terminal::Clear(
        crossterm::terminal::ClearType::All,
    ))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    let stdout = terminal.backend_mut();
    stdout.execute(LeaveAlternateScreen)?;
    stdout.execute(crossterm::cursor::Show)?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        // TreeState.select_first() needs identifiers from a previous render,
        // so re-select when empty and let the next draw pick it up.
        if !app.selection_visible() && !app.items().is_empty() {
            app.reset_selection_to_first();
        }
        terminal.draw(|f| render_app(f, app))?;
        if let Event::Key(key) = read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.mode {
                Mode::Browser => match app.focus {
                    Focus::Tree => tree_key(app, key.code)?,
                    Focus::Search => search_key(app, key.code)?,
                    Focus::Detail => detail_key(app, key.code)?,
                },
                Mode::Edit => edit_key(app, key)?,
                Mode::Help => app.mode = Mode::Browser,
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Keys while the tree pane has focus (r[tui-focus]).
fn tree_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => {
            app.detail_offset = 0;
            app.tree_state_mut().key_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.detail_offset = 0;
            app.tree_state_mut().key_up();
        }
        KeyCode::Right => {
            app.detail_offset = 0;
            app.tree_state_mut().key_right();
        }
        KeyCode::Left => {
            app.detail_offset = 0;
            app.tree_state_mut().key_left();
        }
        KeyCode::Enter | KeyCode::Char('f') => {
            app.detail_offset = 0;
            app.tree_state_mut().toggle_selected();
        }
        KeyCode::Tab => app.focus = Focus::Search,
        KeyCode::BackTab => app.focus = Focus::Detail,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('/') => app.focus = Focus::Search,
        KeyCode::Esc => {
            if !app.search.is_empty() {
                app.set_search(String::new());
                app.confirm_delete = false;
                app.reset_selection_to_first();
            } else {
                toggle_review_filter(app);
            }
        }
        KeyCode::Char('r') => toggle_review_filter(app),
        KeyCode::Char('p') => {
            app.confirm_delete = false;
            app.refresh_projects()?;
            app.message = Some("Refreshed".to_string());
        }
        KeyCode::Char('a') => {
            if app.filter.is_some() {
                approve_selected(app)?;
            } else {
                app.message = Some("Press r to enter the proposed filter to approve".to_string());
            }
        }
        KeyCode::Char('d') => {
            if app.filter.is_some() {
                if app.selected_issue().is_some() {
                    app.confirm_delete = true;
                    app.message = Some("Press y to confirm closing (delete)".to_string());
                } else {
                    app.message = Some("No selection".to_string());
                }
            } else {
                app.message = Some("Press r to enter the proposed filter to close".to_string());
            }
        }
        KeyCode::Char('y') if app.confirm_delete => {
            app.confirm_delete = false;
            delete_selected(app)?;
        }
        _ => app.confirm_delete = false,
    }
    Ok(())
}

fn toggle_review_filter(app: &mut App) {
    let next = if app.filter.is_some() {
        None
    } else {
        Some(Status::Proposed)
    };
    app.set_filter(next);
    app.confirm_delete = false;
    app.reset_selection_to_first();
}

/// Keys while the search line has focus (`/` or Tab). The filter applies
/// live on every keystroke; Enter accepts it, Esc clears it and returns to
/// the tree, Tab moves focus elsewhere without closing the search.
fn search_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Enter => {
            app.focus = Focus::Tree;
        }
        KeyCode::Esc => {
            app.set_search(String::new());
            app.focus = Focus::Tree;
            app.confirm_delete = false;
            app.reset_selection_to_first();
        }
        KeyCode::Tab => app.focus = Focus::Detail,
        KeyCode::BackTab => app.focus = Focus::Tree,
        KeyCode::Backspace | KeyCode::Delete => {
            let mut next = app.search.clone();
            if next.pop().is_some() {
                app.set_search(next);
                if !app.selection_visible() {
                    app.reset_selection_to_first();
                }
            }
        }
        KeyCode::Char(c) => {
            let mut next = app.search.clone();
            next.push(c);
            app.set_search(next);
            if !app.selection_visible() {
                app.reset_selection_to_first();
            }
        }
        _ => {}
    }
    Ok(())
}

/// Keys while the detail pane has focus (r[tui-focus]): j/k scroll the
/// detail, `e` opens the inline edit pane (r[tui-edit]), Esc returns to
/// the tree, `/` jumps to the search line.
fn detail_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down | KeyCode::PageDown => {
            app.detail_offset += 1;
        }
        KeyCode::Char('k') | KeyCode::Up | KeyCode::PageUp => {
            app.detail_offset = app.detail_offset.saturating_sub(1);
        }
        KeyCode::Tab => app.focus = Focus::Tree,
        KeyCode::BackTab => app.focus = Focus::Search,
        KeyCode::Char('/') => {
            app.detail_offset = 0;
            app.focus = Focus::Search;
        }
        KeyCode::Char('e') => open_edit(app)?,
        KeyCode::Esc => app.focus = Focus::Tree,
        _ => app.confirm_delete = false,
    }
    Ok(())
}

fn approve_selected(app: &mut App) -> Result<()> {
    let Some((issue, pd)) = app.selected_with_project() else {
        app.message = Some("No selection".to_string());
        return Ok(());
    };
    if issue.status != Status::Proposed {
        app.message = Some("Only proposed issues can be approved".to_string());
        return Ok(());
    }
    let uuid = issue.id;
    let root = pd.root.clone();
    let Some(ops) = app.ops_map.get(&root) else {
        app.message = Some("No operations for this project".to_string());
        return Ok(());
    };
    match ops.approve(&uuid.to_string()) {
        Ok(outcome) => {
            let alid = ops.appropriate_length_id(&uuid)?;
            app.reload_project(&root)?;
            app.message = Some(fold_warnings(
                format!("\u{2713} Approved {alid}"),
                &outcome.warnings,
            ));
        }
        Err(e) => app.message = Some(format!("Error: {e}")),
    }
    Ok(())
}

fn delete_selected(app: &mut App) -> Result<()> {
    let Some((issue, pd)) = app.selected_with_project() else {
        app.message = Some("No selection".to_string());
        return Ok(());
    };
    let uuid = issue.id;
    let root = pd.root.clone();
    let Some(ops) = app.ops_map.get(&root) else {
        app.message = Some("No operations for this project".to_string());
        return Ok(());
    };
    match ops.delete(&uuid.to_string(), false) {
        Ok(outcome) => {
            app.reload_project(&root)?;
            app.message = Some(fold_warnings(
                format!("\u{2713} Deleted {}", outcome.value),
                &outcome.warnings,
            ));
        }
        Err(e) => app.message = Some(format!("Error: {e}")),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Inline edit pane (r[tui-edit])
// ---------------------------------------------------------------------------

/// The live issue `issue_id` and its project, wherever they are now (the
/// pane pins the issue, so commits survive tree reloads and re-sorts).
fn issue_with_project<'a>(app: &'a App, issue_id: Uuid) -> Option<(&'a Issue, &'a ProjectData)> {
    let pd = app.project_of(&issue_id)?;
    let issue = pd.issues.iter().find(|i| i.id == issue_id)?;
    Some((issue, pd))
}

/// `e` in the detail pane: open the edit pane for the selected issue.
fn open_edit(app: &mut App) -> Result<()> {
    let Some((issue, _)) = app.selected_with_project() else {
        app.message = Some("Select an issue to edit".to_string());
        return Ok(());
    };
    let edit = EditState::for_issue(issue);
    app.detail_offset = 0;
    app.confirm_delete = false;
    app.edit = Some(edit);
    app.mode = Mode::Edit;
    Ok(())
}

fn close_edit(app: &mut App) {
    app.mode = Mode::Browser;
    app.edit = None;
}

/// The nested tree path (list of identifiers) to `identifier`, if visible.
fn path_to_item(
    items: &[TreeItem<'static, String>],
    identifier: &str,
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    for item in items {
        prefix.push(item.identifier().clone());
        if item.identifier().as_str() == identifier {
            return Some(prefix.clone());
        }
        if let Some(path) = path_to_item(item.children(), identifier, prefix) {
            return Some(path);
        }
        prefix.pop();
    }
    None
}

/// Select `identifier` in the tree, opening its ancestors so it is visible
/// again after a mutation re-built the items.
fn select_item(app: &mut App, identifier: &str) -> bool {
    let Some(path) = path_to_item(app.items(), identifier, &mut Vec::new()) else {
        return false;
    };
    for len in 1..path.len() {
        app.tree_state_mut().open(path[..len].to_vec());
    }
    app.tree_state_mut().select(path);
    true
}

/// Step the pending Type/Status value by `delta`; other fields ignore the
/// arrows (they only scroll/steer during text editing, which is unsupported).
fn cycle_edit(edit: &mut EditState, delta: i8) {
    match edit.field {
        EditField::Type => edit.pending_type = cycle_type(edit.pending_type.clone(), delta),
        EditField::Status => edit.pending_status = cycle_status(edit.pending_status.clone(), delta),
        EditField::Title | EditField::Description => {}
    }
}

/// Apply the highlighted field through the operations layer: `update` for
/// title/description (keeping the non-empty-title validation),
/// `convert` for type, `update_status` for status. Every applied field is
/// one operation and therefore its own jj commit (r[tui-edit]).
fn commit_edit(app: &mut App, edit: &mut EditState) -> Result<()> {
    let Some((_, pd)) = issue_with_project(app, edit.issue_id) else {
        app.message = Some("Issue no longer available".to_string());
        return Ok(());
    };
    let root = pd.root.clone();
    let Some(ops) = app.ops_map.get(&root) else {
        app.message = Some("No operations for this project".to_string());
        return Ok(());
    };
    let id = edit.issue_id.to_string();
    let outcome = match edit.field {
        EditField::Title => ops.update(&id, Some(edit.text()), None, None).map(|out| {
            (
                out.value,
                fold_warnings("\u{2713} Updated title".to_string(), &out.warnings),
            )
        }),
        EditField::Description => ops.update(&id, None, Some(edit.text()), None).map(|out| {
            (
                out.value,
                fold_warnings("\u{2713} Updated description".to_string(), &out.warnings),
            )
        }),
        EditField::Type => ops.convert(&id, edit.pending_type.clone()).map(|out| {
            (
                out.value,
                fold_warnings(
                    format!("\u{2713} Converted to {}", edit.pending_type),
                    &out.warnings,
                ),
            )
        }),
        EditField::Status => ops
            .update_status(&id, edit.pending_status.clone())
            .map(|out| {
                (
                    out.value,
                    fold_warnings(
                        format!("\u{2713} Status: {}", edit.pending_status),
                        &out.warnings,
                    ),
                )
            }),
    };
    match outcome {
        Ok((false, _)) => {
            app.message = Some("No changes: nothing to update".to_string());
        }
        Ok((true, message)) => {
            app.reload_project(&root)?;
            if !select_item(app, &id) {
                // The issue is hidden by an active filter: stop editing it.
                app.message = Some("Issue hidden by filter; edit closed".to_string());
                app.edit = None;
                app.mode = Mode::Browser;
                return Ok(());
            }
            app.message = Some(message);
            // The pane now reflects the committed values.
            edit.editing = false;
            if let Some(issue) = issue_with_project(app, edit.issue_id) {
                edit.pending_type = issue.0.issue_type.clone();
                edit.pending_status = issue.0.status.clone();
            }
        }
        Err(e) => {
            // Validation (empty title) and convert guards keep the pane open
            // so the user can fix the field (r[tui-edit]).
            app.message = Some(format!("Error: {e}"));
        }
    }
    Ok(())
}

/// Keys while the inline edit pane is open (r[tui-edit]). Esc cancels
/// without writing: an in-progress field edit is discarded, and in
/// navigation it closes the pane. Enter applies the highlighted field:
/// immediately for the cycled Type/Status (commit-and-stay), after editing
/// for Title/Description (arm-then-commit); in the multi-line Description
/// edit Enter inserts a newline and Tab applies. Every key that is not a
/// pane-level key while a typed field is armed is converted to the widget's
/// `Input` and handed to the `TextArea` (its own arrows/Home/End/
/// Backspace/Delete/word moves and Ctrl chords, d31).
fn edit_key(app: &mut App, key: crossterm::event::KeyEvent) -> Result<()> {
    let Some(mut edit) = app.edit.clone() else {
        return Ok(());
    };
    let mut closed = false;
    let code = key.code;
    match code {
        KeyCode::Char('q') if !edit.editing => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab if !edit.editing => {
            edit.field = step_field(edit.field, 1);
        }
        KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab if !edit.editing => {
            edit.field = step_field(edit.field, -1);
        }
        // Enter from navigation arms the typed fields (Title/Description
        // keep arm-then-commit) and applies the cycled fields (Type/Status)
        // on the first press, staying in the pane (r[tui-edit]).
        KeyCode::Enter if !edit.editing => match edit.field {
            EditField::Type | EditField::Status => {
                commit_edit(app, &mut edit)?;
            }
            EditField::Title | EditField::Description => {
                let Some(issue) = issue_with_project(app, edit.issue_id).map(|(i, _)| i.clone())
                else {
                    app.message = Some("Issue no longer available".to_string());
                    close_edit(app);
                    return Ok(());
                };
                edit.editing = true;
                match edit.field {
                    EditField::Title => {
                        edit.textarea =
                            textarea_for(issue.title.clone(), end_cursor(issue.title.clone()));
                    }
                    EditField::Description => {
                        let text = issue.description.clone().unwrap_or_default();
                        edit.textarea = textarea_for(text.clone(), end_cursor(text));
                    }
                    EditField::Type | EditField::Status => unreachable!("handled above"),
                }
            }
        },
        // While a typed field is armed everything else - arrows, Home/End,
        // Backspace/Delete, printable characters, Ctrl/Alt chords - goes to
        // the widget, which moves the cursor and edits its buffer. j/k move
        // the cursor in a multi-line Description instead of switching fields.
        KeyCode::Char(_)
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::Backspace
        | KeyCode::Delete
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Insert
            if edit.editing =>
        {
            edit.textarea.input(Input::from(key));
        }
        KeyCode::Left | KeyCode::Right if !edit.editing => {
            cycle_edit(&mut edit, if code == KeyCode::Right { 1 } else { -1 });
        }
        // In the Description edit Enter inserts a newline (multi-line), so
        // Tab is the apply key there; Title applies on Enter (r[tui-edit]).
        KeyCode::Enter if edit.editing => match edit.field {
            EditField::Title => commit_edit(app, &mut edit)?,
            EditField::Description => {
                edit.textarea.input(Input::from(key));
            }
            EditField::Type | EditField::Status => unreachable!("cycled fields never arm"),
        },
        KeyCode::Tab if edit.editing => commit_edit(app, &mut edit)?,
        KeyCode::Esc => {
            if edit.editing {
                // Cancel the in-progress field edit without writing.
                edit.editing = false;
                if let Some(issue) = issue_with_project(app, edit.issue_id) {
                    edit.pending_type = issue.0.issue_type.clone();
                    edit.pending_status = issue.0.status.clone();
                }
            } else {
                closed = true;
            }
        }
        _ => {}
    }
    if closed {
        close_edit(app);
    } else {
        app.edit = Some(edit);
    }
    Ok(())
}

/// A widget with `text` as the buffer, the cursor at `(row, col)` (char
/// offsets), word-or-glyph soft wrap at render time and no cursor-line
/// decoration beyond the base style (the slot rect is the pane's
/// highlight; r[tui-edit], d31).
fn textarea_for(text: String, cursor: (usize, usize)) -> TextArea<'static> {
    let mut ta = TextArea::from(text.split('\n').map(str::to_string).collect::<Vec<_>>());
    ta.set_cursor_line_style(Style::new());
    ta.set_wrap_mode(WrapMode::WordOrGlyph);
    ta.move_cursor(ratatui_textarea::CursorMove::Jump(
        cursor.0 as u16,
        cursor.1 as u16,
    ));
    ta
}

/// The (row, col) char position just past the last character of `text`:
/// the append point the pane arms typed fields at.
fn end_cursor(text: String) -> (usize, usize) {
    let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let last = lines.len() - 1;
    (last, lines[last].chars().count())
}

/// One field row for the pane: `> ` marker on the active field, the label
/// bold, the value live from the issue (or the pending value for the
/// active cycled field) and the active row highlighted (r[tui-edit]).
fn field_row(edit: &EditState, issue: Option<&Issue>, field: EditField) -> Line<'static> {
    let active = edit.field == field;
    let highlight = Style::new().bg(Color::Rgb(60, 60, 60));
    let marker = if active { "> " } else { "  " };
    let value_style = if active { highlight } else { Style::new() };
    let label = field_label(field);
    let value = match field {
        EditField::Title => issue.map(|i| i.title.clone()).unwrap_or_default(),
        EditField::Description => issue
            // First line only; continuation lines are emitted by
            // field_rows so multi-line descriptions stay parseable (bug 1bc).
            .map(|i| {
                let first = i
                    .description
                    .clone()
                    .unwrap_or_default()
                    .split('\n')
                    .map(str::to_string)
                    .collect::<Vec<_>>()[0]
                    .clone();
                first
            })
            .unwrap_or_default(),
        EditField::Type => {
            if active {
                edit.pending_type.to_string()
            } else {
                issue
                    .map(|i| i.issue_type.to_string())
                    .unwrap_or_else(|| edit.pending_type.to_string())
            }
        }
        EditField::Status => {
            if active {
                edit.pending_status.to_string()
            } else {
                issue
                    .map(|i| i.status.to_string())
                    .unwrap_or_else(|| edit.pending_status.to_string())
            }
        }
    };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(
            format!("{label}: "),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, value_style),
    ])
}

/// The pane's hint line: which keys do what in the current state.
fn edit_hint(edit: &EditState) -> Line<'static> {
    let hint = if edit.editing {
        match edit.field {
            EditField::Description => "Enter: new line, Tab: apply, Esc: cancel".to_string(),
            EditField::Title => "Enter applies, Esc cancels".to_string(),
            // Cycled fields never arm, but keep rendering sane if they do.
            EditField::Type | EditField::Status => "Enter applies, Esc cancels".to_string(),
        }
    } else {
        match edit.field {
            EditField::Title | EditField::Description => {
                "j/k move, Enter edits, Esc closes".to_string()
            }
            EditField::Type | EditField::Status => {
                "Left/Right cycles, Enter applies, j/k move, Esc closes".to_string()
            }
        }
    };
    Line::from(Span::styled(hint, Style::new().fg(Color::DarkGray)))
}

/// The armed typed field that owns the widget slot, if any (r[tui-edit]).
fn edit_slot(app: &App) -> Option<EditField> {
    let Some(edit) = &app.edit else {
        return None;
    };
    if edit.editing && matches!(edit.field, EditField::Title | EditField::Description) {
        Some(edit.field)
    } else {
        None
    }
}

/// True when `field` is the armed typed field owning the widget slot.
fn is_edit_slot(edit: &EditState, field: EditField) -> bool {
    edit.editing
        && matches!(field, EditField::Title | EditField::Description)
        && edit.field == field
}

/// Pane row count for a non-slot field: one, except the non-edited
/// Description keeps its line structure (split on '\n', mirroring the
/// detail pane) so multi-line descriptions stay parseable (bug 1bc).
fn field_row_count(edit: &EditState, issue: Option<&Issue>, field: EditField) -> usize {
    if field == EditField::Description && !is_edit_slot(edit, field) {
        let desc = issue
            .map(|i| i.description.clone().unwrap_or_default())
            .unwrap_or_default();
        return desc
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>()
            .len()
            .max(1);
    }
    1
}

/// Rows for a non-slot field. The Description's continuation lines render
/// indented under the first (label) row; all other fields are one row.
fn field_rows(edit: &EditState, issue: Option<&Issue>, field: EditField) -> Vec<Line<'static>> {
    let n = field_row_count(edit, issue, field);
    if n == 1 {
        return vec![field_row(edit, issue, field)];
    }
    let desc = issue
        .map(|i| i.description.clone().unwrap_or_default())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (i, part) in desc.split('\n').enumerate() {
        if i == 0 {
            out.push(field_row(edit, issue, field));
        } else {
            out.push(Line::raw(format!("  {part}")));
        }
    }
    out
}

/// Rows and heights for the rect-based edit pane (d31): `above` rows on
/// top, then the widget slot of `editor_height` rows, then `below` rows
/// (including the hint). When no typed field is armed the slot is empty
/// and every field is a row. `width` is the widget's render width (the
/// detail area's inner width) so the slot height equals the wrapped line
/// count of the widget content; the slot never grows so the hint row is
/// pushed off - it is clamped to what the area affords.
fn edit_pane_geometry(app: &App, width: usize) -> (usize, usize, usize) {
    let Some(edit) = &app.edit else {
        return (0, 0, 0);
    };
    let issue: Option<&Issue> = issue_with_project(app, edit.issue_id).map(|(i, _)| i);
    let mut above = 0;
    let mut below = 0;
    let mut editor_height = 0;
    for (i, field) in EDIT_FIELDS.iter().enumerate() {
        if is_edit_slot(edit, *field) {
            editor_height = edit_area_height(&edit.textarea, width);
            continue;
        }
        let rows = field_row_count(edit, issue, *field);
        if i < field_index(edit.field) {
            above += rows;
        } else {
            below += rows;
        }
    }
    // The slot grows with the widget's wrapped content; render_edit_pane
    // clamps it to what the detail area actually affords (bug 2c0 - the old
    // 'available' clamp capped the editor by the pane's OTHER rows, so a
    // multi-line description got a 3-4 line window in any terminal).
    (above, editor_height.max(1), below)
}

/// Render the inline edit pane into the detail area: field rows above and
/// below, the armed `TextArea` in its reserved rect (height = wrapped line
/// count of its content), and the hint at the bottom. The pane reserves
/// rects and never paints the widget over field rows (d31).
fn render_edit_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(ref edit) = &app.edit else {
        return;
    };
    let short = edit.issue_id.to_string()[..8].to_string();
    let block = Block::bordered().title(format!("edit: {short}"));
    let inner = block.inner(area);
    let issue: Option<&Issue> = issue_with_project(app, edit.issue_id).map(|(i, _)| i);
    let slot = edit_slot(app);
    // Rect-based layout (d31): field rows keep their order with the armed
    // typed field's place reserved for its slot; the slot owns (1) the
    // active field's label row and (2) the widget rect with the wrapped
    // content's line count. Nothing is painted over the field rows.
    let mut rows: Vec<Line<'static>> = Vec::new();
    for field in EDIT_FIELDS {
        if slot == Some(field) {
            continue;
        }
        for r in field_rows(edit, issue, field) {
            rows.push(r);
        }
    }
    let (_, slot_h_raw, _) = edit_pane_geometry(app, inner.width as usize);
    // slot_h = label row + wrapped content rows (or just the content when
    // the slot is empty); kept tight so the field rows layout around it.
    let slot_h = if slot.is_some() {
        (1 + (slot_h_raw.max(1) - 1)).min(
            (inner.height as usize)
                .saturating_sub(rows.len() + 1)
                .max(1),
        )
    } else {
        0
    };
    let slot_index = slot.map(|f| field_index(f)).unwrap_or(0);
    let upper: Vec<Line<'static>> = rows[..slot_index].iter().cloned().collect();
    let mut lower: Vec<Line<'static>> = rows[slot_index..].iter().cloned().collect();
    lower.push(edit_hint(edit));
    let [top_area, slot_area, bottom_area] = Layout::vertical([
        Constraint::Length(upper.len() as u16),
        Constraint::Length(slot_h as u16),
        Constraint::Min(0),
    ])
    .areas(inner);
    let highlight = Style::new().bg(Color::Rgb(60, 60, 60));
    if !upper.is_empty() {
        f.render_widget(
            Paragraph::new(upper)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .style(highlight),
            top_area,
        );
    }
    if slot.is_some() {
        // The slot's first row is the active field's label (marker + bold
        // label, value lives in the widget below); the widget gets its own
        // rect under it - never over the label or the field rows.
        f.render_widget(
            Paragraph::new(vec![field_row(edit, issue, edit.field)]).style(highlight),
            Rect {
                x: inner.x,
                y: slot_area.y,
                width: slot_area.width,
                height: 1,
            },
        );
        let mut ta = textarea_for(edit.text(), edit.cursor());
        ta.set_style(highlight);
        ta.set_wrap_mode(WrapMode::WordOrGlyph);
        let slot_rect = Rect {
            x: inner.x + 2,
            y: slot_area.y + 1,
            width: slot_area.width.saturating_sub(2),
            height: slot_area.height.saturating_sub(1),
        };
        f.render_widget(&ta, slot_rect);
    }
    if !lower.is_empty() {
        f.render_widget(
            Paragraph::new(lower).wrap(ratatui::widgets::Wrap { trim: false }),
            bottom_area,
        );
    }
    f.render_widget(block, area);
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Relationship;
    use crate::storage::LoadWarning;
    use ratatui::backend::TestBackend;
    use std::time::{Duration, UNIX_EPOCH};

    /// A Press key event for the edit pane's key handler (the event loop
    /// delivers crossterm KeyEvents; tests build them from a KeyCode).
    fn key_event(code: KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::empty())
    }

    fn warning(message: &str) -> LoadWarning {
        LoadWarning {
            file_name: "bug-11111111-x.json".to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn fold_warnings_empty_is_message_unchanged() {
        assert_eq!(
            fold_warnings("\u{2713} Done".to_string(), &[]),
            "\u{2713} Done"
        );
    }

    #[test]
    fn fold_warnings_appends_first_warning_and_more_count() {
        assert_eq!(
            fold_warnings("\u{2713} Done".to_string(), &[warning("k1")]),
            "\u{2713} Done; WARNING: k1"
        );
        assert_eq!(
            fold_warnings("\u{2713} Done".to_string(), &[warning("k1"), warning("k2")]),
            "\u{2713} Done; WARNING: k1 (+1 more)"
        );
    }

    fn issue(uuid_suffix: u128, title: &str, status: Status, issue_type: IssueType) -> Issue {
        let mut i = Issue::new(title.to_string(), None, issue_type);
        i.id = Uuid::from_u128(uuid_suffix);
        i.status = status;
        i
    }

    fn rel(source: &mut Issue, relation_type: RelationType, target: Uuid) {
        source.relationships.push(Relationship {
            relation_type,
            target_id: target,
        });
    }

    fn pd(root: &str, issues: Vec<Issue>) -> ProjectData {
        ProjectData {
            root: PathBuf::from(root),
            name: PathBuf::from(root)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            related_titles: issues.iter().map(|i| (i.id, i.title.clone())).collect(),
            incoming: build_incoming_relationships_map(&issues)
                .into_iter()
                .map(|(uuid, rels)| {
                    (
                        uuid,
                        rels.into_iter()
                            .map(|r| (r.source_id, r.source_title, r.relation_type))
                            .collect(),
                    )
                })
                .collect(),
            issues,
            warnings: Vec::new(),
            index_mtime: None,
        }
    }

    fn project_a_issues() -> Vec<Issue> {
        let epic_id = Uuid::from_u128(0x4444);
        let mut child_todo = issue(0x2222, "Todo child", Status::Todo, IssueType::Bug);
        rel(&mut child_todo, RelationType::PartOf, epic_id);
        let epic = issue(0x4444, "The epic", Status::Todo, IssueType::Epic);
        vec![child_todo, epic]
    }

    fn app_with(pds: Vec<ProjectData>) -> App {
        let mut app = App::new();
        app.set_projects(pds);
        app
    }

    fn draw_once(app: &mut App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
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
        if !app.selection_visible() {
            app.reset_selection_to_first();
        }
        draw_once(app)
    }

    fn row_of(rows: &[String], needle: &str) -> Option<usize> {
        rows.iter().position(|r| r.contains(needle))
    }

    // ---- pure helper tests (kept from the single-project iteration) ----

    #[test]
    fn epic_progress_line() {
        let issues = project_a_issues();
        let counts = build_epic_counts(&issues);
        let epic = issues
            .iter()
            .find(|i| i.issue_type == IssueType::Epic)
            .unwrap();
        assert!(epic_line(epic, &counts)
            .to_string()
            .contains("The epic (0/1)"));
    }

    #[test]
    fn detail_lines_split_multiline_description() {
        let mut i = issue(0x1234, "Titled", Status::Todo, IssueType::Bug);
        i.description = Some("first line\nsecond line".to_string());
        let lines = detail_lines(&i, &HashMap::new(), &HashMap::new())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        let first = lines
            .iter()
            .position(|l| l.contains("Description: first line"))
            .expect("first");
        assert!(lines[first + 1].contains("second line"), "{lines:?}");
    }

    #[test]
    fn detail_lines_show_relationships_both_ways() {
        let issues = project_a_issues();
        let epic = issues
            .iter()
            .find(|i| i.issue_type == IssueType::Epic)
            .unwrap();
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
        let lines = detail_lines(epic, &related, &incoming)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        assert!(lines.iter().any(|l| l.contains("ID:")), "has id line");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("part of") || l.contains("has part")),
            "has relationship"
        );
    }

    // ---- multi-project tree (r[project-registry]) ----

    #[test]
    fn tree_nests_projects_above_epics() {
        let a = pd("/repos/alpha", project_a_issues());
        let b = pd(
            "/repos/beta",
            vec![issue(0x3333, "Orphan", Status::Proposed, IssueType::Design)],
        );
        let app = app_with(vec![a, b]);
        assert_eq!(app.items().len(), 2, "two project nodes");
        assert_eq!(
            app.items()[0].children().len(),
            1,
            "alpha subtree top level: one epic"
        );
        assert_eq!(app.items()[1].children().len(), 1, "beta: one orphan leaf");
    }

    #[test]
    fn project_line_shows_name_and_total() {
        let a = pd("/repos/alpha", project_a_issues());
        assert!(
            project_line(&a).to_string().contains("alpha"),
            "name: {:?}",
            project_line(&a).to_string()
        );
        assert!(project_line(&a).to_string().contains("(2)"), "total count");
    }

    #[test]
    fn project_detail_lines_show_path_counts_and_mtime() {
        let mut a = pd("/repos/alpha", project_a_issues());
        a.index_mtime = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let lines = project_detail_lines(&a)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        assert!(
            lines.iter().any(|l| l.contains("/repos/alpha")),
            "path: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("Issues: 2")),
            "total: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("todo 2")),
            "per-status counts: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Last index update") || l.contains("unknown")),
            "mtime line"
        );
    }

    #[test]
    fn selection_across_projects_routes_to_own_project() {
        let a = pd("/repos/alpha", project_a_issues());
        let b = pd(
            "/repos/beta",
            vec![issue(0x3333, "Orphan", Status::Proposed, IssueType::Design)],
        );
        let app = app_with(vec![a, b]);
        let b_uuid = Uuid::from_u128(0x3333);
        let (issue, pd_b) = {
            let issue = app
                .projects
                .iter()
                .find(|p| p.root == PathBuf::from("/repos/beta"))
                .unwrap()
                .issues
                .iter()
                .find(|i| i.id == b_uuid)
                .unwrap()
                .title
                .clone();
            let project = app.project_of(&b_uuid).unwrap();
            (issue, project.root.clone())
        };
        assert_eq!(issue, "Orphan");
        assert_eq!(
            pd_b,
            PathBuf::from("/repos/beta"),
            "mutations route to the issue's project"
        );
        assert!(
            app.project_of(&Uuid::from_u128(0x4444)).is_some(),
            "alpha issue found too"
        );
    }

    #[test]
    fn selection_visible_checks_recursively() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        // child uuid nested inside project > epic
        let child_uuid = Uuid::from_u128(0x2222).to_string();
        app.tree_state_mut().select(vec![
            "/repos/alpha".to_string(),
            Uuid::from_u128(0x4444).to_string(),
            child_uuid.clone(),
        ]);
        assert!(
            items_contain(app.items(), &child_uuid),
            "recursive containment"
        );
        assert!(app.selection_visible());
        app.tree_state_mut()
            .select(vec!["/repos/nowhere".to_string()]);
        assert!(!app.selection_visible(), "unknown root not visible");
    }

    // ---- selection re-settle after a mutation (r[tui-review]) ----

    #[test]
    fn tree_order_flattens_pre_order() {
        let a = pd("/repos/alpha", project_a_issues());
        let app = app_with(vec![a]);
        let order = tree_order(app.items());
        assert_eq!(
            order,
            vec![
                "/repos/alpha".to_string(),
                Uuid::from_u128(0x4444).to_string(),
                Uuid::from_u128(0x2222).to_string(),
            ],
            "project > epic > child in key_down order"
        );
    }

    #[test]
    fn last_leaf_path_is_the_deep_last_issue() {
        let a = pd("/repos/alpha", project_a_issues());
        let app = app_with(vec![a]);
        let path = last_leaf_path(app.items(), &mut Vec::new()).unwrap();
        assert_eq!(
            path,
            vec![
                "/repos/alpha".to_string(),
                Uuid::from_u128(0x4444).to_string(),
                Uuid::from_u128(0x2222).to_string(),
            ],
            "deep-last leaf under project > epic"
        );
    }

    #[test]
    fn selection_targets_next_follows_in_tree_order() {
        let b = pd(
            "/repos/beta",
            vec![
                issue(0x3333, "Alpha", Status::Proposed, IssueType::Bug),
                issue(0x4444, "Beta", Status::Proposed, IssueType::Design),
                issue(0x5555, "Gamma", Status::Proposed, IssueType::Usecase),
            ],
        );
        let mut app = app_with(vec![b]);
        app.set_filter(Some(Status::Proposed));
        app.reset_selection_to_first();
        let (same, next) = app.selection_targets();
        assert_eq!(same, Some(Uuid::from_u128(0x3333).to_string()));
        assert_eq!(
            next,
            Some(Uuid::from_u128(0x4444).to_string()),
            "the item after the selection in tree order"
        );
        // Move to the last item: no successor.
        app.tree_state_mut().select(vec![
            "/repos/beta".to_string(),
            Uuid::from_u128(0x5555).to_string(),
        ]);
        let (same, next) = app.selection_targets();
        assert_eq!(same, Some(Uuid::from_u128(0x5555).to_string()));
        assert_eq!(next, None, "last visible item has no successor");
    }

    #[test]
    fn reset_selection_to_last_picks_the_last_visible_issue() {
        let b = pd(
            "/repos/beta",
            vec![
                issue(0x3333, "Alpha", Status::Proposed, IssueType::Bug),
                issue(0x4444, "Beta", Status::Proposed, IssueType::Design),
            ],
        );
        let mut app = app_with(vec![b]);
        app.set_filter(Some(Status::Proposed));
        app.reset_selection_to_last();
        assert_eq!(
            app.tree_state().selected().last(),
            Some(&Uuid::from_u128(0x4444).to_string()),
            "new last item selected"
        );
        assert!(app.selection_visible());
    }

    #[test]
    fn proposed_filter_lists_per_project_and_hides_empty_projects() {
        let a = pd("/repos/alpha", project_a_issues()); // nothing proposed
        let b = pd(
            "/repos/beta",
            vec![issue(0x3333, "Orphan", Status::Proposed, IssueType::Design)],
        );
        let mut app = app_with(vec![a, b]);
        app.set_filter(Some(Status::Proposed));
        app.tree_state_mut().open(vec!["/repos/beta".to_string()]);
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("beta")),
            "project with matches shown"
        );
        assert!(
            !rows.iter().any(|r| r.contains("alpha")),
            "empty project hidden under filter"
        );
        assert!(
            rows.iter().any(|r| r.contains("Orphan")),
            "proposed leaf visible"
        );
    }

    // ------------------------------------------------------------------
    // Derived render cache: `items` must be recomputed inside every
    // mutation path (filter, search, projects, refresh/reload), never by
    // callers remembering to rebuild - staleness is impossible by
    // construction. Read back through `app.items()` after each mutation.
    // ------------------------------------------------------------------

    #[test]
    fn derived_tree_follows_filter_and_search_via_key_handlers() {
        // alpha: "Todo child" (bug, todo) + "The epic" (epic, todo), with
        // nothing proposed.
        let mut app = app_with(vec![pd("/repos/alpha", project_a_issues())]);

        // Filter path: `r` toggles the review filter through tree_key.
        tree_key(&mut app, KeyCode::Char('r')).unwrap();
        assert_eq!(app.filter, Some(Status::Proposed));
        assert!(
            app.items().is_empty(),
            "proposed filter hides alpha (no proposals)"
        );
        tree_key(&mut app, KeyCode::Char('r')).unwrap();
        assert_eq!(app.filter, None);
        assert_eq!(app.items().len(), 1, "filter cleared: project node back");
        assert_eq!(app.items()[0].children().len(), 1, "epic node restored");

        // Search path: each keystroke must land a rebuilt tree.
        app.focus = Focus::Search;
        search_key(&mut app, KeyCode::Char('t')).unwrap(); // "t"
        assert_eq!(
            app.items()[0].children().len(),
            2,
            "both titles contain 't'"
        );
        search_key(&mut app, KeyCode::Char('o')).unwrap(); // "to"
        assert_eq!(
            app.items()[0].children().len(),
            1,
            "'to' only matches 'Todo child'"
        );
        assert!(
            app.items()[0]
                .children()
                .iter()
                .all(|c| c.children().is_empty()),
            "search flattens the tree to leaves"
        );
        search_key(&mut app, KeyCode::Char('t')).unwrap(); // "tot"
        assert!(app.items().is_empty(), "no matches: tree empties live");
        search_key(&mut app, KeyCode::Backspace).unwrap(); // "to"
        assert_eq!(
            app.items()[0].children().len(),
            1,
            "backspace restores the match"
        );
        search_key(&mut app, KeyCode::Esc).unwrap(); // clears
        assert_eq!(app.search, "");
        assert_eq!(
            app.items()[0].children().len(),
            1,
            "Esc restores the unfiltered tree shape"
        );
    }

    #[test]
    fn set_projects_rebuilds_the_derived_tree() {
        // The projects mutation path (the pure half of `load`; a refresh
        // would deliver the same kind of new issue set).
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        assert_eq!(app.items()[0].children().len(), 1, "one epic subtree");

        let mut issues = project_a_issues();
        issues.push(issue(0x6666, "Extra", Status::Todo, IssueType::Bug));
        app.set_projects(vec![pd("/repos/alpha", issues)]);
        assert_eq!(
            app.items()[0].children().len(),
            2,
            "new orphan leaf visible through the next access"
        );
        let new_uuid = Uuid::from_u128(0x6666).to_string();
        assert!(
            items_contain(app.items(), &new_uuid),
            "the new issue's node is in the rebuilt tree"
        );
    }

    #[test]
    fn settle_selection_rebuilds_after_project_changes() {
        // The refresh/reload path (r[tui-refresh], r[tui-review]) mutates
        // the projects list and then re-settles via settle_selection: the
        // rebuild must live inside it, so a refresh can never leave the
        // derived tree stale.
        let a = pd("/repos/alpha", project_a_issues()); // nothing proposed
        let mut app = app_with(vec![a]);
        app.set_filter(Some(Status::Proposed));
        assert!(
            app.items().is_empty(),
            "filter applied: alpha has no proposals"
        );

        // A refresh-style change: a new project with a proposal appears.
        let b = pd(
            "/repos/beta",
            vec![issue(0x3333, "Orphan", Status::Proposed, IssueType::Design)],
        );
        app.projects.push(b); // refresh_projects mutates `projects` directly
        let (same, next) = app.selection_targets();
        app.settle_selection(same, next);

        assert_eq!(app.items().len(), 1, "rebuilt tree shows the new project");
        let proposal = Uuid::from_u128(0x3333).to_string();
        assert!(
            items_contain(app.items(), &proposal),
            "the proposal's node is in the rebuilt tree"
        );
        assert!(app.selection_visible(), "selection re-settled onto it");
    }

    #[test]
    fn children_sort_proposed_before_todo_before_done_then_bug_before_rest() {
        let epic_id = Uuid::from_u128(0x4444);
        let mut done_design = issue(0x7777, "Done design", Status::Done, IssueType::Design);
        rel(&mut done_design, RelationType::PartOf, epic_id);
        let mut issues = project_a_issues();
        issues.push(done_design);
        let a = pd("/repos/alpha", issues);
        let mut app = app_with(vec![a]);
        app.tree_state_mut().open(vec!["/repos/alpha".to_string()]);
        app.tree_state_mut().open(vec![
            "/repos/alpha".to_string(),
            Uuid::from_u128(0x4444).to_string(),
        ]);
        let rows = draw(&mut app);
        let todo_bug = row_of(&rows, "Todo child").unwrap();
        let done_design_row = row_of(&rows, "Done design").unwrap();
        assert!(todo_bug < done_design_row, "{rows:?}");
    }

    #[test]
    fn browser_renders_projects_tree_and_detail() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        app.tree_state_mut().open(vec!["/repos/alpha".to_string()]);
        app.tree_state_mut().open(vec![
            "/repos/alpha".to_string(),
            Uuid::from_u128(0x4444).to_string(),
        ]);
        app.tree_state_mut().select(vec![
            "/repos/alpha".to_string(),
            Uuid::from_u128(0x4444).to_string(),
        ]);
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("alpha")),
            "project node: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("The epic")),
            "epic under project: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("Status:")),
            "detail fields: {rows:?}"
        );
    }

    #[test]
    fn right_unfolds_and_left_folds_within_project() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        let tree_side = |rows: &[String]| {
            rows.iter()
                .map(|r| r.chars().take(34).collect::<String>())
                .collect::<Vec<_>>()
        };
        // Startup auto-opens down to the first issue (selection visible).
        let rows_open = tree_side(&draw(&mut app));
        assert!(
            rows_open.iter().any(|r| r.contains("The epic")),
            "project auto-opened: {rows_open:?}"
        );
        assert!(
            rows_open.iter().any(|r| r.contains("Todo child")),
            "first issue selected & visible"
        );
        app.tree_state_mut().key_up(); // move onto the epic
        let _ = draw(&mut app);
        app.tree_state_mut().key_left(); // fold the (selected, open) epic
        let rows_folded = tree_side(&draw(&mut app));
        assert!(
            !rows_folded.iter().any(|r| r.contains("Todo child")),
            "Left folds epic"
        );
        app.tree_state_mut().key_right(); // Right re-unfolds the epic
        let rows_unfolded = tree_side(&draw(&mut app));
        assert!(
            rows_unfolded.iter().any(|r| r.contains("Todo child")),
            "Right unfolds epic"
        );
    }

    #[test]
    fn help_overlay_is_opaque() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        app.mode = Mode::Help;
        let rows = draw(&mut app);
        let joined = rows.join("\n");
        assert!(
            joined.contains("help (any key to close)"),
            "help shown: {joined}"
        );
        let under: Vec<&String> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| (7..=16).contains(i))
            .map(|(_, r)| r)
            .collect();
        assert!(
            under.iter().any(|r| r.contains("move")),
            "help body: {under:?}"
        );
        assert!(
            !under
                .iter()
                .any(|r| r.contains("No selection") || r.contains("Status:")),
            "browser leaked through: {under:?}"
        );
    }

    // ---- search query (r[tui-search]) ----

    #[test]
    fn parse_search_query_splits_terms_and_classifies_prefixes() {
        let q = parse_search_query("pane edit s:t t:bug s:!d");
        assert_eq!(
            q.terms,
            vec![
                QueryTerm::Text("pane".to_string(), false),
                QueryTerm::Text("edit".to_string(), false),
                QueryTerm::Status(Status::Todo, false),
                QueryTerm::Type(IssueType::Bug, false),
                QueryTerm::Status(Status::Done, true),
            ]
        );
    }

    #[test]
    fn parse_search_query_handles_case_aliases_negation_and_dead_terms() {
        assert_eq!(
            parse_search_query("S:T t:UX").terms,
            vec![
                QueryTerm::Status(Status::Todo, false),
                QueryTerm::Type(IssueType::UxProblem, false),
            ]
        );
        // A bare term matches its exact substring: split on spaces only.
        assert_eq!(
            parse_search_query("pane-edit").terms,
            vec![QueryTerm::Text("pane-edit".to_string(), false)]
        );
        // `!` negates a bare term, and terms combine with AND.
        assert_eq!(
            parse_search_query("!pane edit").terms,
            vec![
                QueryTerm::Text("pane".to_string(), true),
                QueryTerm::Text("edit".to_string(), false),
            ]
        );
        // Full status and type names work too.
        assert_eq!(
            parse_search_query("s:in_progress t:milestone").terms,
            vec![
                QueryTerm::Status(Status::InProgress, false),
                QueryTerm::Type(IssueType::Milestone, false),
            ]
        );
        // Unparseable specs and bare punctuation match nothing.
        assert_eq!(parse_search_query("s:xyz").terms, vec![QueryTerm::Dead]);
        assert_eq!(
            parse_search_query("t:9 s:!").terms,
            vec![QueryTerm::Dead, QueryTerm::Dead]
        );
        // Punctuation no longer forces a dead term: it matches literally.
        assert_eq!(
            parse_search_query("---").terms,
            vec![QueryTerm::Text("---".to_string(), false)]
        );
        // Empty and whitespace-only queries compile to no terms (unfiltered).
        assert_eq!(parse_search_query("").terms, vec![]);
        assert_eq!(parse_search_query("   ").terms, vec![]);
    }

    #[test]
    fn text_term_matches_exact_substrings_case_insensitively() {
        assert!(text_matches("pane", "Edit pane"));
        assert!(text_matches("edit", "Edit pane"));
        assert!(text_matches("e", "Edit pane"), "substring anywhere");
        assert!(!text_matches("xyz", "Edit pane"));
        assert!(!text_matches("", "anything"), "empty term never matches");
    }

    #[test]
    fn text_term_matches_short_id_by_substring() {
        let short_id = "cb08a6e4";
        assert!(short_id_matches(short_id, "cb"));
        assert!(short_id_matches(short_id, "cb0"));
        assert!(
            short_id_matches(short_id, "08a"),
            "substring, not just prefix"
        );
        assert!(
            short_id_matches(short_id, "CB08A6E4"),
            "case-insensitive id match"
        );
        assert!(!short_id_matches(short_id, "ab"));
        assert!(!short_id_matches(short_id, ""));
    }

    fn issue_with_id(title: &str, description: Option<&str>) -> Issue {
        let mut i = issue(0x2222, title, Status::Todo, IssueType::Bug);
        i.id = Uuid::parse_str("cb08a6e4-1111-4222-8333-000000000001").unwrap();
        i.description = description.map(|d| d.to_string());
        i
    }

    #[test]
    fn issue_matches_query_ands_all_terms() {
        let i = issue_with_id("Edit pane", Some("Run the linter"));
        assert!(issue_matches_query(&i, &parse_search_query("pane edit")));
        assert!(
            issue_matches_query(&i, &parse_search_query("edit pane")),
            "term order irrelevant"
        );
        assert!(
            issue_matches_query(&i, &parse_search_query("pan")),
            "exact substring: pan matches pane"
        );
        assert!(!issue_matches_query(
            &i,
            &parse_search_query("pane linter nosuch")
        ));
        assert!(
            issue_matches_query(&i, &parse_search_query("cb0")),
            "short-id substring"
        );
        assert!(
            issue_matches_query(&i, &parse_search_query("linter")),
            "description substring"
        );
        assert!(issue_matches_query(&i, &parse_search_query("s:t cb0")));
        assert!(!issue_matches_query(&i, &parse_search_query("s:done")));
        assert!(issue_matches_query(&i, &parse_search_query("s:!done")));
        assert!(issue_matches_query(&i, &parse_search_query("t:bug")));
        assert!(!issue_matches_query(&i, &parse_search_query("t:epic")));
        assert!(issue_matches_query(&i, &parse_search_query("t:!epic")));
        // `!` negates a bare term too.
        assert!(!issue_matches_query(&i, &parse_search_query("!pane")));
        assert!(issue_matches_query(&i, &parse_search_query("!xyz")));
        assert!(
            !issue_matches_query(&i, &parse_search_query("!pane xyz")),
            "!pane (false here) AND xyz stays false"
        );
        assert!(issue_matches_query(&i, &parse_search_query("!xyz !nope")));
        assert!(!issue_matches_query(&i, &parse_search_query("!linter pan")));
        assert!(
            !issue_matches_query(&i, &parse_search_query("s:x")),
            "dead term matches nothing"
        );
        assert!(
            !issue_matches_query(&i, &parse_search_query("!")),
            "bare bang matches nothing"
        );
        // A term may match different fields than another term.
        assert!(issue_matches_query(
            &i,
            &parse_search_query("linter t:bug s:todo")
        ));
        // No terms: every issue matches (no filter).
        assert!(issue_matches_query(&i, &parse_search_query("")));
    }

    #[test]
    fn tree_items_flattens_matching_issues_under_search() {
        let issues = project_a_issues(); // "Todo child" (bug, todo) + "The epic" (epic, todo)
        let counts = build_epic_counts(&issues);
        // A text term matches only the child: "The epic" has no "todo" substring.
        let flat = tree_items(&issues, &counts, None, "todo").unwrap();
        assert_eq!(flat.len(), 1, "only 'Todo child' contains the word 'todo'");
        assert!(
            flat.iter().all(|c| c.children().is_empty()),
            "flat leaves under search"
        );
        // A status term matches both issues.
        let by_status = tree_items(&issues, &counts, None, "s:todo").unwrap();
        assert_eq!(by_status.len(), 2, "both issues are todo: {by_status:?}");
        let none = tree_items(&issues, &counts, None, "zzz").unwrap();
        assert!(none.is_empty(), "no match, no rows");
        let tree = tree_items(&issues, &counts, None, "").unwrap();
        assert!(
            tree.iter().any(|c| !c.children().is_empty()),
            "empty query keeps the tree"
        );
        // The review filter still combines with search (AND).
        let combined = tree_items(&issues, &counts, Some(Status::Proposed), "s:todo").unwrap();
        assert!(
            combined.is_empty(),
            "proposed AND todo matches nothing here"
        );
    }

    #[test]
    fn search_focus_shows_prompt_and_applies_live() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        app.focus = Focus::Search;
        app.set_search("todo".to_string());
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("search: todo")),
            "prompt shown: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("Todo child")),
            "match listed: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("The epic")),
            "second match listed: {rows:?}"
        );
        // Empty search drops the filter: tree shape returns.
        app.set_search("zzz".to_string());
        assert!(app.items().is_empty(), "no matches anywhere");
    }

    #[test]
    fn search_key_enter_keeps_filter_esc_clears() {
        let mut app = app_with(vec![pd("/repos/alpha", project_a_issues())]);
        app.focus = Focus::Search;
        app.set_search("todo".to_string());
        search_key(&mut app, KeyCode::Enter).unwrap();
        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.search, "todo", "Enter keeps the active filter");
        // Esc clears the search and restores the tree.
        app.focus = Focus::Search;
        search_key(&mut app, KeyCode::Esc).unwrap();
        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.search, "");
        assert_eq!(
            app.items()[0].children().len(),
            1,
            "project node back to one epic subtree"
        );
        // Tab moves focus away without closing the search; Backspace edits.
        app.focus = Focus::Search;
        app.search = "todo".to_string();
        search_key(&mut app, KeyCode::Tab).unwrap();
        assert_eq!(app.focus, Focus::Detail, "Tab cycles to the detail pane");
        assert_eq!(app.search, "todo", "the query survives a focus switch");
        app.focus = Focus::Search;
        search_key(&mut app, KeyCode::Backspace).unwrap();
        assert_eq!(app.search, "tod");
    }

    #[test]
    fn tab_cycles_focus_and_slash_jumps_to_search() {
        let mut app = app_with(vec![pd("/repos/alpha", project_a_issues())]);
        assert_eq!(app.focus, Focus::Tree);
        tree_key(&mut app, KeyCode::Tab).unwrap();
        assert_eq!(app.focus, Focus::Search);
        search_key(&mut app, KeyCode::Tab).unwrap();
        assert_eq!(app.focus, Focus::Detail);
        detail_key(&mut app, KeyCode::Tab).unwrap();
        assert_eq!(app.focus, Focus::Tree);
        // BackTab reverses; '/' jumps to the search from tree or detail.
        detail_key(&mut app, KeyCode::Char('/')).unwrap();
        assert_eq!(app.focus, Focus::Search);
        search_key(&mut app, KeyCode::BackTab).unwrap();
        assert_eq!(app.focus, Focus::Tree);
        tree_key(&mut app, KeyCode::BackTab).unwrap();
        assert_eq!(app.focus, Focus::Detail);
        // Esc in the detail pane returns to the tree.
        detail_key(&mut app, KeyCode::Esc).unwrap();
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn detail_keys_scroll_within_limits_and_quit() {
        let a = pd("/repos/alpha", project_a_issues());
        let mut app = app_with(vec![a]);
        app.focus = Focus::Detail;
        app.detail_offset = 0;
        detail_key(&mut app, KeyCode::Char('j')).unwrap();
        assert_eq!(app.detail_offset, 1, "j scrolls down");
        detail_key(&mut app, KeyCode::Char('k')).unwrap();
        assert_eq!(app.detail_offset, 0, "k scrolls back up");
        // The render clamps the offset to the line count; no underflow.
        detail_key(&mut app, KeyCode::Char('k')).unwrap();
        assert_eq!(app.detail_offset, 0, "offset clamps at zero");
        // q still quits from the detail pane.
        detail_key(&mut app, KeyCode::Char('q')).unwrap();
        assert!(app.should_quit);
    }

    // ---- stale background cells (r[tui-background]) ----

    #[test]
    fn first_frame_does_not_leak_previous_terminal_contents() {
        // Simulate a terminal whose alternate screen still shows earlier
        // content: seed the backend with "Z" before the app's first draw.
        let issues = vec![
            issue(0x1111, "Alpha", Status::Todo, IssueType::Bug),
            issue(0x2222, "Beta", Status::Todo, IssueType::Bug),
            issue(0x3333, "Gamma", Status::Todo, IssueType::Bug),
        ];
        let mut app = app_with(vec![pd("/repos/alpha", issues)]);
        let z_wall: Vec<String> = (0..24).map(|_| "Z".repeat(90)).collect();
        let backend = TestBackend::with_lines(z_wall);
        let mut terminal = Terminal::new(backend).unwrap();
        // run() clears the screen once after Terminal::new; without this the
        // first frame only repaints cells its widgets touch, and unpainted
        // cells would still show whatever the terminal held before the TUI.
        terminal.clear().unwrap();
        if !app.selection_visible() {
            app.reset_selection_to_first();
        }
        terminal.draw(|f| render_app(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                assert!(
                    buf[(x, y)].symbol() != "Z",
                    "stale previous-terminal cell at ({x},{y})"
                );
            }
        }
    }

    // ---- inline edit pane (r[tui-edit]) ----

    fn edit_app() -> (App, Uuid) {
        let epic_id = Uuid::from_u128(0x4444);
        let mut app = app_with(vec![pd("/repos/alpha", project_a_issues())]);
        app.tree_state_mut()
            .select(vec!["/repos/alpha".to_string(), epic_id.to_string()]);
        open_edit(&mut app).unwrap();
        (app, epic_id)
    }

    /// An edit state backed by a real on-disk store (`.no_jj` marker, so no
    /// jj integration): commits go through the operations layer and the
    /// reload path like in production.
    fn edit_app_ops() -> (tempfile::TempDir, App, Uuid) {
        let tempdir = tempfile::Builder::new()
            .prefix("pebbles-tui-edit-")
            .tempdir()
            .expect("tempdir");
        let root = tempdir.path().to_path_buf();
        let pebbles = root.join(".pebbles");
        std::fs::create_dir_all(pebbles.join("issues")).expect("issues dir");
        std::fs::write(pebbles.join(".no_jj"), "").expect("no_jj marker");
        // Short id is the first 8 chars of the uuid string (models short_id):
        // build the uuid so `00007777` is both the file short and the uuid
        // prefix (same convention as the e2e fixtures).
        let id_u128: u128 = 0x7777;
        let id = Uuid::parse_str(&format!("{:08x}-1111-4222-8333-{:012x}", id_u128, id_u128))
            .expect("fixture uuid");
        let file = "bug-00007777-x.json".to_string();
        let issue = format!(
            "{{\"id\":\"{id}\",\"title\":\"Solo bug\",\"description\":null,\"status\":\"todo\",\"issue_type\":\"bug\",\"tags\":[],\"relationships\":[],\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n"
        );
        std::fs::write(pebbles.join("issues").join(&file), issue).expect("issue file");
        std::fs::write(
            pebbles.join("index.json"),
            format!("{{\"issues\":[{{\"id\":\"{id}\",\"file_name\":\"{file}\"}}]}}"),
        )
        .expect("index");
        let mut app = App::load(&[root.clone()]).expect("load temp project");
        app.tree_state_mut()
            .select(vec![root.to_string_lossy().to_string(), id.to_string()]);
        open_edit(&mut app).unwrap();
        (tempdir, app, id)
    }

    fn issue_file(root: &std::path::Path, id: Uuid) -> std::path::PathBuf {
        let short = id.to_string()[..8].to_string();
        let files: Vec<std::path::PathBuf> = root
            .join(".pebbles/issues")
            .read_dir()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains(&short))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(files.len(), 1, "one matching issue file for {id}");
        files.into_iter().next().unwrap()
    }

    #[test]
    fn step_field_wraps_and_cycles_cover_all_values() {
        assert_eq!(step_field(EditField::Title, 1), EditField::Description);
        assert_eq!(step_field(EditField::Title, -1), EditField::Status);
        assert_eq!(step_field(EditField::Status, 1), EditField::Title);
        assert_eq!(cycle_status(Status::Todo, 1), Status::InProgress);
        assert_eq!(cycle_status(Status::Blocked, 1), Status::Proposed, "wraps");
        assert_eq!(cycle_status(Status::Proposed, -1), Status::Blocked);
        assert_eq!(cycle_type(IssueType::Bug, 1), IssueType::UxProblem);
        assert_eq!(cycle_type(IssueType::Milestone, 1), IssueType::Bug, "wraps");
        assert_eq!(cycle_type(IssueType::Epic, -1), IssueType::Validation);
    }

    #[test]
    fn open_edit_requires_an_issue_selection() {
        let mut app = app_with(vec![pd("/repos/alpha", project_a_issues())]);
        // A project node is selected, not an issue: the pane does not open.
        app.tree_state_mut()
            .select(vec!["/repos/alpha".to_string()]);
        detail_key(&mut app, KeyCode::Char('e')).unwrap();
        assert_eq!(app.mode, Mode::Browser, "no edit mode without an issue");
        assert!(
            app.message.as_deref().unwrap().contains("Select an issue"),
            "hint: {:?}",
            app.message
        );
        // With an issue selected the pane opens pinned to it, with the live
        // values as the pending defaults.
        let (mut app, epic_id) = edit_app();
        let edit = app.edit.as_ref().unwrap();
        assert_eq!(edit.issue_id, epic_id);
        assert_eq!(edit.field, EditField::Title);
        assert!(!edit.editing);
        assert_eq!(edit.pending_type, IssueType::Epic);
        assert_eq!(edit.pending_status, Status::Todo);
        close_edit(&mut app);
        assert_eq!(app.mode, Mode::Browser);
        assert!(app.edit.is_none());
    }

    #[test]
    fn edit_pane_non_active_rows_show_live_values_while_editing() {
        let (mut app, epic_id) = edit_app();
        // Give the epic a description so the live description is visible.
        let issue = app
            .projects
            .iter_mut()
            .flat_map(|p| p.issues.iter_mut())
            .find(|i| i.id == epic_id)
            .unwrap();
        issue.description = Some("Two lines\nhere".to_string());
        // Enter arms the title edit; every other row must show the live
        // value (the display bug: the shared empty buffer + cursor took
        // over the other text rows while editing any field).
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert!(app.edit.as_ref().unwrap().editing);
        let rows = draw(&mut app);
        let joined = rows.join("\n");
        assert!(
            joined.contains("The epic"),
            "title content in the widget slot: {joined}"
        );
        assert!(
            joined.contains("Description: Two lines"),
            "description first line shows in the live row: {joined}"
        );
        assert!(
            joined.contains("  here"),
            "description continuation line keeps its line break (bug 1bc): {joined}"
        );
        assert!(
            !joined.contains("Two lines here"),
            "description no longer flattens to one line (bug 1bc): {joined}"
        );
        assert!(joined.contains("Type: epic"), "{joined}");
        assert!(joined.contains("Status: todo"), "{joined}");
        assert!(
            !joined.contains("Two lines here\u{258f}"),
            "no cursor glyph on non-active rows (widget draws its own cursor): {joined}"
        );
    }

    fn edit_pane_renders_fields_pending_values_and_hints() {
        let (mut app, _) = edit_app();
        let rows = draw(&mut app);
        let joined = rows.join("\n");
        assert!(joined.contains("> Title: The epic"), "{joined}");
        assert!(joined.contains("Type: epic"), "{joined}");
        assert!(joined.contains("Status: todo"), "{joined}");
        assert!(joined.contains("j/k move, Enter edits"), "{joined}");
        // Cycling the pending status is visible without a commit.
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        assert_eq!(
            app.edit.as_ref().unwrap().field,
            EditField::Status,
            "three j presses land on Status"
        );
        edit_key(&mut app, key_event(KeyCode::Right)).unwrap();
        assert_eq!(
            app.edit.as_ref().unwrap().pending_status,
            Status::InProgress
        );
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("> Status: in_progress")),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("Left/Right cycles, Enter applies")),
            "cycled-field hint: {rows:?}"
        );
        // Esc in navigation closes the pane without writing: the uncommitted
        // cycle is dropped with it.
        edit_key(&mut app, key_event(KeyCode::Esc)).unwrap();
        assert_eq!(app.mode, Mode::Browser);
        assert!(app.edit.is_none());
    }

    #[test]
    fn edit_text_types_backspaces_and_esc_discards() {
        let (mut app, _) = edit_app();
        // Enter starts editing the title with the live value pre-filled.
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        let edit = app.edit.as_ref().unwrap();
        assert!(edit.editing);
        assert_eq!(edit.text(), "The epic");
        // q types while editing instead of quitting.
        edit_key(&mut app, key_event(KeyCode::Char('q'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('!'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('?'))).unwrap();
        assert!(!app.should_quit, "q appends while editing");
        assert_eq!(app.edit.as_ref().unwrap().text(), "The epicq!?");
        edit_key(&mut app, key_event(KeyCode::Backspace)).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().text(), "The epicq!");
        // The buffer is rendered in the reserved widget slot while editing.
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("The epicq!")),
            "widget content visible: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("Enter applies, Esc cancels")),
            "typing hint: {rows:?}"
        );
        // Esc discards the pending edit: no operation ran, the buffer is
        // dropped and the pane returns to navigation.
        edit_key(&mut app, key_event(KeyCode::Esc)).unwrap();
        let edit = app.edit.as_ref().unwrap();
        assert!(!edit.editing, "Esc returns to navigation");
        assert_eq!(app.mode, Mode::Edit, "Esc on a field edit keeps the pane");
        // Esc in navigation closes the pane.
        edit_key(&mut app, key_event(KeyCode::Esc)).unwrap();
        assert_eq!(app.mode, Mode::Browser);
        assert!(app.edit.is_none());
    }

    #[test]
    fn edit_enter_from_navigation_only_starts_editing() {
        let (mut app, _) = edit_app();
        // One Enter on a typed field must not apply anything: it enters
        // editing mode (cycled fields do apply on the first Enter).
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        edit_key(&mut app, key_event(KeyCode::Esc)).unwrap();
        let rows = draw(&mut app);
        assert!(
            rows.iter().any(|r| r.contains("> Title: The epic")),
            "title untouched after Enter + Esc: {rows:?}"
        );
        // Tab moves to the next field; BackTab back.
        edit_key(&mut app, key_event(KeyCode::Tab)).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::Description);
        edit_key(&mut app, key_event(KeyCode::BackTab)).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::Title);
    }

    #[test]
    fn q_quits_from_edit_navigation() {
        let (mut app, _) = edit_app();
        edit_key(&mut app, key_event(KeyCode::Char('q'))).unwrap();
        assert!(app.should_quit, "q quits while navigating the pane");
    }

    // ---- widget probe (ratatui-textarea adoption, d31) ----

    #[test]
    fn textarea_probe_feeds_crossterm_keycodes() {
        let mut ta = ratatui_textarea::TextArea::from(vec!["The epic".to_string()]);
        // Cursor starts at the head; move it to the end so typed chars
        // append, mirroring the pane's arm-then-append behavior.
        ta.move_cursor(ratatui_textarea::CursorMove::Jump(
            0,
            ta.lines()[0].chars().count() as u16,
        ));
        let key_event = crossterm::event::KeyEvent::new(
            KeyCode::Char('q'),
            crossterm::event::KeyModifiers::empty(),
        );
        let input = ratatui_textarea::Input::from(key_event);
        assert!(ta.input(input), "typing modifies the buffer");
        assert_eq!(ta.lines(), vec!["The epicq".to_string()]);
        // Arrows move the cursor without touching the text.
        ta.move_cursor(ratatui_textarea::CursorMove::Back);
        assert_eq!(ta.cursor(), (0, 8));
    }

    // ---- in-buffer editing (r[tui-edit]) ----

    #[test]
    fn textarea_buffer_roundtrips_and_starts_cursor_at_end() {
        let b = textarea_for("The epic".to_string(), end_cursor("The epic".to_string()));
        assert_eq!(b.lines().join("\n"), "The epic");
        assert_eq!(
            textarea_lines_of(&b),
            (0, 8),
            "cursor at end for append typing"
        );
        let empty = textarea_for(String::new(), end_cursor(String::new()));
        assert!(empty.is_empty());
        assert_eq!(textarea_lines_of(&empty), (0, 0));
        // Multi-line text keeps its lines.
        let multi = textarea_for("one\ntwo".to_string(), end_cursor("one\ntwo".to_string()));
        assert_eq!(multi.lines(), vec!["one".to_string(), "two".to_string()]);
    }

    fn textarea_lines_of(ta: &TextArea<'static>) -> (usize, usize) {
        let cursor = ta.cursor();
        (cursor.0 as usize, cursor.1 as usize)
    }

    #[test]
    fn textarea_buffer_inserts_at_cursor_and_moves() {
        let mut b = textarea_for("abc".to_string(), end_cursor("abc".to_string()));
        move_left(&mut b);
        b.insert_char('X');
        assert_eq!(b.lines().join("\n"), "abXc");
        assert_eq!(textarea_lines_of(&b), (0, 3));
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        b.insert_char('0');
        assert_eq!(b.lines().join("\n"), "0abXc");
        b.move_cursor(ratatui_textarea::CursorMove::End);
        b.insert_char('Z');
        assert_eq!(b.lines().join("\n"), "0abXcZ");
        // Left at the start of line 0 is a no-op.
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        move_left(&mut b);
        assert_eq!(textarea_lines_of(&b), (0, 0));
    }

    fn move_left(ta: &mut TextArea<'static>) {
        ta.move_cursor(ratatui_textarea::CursorMove::Back);
    }

    #[test]
    fn textarea_buffer_backspace_and_delete_at_cursor() {
        let mut b = textarea_for("abc".to_string(), end_cursor("abc".to_string()));
        b.delete_char();
        assert_eq!(b.lines().join("\n"), "ab");
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        b.delete_next_char();
        assert_eq!(b.lines().join("\n"), "b");
        assert_eq!(textarea_lines_of(&b), (0, 0), "delete keeps the cursor put");
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        b.delete_char();
        assert_eq!(b.lines().join("\n"), "b", "backspace at start is a no-op");
        b.delete_next_char();
        b.delete_next_char();
        assert!(b.is_empty(), "delete to the end");
    }

    #[test]
    fn textarea_buffer_newline_splits_lines_and_joins_on_commit() {
        let mut b = textarea_for("first".to_string(), end_cursor("first".to_string()));
        b.insert_newline();
        assert_eq!(b.lines(), vec!["first".to_string(), String::new()]);
        assert_eq!(textarea_lines_of(&b), (1, 0), "cursor on the fresh line");
        b.insert_str("second".to_string());
        assert_eq!(b.lines().join("\n"), "first\nsecond");
        // Backspace at the start of a line merges it into the previous one.
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        b.delete_char();
        assert_eq!(b.lines().join("\n"), "firstsecond", "joined lines");
        assert_eq!(textarea_lines_of(&b), (0, 5), "cursor at the join point");
        // Delete at the join point removes the merged line's first char.
        b.delete_next_char();
        assert_eq!(b.lines().join("\n"), "firstecond");
    }

    #[test]
    fn textarea_buffer_home_end_and_cross_line_navigation() {
        let mut b = textarea_for(
            "line one\nline two".to_string(),
            end_cursor("line one\nline two".to_string()),
        );
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        assert_eq!(
            textarea_lines_of(&b),
            (1, 0),
            "Head goes to the current line's start"
        );
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        assert_eq!(textarea_lines_of(&b), (1, 0), "Head is line-scoped");
        b.move_cursor(ratatui_textarea::CursorMove::Back);
        assert_eq!(
            textarea_lines_of(&b),
            (0, 8),
            "Back from line start joins line 0"
        );
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        assert_eq!(textarea_lines_of(&b), (0, 0));
        b.move_cursor(ratatui_textarea::CursorMove::Forward);
        assert_eq!(textarea_lines_of(&b), (0, 1));
        b.move_cursor(ratatui_textarea::CursorMove::End);
        assert_eq!(textarea_lines_of(&b), (0, 8));
        // Forward at the end of line 0 moves to the start of line 1.
        b.move_cursor(ratatui_textarea::CursorMove::Forward);
        assert_eq!(textarea_lines_of(&b), (1, 0));
        // Back at the start of line 1 moves to the end of line 0.
        b.move_cursor(ratatui_textarea::CursorMove::Back);
        assert_eq!(textarea_lines_of(&b), (0, 8));
    }

    #[test]
    fn textarea_buffer_multibyte_chars_stay_on_boundaries() {
        let mut b = textarea_for("caf\u{e9}".to_string(), end_cursor("caf\u{e9}".to_string()));
        assert_eq!(
            textarea_lines_of(&b),
            (0, 4),
            "é is one char (cursor col = char count)"
        );
        b.move_cursor(ratatui_textarea::CursorMove::Back);
        b.insert_char('\u{e9}');
        assert_eq!(b.lines().join("\n"), "caf\u{e9}\u{e9}");
        b.delete_char();
        assert_eq!(b.lines().join("\n"), "caf\u{e9}");
        b.move_cursor(ratatui_textarea::CursorMove::Head);
        b.delete_next_char();
        assert_eq!(b.lines().join("\n"), "af\u{e9}");
    }

    fn edit_pane_multi_line_description_reserves_slot_and_renders() {
        let (mut app, epic_id) = edit_app();
        let issue = app
            .projects
            .iter_mut()
            .flat_map(|p| p.issues.iter_mut())
            .find(|i| i.id == epic_id)
            .unwrap();
        issue.description = Some("first line\nsecond line".to_string());
        // Step to Description and arm its edit.
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        let edit = app.edit.as_ref().unwrap();
        assert!(edit.editing);
        assert_eq!(edit.textarea.lines().len(), 2, "pre-filled with both lines");
        // The cursor pre-fills at the end of the text; Home then Left puts
        // it at the end of line 0, so Enter splits the first line.
        edit_key(&mut app, key_event(KeyCode::Home)).unwrap();
        edit_key(&mut app, key_event(KeyCode::Left)).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().cursor(), (0, 10));
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert_eq!(
            app.edit.as_ref().unwrap().text(),
            "first line\n\nsecond line"
        );
        let rows = draw(&mut app);
        let joined = rows.join("\n");
        // The Description label row stays above the widget slot; the slot
        // is reserved so later buffer lines render there, never over other
        // fields (d31).
        assert!(
            joined.contains("Description:"),
            "label row renders: {joined}"
        );
        assert!(
            rows.iter().any(|r| r.contains("first line")),
            "first buffer line visible: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("second line")),
            "later buffer lines are their own rows: {rows:?}"
        );
        // Non-active rows still show live values and keep the old row
        // format (`> ` marker only on the active field row).
        assert!(joined.contains("Title: The epic"), "{joined}");
        assert!(
            joined.contains("Enter: new line, Tab: apply, Esc: cancel"),
            "multi-line hint: {joined}"
        );
        assert!(
            !joined.contains("> Title: The epic"),
            "the active field is the widget slot, not a highlighted row: {joined}"
        );
    }

    #[test]
    fn edit_pane_description_slot_grows_with_content() {
        // Regression (bug 11265333): the editor slot was capped by the pane's
        // OTHER rows (~3-4 lines) instead of the content/area, so long
        // descriptions got a tiny window even in a large terminal. With
        // the fix the slot grows: 6 description lines must all render.
        let (mut app, epic_id) = edit_app();
        let issue = app
            .projects
            .iter_mut()
            .flat_map(|p| p.issues.iter_mut())
            .find(|i| i.id == epic_id)
            .unwrap();
        issue.description = Some("line 1\nline 2\nline 3\nline 4\nline 5\nline 6".to_string());
        // Step to Description and arm its edit.
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert!(app.edit.as_ref().unwrap().editing);
        let rows = draw(&mut app);
        for n in 1..6 {
            assert!(
                rows.iter().any(|r| r.contains(&format!("line {n}"))),
                "description line {n} visible in the grown slot: {rows:?}"
            );
        }
    }

    fn edit_cycle_applies_on_first_enter_and_stays() {
        let (tempdir, mut app, id) = edit_app_ops();
        let root = tempdir.path().to_path_buf();
        // Status: step to it, cycle todo -> in_progress, one Enter applies.
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::Status);
        edit_key(&mut app, key_event(KeyCode::Right)).unwrap();
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        let edit = app.edit.as_ref().unwrap();
        assert_eq!(app.mode, Mode::Edit, "pane stays open");
        assert!(!edit.editing, "no re-arm after the apply");
        assert_eq!(edit.field, EditField::Status, "highlight stays");
        assert_eq!(
            edit.pending_status,
            Status::InProgress,
            "pending re-synced to the applied value"
        );
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .contains("Status: in_progress"),
            "message: {:?}",
            app.message
        );
        let content = std::fs::read_to_string(issue_file(&root, id)).unwrap();
        assert!(content.contains("\"status\": \"in_progress\""), "{content}");
        // Enter again without cycling: no-change, still stays.
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert_eq!(
            app.message.as_deref().unwrap(),
            "No changes: nothing to update"
        );
        assert_eq!(app.mode, Mode::Edit);
        assert!(!app.edit.as_ref().unwrap().editing);
        // Type: back to Type, cycle bug -> ux_problem -> usecase (two
        // Rights in cycle order), one Enter applies.
        edit_key(&mut app, key_event(KeyCode::Char('k'))).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::Type);
        edit_key(&mut app, key_event(KeyCode::Right)).unwrap();
        edit_key(&mut app, key_event(KeyCode::Right)).unwrap();
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert_eq!(app.mode, Mode::Edit, "still stays after type apply");
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .contains("Converted to usecase"),
            "message: {:?}",
            app.message
        );
        let content = std::fs::read_to_string(issue_file(&root, id)).unwrap();
        assert!(content.contains("\"issue_type\": \"usecase\""), "{content}");
        // Esc closes the pane (navigation).
        edit_key(&mut app, key_event(KeyCode::Esc)).unwrap();
        assert_eq!(app.mode, Mode::Browser);
        assert!(app.edit.is_none());
    }

    #[test]
    fn edit_description_enter_inserts_newline_and_tab_applies() {
        let (tempdir, mut app, id) = edit_app_ops();
        let root = tempdir.path().to_path_buf();
        // Step to Description, arm, type a two-line value.
        edit_key(&mut app, key_event(KeyCode::Char('j'))).unwrap();
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        for c in "Fixes login".chars() {
            edit_key(&mut app, key_event(KeyCode::Char(c))).unwrap();
        }
        edit_key(&mut app, key_event(KeyCode::Enter)).unwrap();
        assert_eq!(app.edit.as_ref().unwrap().text(), "Fixes login\n");
        for c in "for real".chars() {
            edit_key(&mut app, key_event(KeyCode::Char(c))).unwrap();
        }
        // Tab applies the multi-line description (Enter is the newline key).
        edit_key(&mut app, key_event(KeyCode::Tab)).unwrap();
        let edit = app.edit.as_ref().unwrap();
        assert!(!edit.editing, "Tab applies and leaves the field edit");
        assert_eq!(
            edit.text(),
            "Fixes login\nfor real",
            "textarea retains the committed text until the next arm"
        );
        assert_eq!(app.mode, Mode::Edit, "pane stays open");
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .contains("Updated description"),
            "message: {:?}",
            app.message
        );
        let content = std::fs::read_to_string(issue_file(&root, id)).unwrap();
        assert!(
            content.contains("\"description\": \"Fixes login\\nfor real\""),
            "{content}"
        );
    }
}
