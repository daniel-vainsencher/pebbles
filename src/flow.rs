use crate::models::{Issue, IssueType, RelationType, Status};
use anyhow::{bail, Result};
use rust_sugiyama::configure::Config;
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

/// Represents the layout result for visualization
pub struct FlowLayout {
    /// Coordinates for each issue: (issue_index, (x, y))
    pub coordinates: Vec<(usize, (f64, f64))>,
    /// Width of the layout
    pub width: f64,
    /// Height of the layout
    pub height: f64,
}

/// Build dependency graph from issues
/// Returns edges as (source, target) where source must complete before target
fn build_dependency_graph(issues: &[Issue]) -> Vec<(u32, u32)> {
    let mut edges = Vec::new();
    let id_to_index: HashMap<_, _> = issues
        .iter()
        .enumerate()
        .map(|(i, issue)| (issue.id, i as u32))
        .collect();

    for (idx, issue) in issues.iter().enumerate() {
        for rel in &issue.relationships {
            // RequiredFor(A, B) means A is required for B → edge A -> B
            // ScheduledBefore(A, B) means A before B → edge A -> B
            // PartOf doesn't create dependency edges (just hierarchy)
            match rel.relation_type {
                RelationType::RequiredFor | RelationType::ScheduledBefore => {
                    if let Some(&target_idx) = id_to_index.get(&rel.target_id) {
                        edges.push((idx as u32, target_idx));
                    }
                }
                RelationType::PartOf => {
                    // Skip - epics don't create flow dependencies
                }
            }
        }
    }

    edges
}

/// Returns `Some(vertex)` if the directed graph contains a cycle, detected
/// with Kahn's algorithm (a vertex that can never be peeled off means a cycle).
fn find_cycle_vertex(edges: &[(u32, u32)], vertex_count: usize) -> Option<u32> {
    let mut indegree = vec![0usize; vertex_count];
    let mut adjacency: Vec<Vec<u32>> = vec![Vec::new(); vertex_count];
    for &(from, to) in edges {
        adjacency[from as usize].push(to);
        indegree[to as usize] += 1;
    }

    let mut queue: Vec<u32> = (0..vertex_count as u32)
        .filter(|v| indegree[*v as usize] == 0)
        .collect();
    let mut processed = 0usize;

    while let Some(v) = queue.pop() {
        processed += 1;
        for &next in &adjacency[v as usize] {
            indegree[next as usize] -= 1;
            if indegree[next as usize] == 0 {
                queue.push(next);
            }
        }
    }

    (processed < vertex_count).then(|| {
        (0..vertex_count as u32)
            .find(|v| indegree[*v as usize] > 0)
            .unwrap()
    })
}

/// Escape text for safe inclusion in XML/SVG content.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Compute layout using rust-sugiyama
pub fn compute_flow_layout(issues: &[Issue]) -> Result<FlowLayout> {
    if issues.is_empty() {
        return Ok(FlowLayout {
            coordinates: Vec::new(),
            width: 0.0,
            height: 0.0,
        });
    }

    let edges = build_dependency_graph(issues);

    if find_cycle_vertex(&edges, issues.len()).is_some() {
        bail!("Dependency graph contains a cycle; use 'pebbles link'/'pebbles unlink' to fix the required_for/scheduled_before relationships before visualizing");
    }

    // Create vertices with estimated widths based on title length
    let vertices: Vec<(u32, (f64, f64))> = issues
        .iter()
        .enumerate()
        .map(|(i, issue)| {
            // Rough estimate: 8 pixels per character, min 80px
            let title_width = (issue.title.len() as f64 * 8.0).max(80.0);
            (i as u32, (title_width, 30.0))
        })
        .collect();

    let config = Config {
        vertex_spacing: 20.0,
        minimum_length: 1,
        dummy_vertices: false, // We don't need bend points for this spike
        ..Default::default()
    };

    // rust-sugiyama returns Vec<(layout, width, height)> for each connected component
    let layouts = rust_sugiyama::from_vertices_and_edges(&vertices, &edges, &config);

    // Merge all components by overlapping them in X (layers)
    // but separating them in Y (vertical position within layer)
    let mut all_coordinates = Vec::new();
    let mut current_y_offset: f64 = 0.0;
    let mut max_width: f64 = 0.0;
    let mut total_height: f64 = 0.0;

    for (component_layout, comp_width, comp_height) in layouts {
        // SWAP X and Y: rust-sugiyama uses Y for layers (top-to-bottom)
        // We want X for layers (left-to-right)
        // All components start at the same X (layer 0), but offset in Y
        for (idx, (x, y)) in component_layout {
            // After swap: y becomes X (layer), x becomes Y (position in layer)
            all_coordinates.push((idx, (y, x + current_y_offset)));
        }

        current_y_offset += comp_width + 20.0; // Add vertical gap between components
        total_height = current_y_offset - 20.0; // Remove last gap
        max_width = max_width.max(comp_height); // height becomes width (max layers)
    }

    Ok(FlowLayout {
        coordinates: all_coordinates,
        width: max_width,
        height: total_height,
    })
}

/// Minimal unambiguous ID prefix for display, falling back to the 8-char ID.
fn issue_id_prefix(id: Uuid, id_prefixes: &HashMap<Uuid, String>) -> String {
    id_prefixes
        .get(&id)
        .cloned()
        .unwrap_or_else(|| id.to_string()[..8].to_string())
}

/// Maximum title length in a flow cell before it is truncated with "...".
const FLOW_TITLE_MAX: usize = 30;

/// Width of the gutter between flow columns.
const FLOW_COLUMN_GUTTER: usize = 2;

/// Truncate a title to at most `FLOW_TITLE_MAX` characters, appending
/// "..." when it is cut, so flow columns hold a consistent width.
fn truncate_flow_title(title: &str) -> String {
    if title.chars().count() <= FLOW_TITLE_MAX {
        title.to_string()
    } else {
        format!(
            "{}...",
            title.chars().take(FLOW_TITLE_MAX - 3).collect::<String>()
        )
    }
}

/// Indicator glyph for one flow issue: "\u{26a0}" (warning) when a
/// non-Done issue blocks it, "\u{2192}" (arrow) when it blocks other
/// issues, and "\u{2713}" (check) when it is ready. A blocked issue
/// always shows the warning, so a blocked issue that also blocks others
/// only shows "\u{26a0}" (the same precedence the list view uses in
/// display.rs `blocker_marker`). A `Done` issue is never "blocked" or
/// "ready" (it is finished); it keeps the arrow only when it still gates
/// other work and otherwise shows the two-space placeholder.
fn flow_indicator(blocked: bool, blocks_others: bool, status: Status) -> &'static str {
    if blocked && status != Status::Done {
        "\u{26a0}"
    } else if blocks_others {
        "\u{2192}"
    } else if status != Status::Done {
        "\u{2713}"
    } else {
        "  "
    }
}

/// Right-pad `text` with spaces to exactly `width` columns. The width
/// includes the trailing gutter, so consecutive columns stay aligned.
fn pad_right(text: String, width: usize) -> String {
    if text.len() >= width {
        text
    } else {
        format!("{}{}", text, " ".repeat(width - text.len()))
    }
}

/// Render layout as text in columns
///
/// Each dependency layer becomes a column across the width under one
/// "Layer N:" header row; disconnected components stack as horizontal
/// bands separated by a blank line. Every issue carries a one-glyph
/// indicator ("\u{2713}" ready, "\u{26a0}" blocked, "\u{2192}" gating
/// other work), and long titles are truncated at a consistent width.
///
/// `_goal` is the goal issue whose critical chain should be highlighted.
/// Threading it here is the seam for the --goal loan (e4937f0b); the
/// highlighting itself is implemented in that loan, which renames this
/// parameter to `goal` as it consumes it.
pub fn render_text(
    issues: &[Issue],
    layout: &FlowLayout,
    id_prefixes: &HashMap<Uuid, String>,
    _goal: Option<Uuid>,
) -> String {
    if layout.coordinates.is_empty() {
        return "No issues to visualize.".to_string();
    }

    // Dependency state per issue, over the same edges that drive the
    // layering (`build_dependency_graph`: required_for + scheduled_before).
    // "Blocked" mirrors `get_unblocked_issues` (operations.rs): an incoming
    // dependency edge from a non-Done issue.
    let edges = build_dependency_graph(issues);
    let mut blocked = vec![false; issues.len()];
    let mut blocks_others = vec![false; issues.len()];
    let mut has_edges = vec![false; issues.len()];
    for &(source, target) in &edges {
        has_edges[source as usize] = true;
        has_edges[target as usize] = true;
        blocks_others[source as usize] = true;
        if issues[source as usize].status != Status::Done {
            blocked[target as usize] = true;
        }
    }

    // X carries the layer coordinate after the Y<->X swap in
    // compute_flow_layout; 50px buckets match rust-sugiyama's layer
    // spacing (kept from the original presentation).
    let mut layers: Vec<i32> = vec![0; issues.len()];
    let mut ys: Vec<f64> = vec![0.0; issues.len()];
    for &(idx, (x, y)) in &layout.coordinates {
        ys[idx] = y;
        if has_edges[idx] {
            layers[idx] = (x / 50.0).round() as i32;
        }
    }

    // Issues without dependency edges have no layer and no indicator;
    // they render as a trailing list.
    let mut standalone: Vec<usize> = Vec::new();
    for idx in 0..issues.len() {
        if !has_edges[idx] {
            standalone.push(idx);
        }
    }
    standalone.sort_by(|a, b| ys[*a].partial_cmp(&ys[*b]).unwrap());

    // Columns are the ascending union of layer numbers across all
    // dependency components, so every component band aligns under the
    // same header row.
    let mut seen_layers: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut column_layers: Vec<i32> = Vec::new();
    for idx in 0..issues.len() {
        if has_edges[idx] && !seen_layers.contains(&layers[idx]) {
            seen_layers.insert(layers[idx]);
            column_layers.push(layers[idx]);
        }
    }
    column_layers.sort();

    let mut output = String::new();
    output.push_str("═══ Project Flow ═══\n\n");

    let has_columns = !column_layers.is_empty();

    // One column cell per graph issue, with its indicator glyph.
    let mut cell: Vec<String> = vec![String::new(); issues.len()];
    for idx in 0..issues.len() {
        if has_edges[idx] {
            cell[idx] = format!(
                "{} {} {}",
                flow_indicator(blocked[idx], blocks_others[idx], issues[idx].status),
                issue_id_prefix(issues[idx].id, id_prefixes),
                truncate_flow_title(&issues[idx].title)
            );
        }
    }

    // Render each component as a band of rows, components stacking
    // top-to-bottom in Y order (the same order the SVG lays them out).
    let mut bands: Vec<Vec<Vec<String>>> = Vec::new();
    let mut comps: Vec<(f64, Vec<usize>)> = Vec::new();
    for comp in dependency_components(issues) {
        // dependency_components returns one component per isolated issue
        // too; those have no dependency edges and render in the trailing
        // standalone list instead of a band.
        if !has_edges[comp[0]] {
            continue;
        }
        // The component's topmost Y: its first rendered row.
        let mut min_y = ys[comp[0]];
        for idx in &comp {
            min_y = min_y.min(ys[*idx]);
        }
        comps.push((min_y, comp));
    }
    comps.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    for (_, comp) in comps {
        // Group the component's issues by layer, sorting a layer's
        // issues by Y so rows follow the layout.
        let mut by_layer: HashMap<i32, Vec<(usize, f64)>> = HashMap::new();
        for idx in comp {
            by_layer
                .entry(layers[idx])
                .or_default()
                .push((idx, ys[idx]));
        }
        let groups: Vec<(i32, Vec<(usize, f64)>)> = by_layer
            .iter()
            .map(|(layer, layer_issues)| (*layer, layer_issues.clone()))
            .collect::<Vec<_>>();
        let mut sorted_by_layer: HashMap<i32, Vec<usize>> = HashMap::new();
        let mut max_rows: usize = 0;
        for (layer, layer_issues) in groups {
            let mut sorted_issues = layer_issues.clone();
            sorted_issues.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            max_rows = max_rows.max(sorted_issues.len());
            let indices: Vec<usize> = sorted_issues
                .iter()
                .map(|(idx, _)| *idx)
                .collect::<Vec<_>>();
            sorted_by_layer.insert(layer.clone(), indices);
        }

        let mut rows: Vec<Vec<String>> = Vec::new();
        for row in 0..max_rows {
            let mut cells: Vec<String> = Vec::new();
            for l in &column_layers {
                if let Some(indices) = sorted_by_layer.get(l) {
                    if row < indices.len() {
                        cells.push(cell[indices[row]].clone());
                    } else {
                        cells.push(String::new());
                    }
                } else {
                    cells.push(String::new());
                }
            }
            rows.push(cells);
        }
        bands.push(rows);
    }

    // Column widths span every band plus the layer headers, so all
    // components share the same alignment.
    let mut column_widths: Vec<usize> = Vec::new();
    for (ci, l) in column_layers.iter().enumerate() {
        let mut w = format!("Layer {}:", *l).len();
        for rows in &bands {
            for cells in rows {
                w = w.max(cells[ci].len());
            }
        }
        column_widths.push(w + FLOW_COLUMN_GUTTER);
    }

    // One header row, then the component bands.
    if has_columns {
        let mut header = String::new();
        for (ci, l) in column_layers.iter().enumerate() {
            if ci + 1 < column_layers.len() {
                header.push_str(&pad_right(format!("Layer {}:", *l), column_widths[ci]));
            } else {
                header.push_str(&format!("Layer {}:", *l));
            }
        }
        output.push_str(&header);
        output.push('\n');
        for (bi, rows) in bands.iter().enumerate() {
            if bi > 0 {
                // Empty line between disconnected components.
                output.push('\n');
            }
            for cells in rows {
                for (ci, _) in column_layers.iter().enumerate() {
                    if ci + 1 < column_layers.len() {
                        output.push_str(&pad_right(cells[ci].clone(), column_widths[ci]));
                    } else {
                        output.push_str(&cells[ci]);
                    }
                }
                output.push('\n');
            }
        }
    }

    // Standalone issues (no dependency edges) render as a trailing list
    // with a two-space placeholder where the indicator glyph would sit.
    if !standalone.is_empty() {
        if has_columns {
            output.push('\n');
        }
        output.push_str(&format!(
            "Standalone issues ({} total):\n",
            standalone.len()
        ));
        for idx in standalone {
            let issue = &issues[idx];
            output.push_str(&format!(
                "  {} {}\n",
                issue_id_prefix(issue.id, id_prefixes),
                truncate_flow_title(&issue.title)
            ));
        }
    }

    output
}

/// Render layout as SVG
///
/// `_goal` is the goal issue whose critical chain should be highlighted.
/// Threading it here is the seam for the --goal loan (e4937f0b); the
/// highlighting itself is implemented in that loan, which renames this
/// parameter to `goal` as it consumes it.
pub fn render_svg(
    issues: &[Issue],
    layout: &FlowLayout,
    id_prefixes: &HashMap<Uuid, String>,
    _goal: Option<Uuid>,
) -> String {
    let padding = 20.0;
    let svg_width = layout.width + 2.0 * padding;
    let svg_height = layout.height + 2.0 * padding;

    let mut svg = format!(
        r#"<svg width="{}" height="{}" xmlns="http://www.w3.org/2000/svg">
<style>
  .issue-box {{ fill: white; stroke: #333; stroke-width: 2; }}
  .issue-text {{ font-family: monospace; font-size: 12px; }}
  .epic {{ fill: #e8f4f8; }}
  .done {{ fill: #e8f8e8; }}
  .in_progress {{ fill: #fff8e8; }}
  .blocked {{ fill: #f8e8e8; }}
</style>
"#,
        svg_width, svg_height
    );

    // Draw edges first (so they appear behind boxes)
    let edges = build_dependency_graph(issues);
    let coord_map: HashMap<usize, (f64, f64)> = layout
        .coordinates
        .iter()
        .map(|&(idx, coords)| (idx, coords))
        .collect();

    // Arrow marker definition
    svg.push_str(
        r##"  <defs>
    <marker id="arrowhead" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto">
      <polygon points="0 0, 10 3, 0 6" fill="#999" />
    </marker>
  </defs>
"##,
    );

    for (source, target) in edges {
        if let (Some(&(sx, sy)), Some(&(tx, ty))) = (
            coord_map.get(&(source as usize)),
            coord_map.get(&(target as usize)),
        ) {
            svg.push_str(&format!(
                r##"  <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#999" stroke-width="1" marker-end="url(#arrowhead)" />
"##,
                sx + padding,
                sy + padding + 15.0,
                tx + padding,
                ty + padding + 15.0
            ));
        }
    }

    // Draw boxes
    for &(idx, (x, y)) in &layout.coordinates {
        let issue = &issues[idx];
        let short_id = issue_id_prefix(issue.id, id_prefixes);
        let box_width = (issue.title.len() as f64 * 8.0).max(80.0);
        let box_height = 30.0;

        let status_class = match issue.status.to_string().as_str() {
            "done" => "done",
            "in_progress" => "in_progress",
            "blocked" => "blocked",
            _ => "",
        };

        let type_class = if issue.issue_type.to_string() == "epic" {
            "epic"
        } else {
            ""
        };

        svg.push_str(&format!(
            r#"  <rect x="{}" y="{}" width="{}" height="{}" class="issue-box {} {}" />
  <text x="{}" y="{}" class="issue-text">{}: {}</text>
"#,
            x + padding,
            y + padding,
            box_width,
            box_height,
            status_class,
            type_class,
            x + padding + 5.0,
            y + padding + 20.0,
            escape_xml(&short_id),
            escape_xml(&issue.title.chars().take(30).collect::<String>())
        ));
    }

    svg.push_str("</svg>\n");
    svg
}

/// Escape text for safe inclusion in a double-quoted DOT string: backslash,
/// double quote, and newline are the characters with special meaning inside
/// DOT quotes. Mirrors `escape_xml` for the SVG backend.
fn escape_dot(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// DOT fill color for an issue's status, matching pebbles' status colors
/// (the same palette the CLI paints status names with): green done, yellow
/// todo, blue in_progress, red blocked, white proposed.
fn status_dot_color(status: Status) -> &'static str {
    match status {
        Status::Done => "green",
        Status::Todo => "yellow",
        Status::InProgress => "blue",
        Status::Blocked => "red",
        Status::Proposed => "white",
    }
}

/// Dependency edges with the relationship type that created them, for DOT
/// edge labels. `PartOf` is excluded exactly as in `build_dependency_graph`.
fn labeled_dependency_edges(issues: &[Issue]) -> Vec<(u32, u32, RelationType)> {
    let id_to_index: HashMap<Uuid, u32> = issues
        .iter()
        .enumerate()
        .map(|(i, issue)| (issue.id, i as u32))
        .collect();

    let mut edges = Vec::new();
    for (idx, issue) in issues.iter().enumerate() {
        for rel in &issue.relationships {
            match rel.relation_type {
                RelationType::RequiredFor | RelationType::ScheduledBefore => {
                    if let Some(&target_idx) = id_to_index.get(&rel.target_id) {
                        edges.push((idx as u32, target_idx, rel.relation_type.clone()));
                    }
                }
                RelationType::PartOf => {
                    // Skip - epics don't create flow dependencies
                }
            }
        }
    }

    edges
}

/// Partition issue indices into the connected components of the dependency
/// graph, ordered by lowest issue index. `PartOf` edges stay out of the
/// graph (matching `build_dependency_graph`), so every multi-member
/// component is one disconnected flow.
fn dependency_components(issues: &[Issue]) -> Vec<Vec<usize>> {
    let edges = labeled_dependency_edges(issues);
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); issues.len()];
    for &(from, to, _) in &edges {
        adjacency[from as usize].push(to as usize);
        adjacency[to as usize].push(from as usize);
    }

    let mut visited: Vec<bool> = vec![false; issues.len()];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..issues.len() {
        if visited[start] {
            continue;
        }
        let mut component: Vec<usize> = Vec::new();
        let mut queue: Vec<usize> = vec![start];
        visited[start] = true;
        while let Some(node) = queue.pop() {
            component.push(node);
            for &next in &adjacency[node] {
                if !visited[next] {
                    visited[next] = true;
                    queue.push(next);
                }
            }
        }
        component.sort();
        components.push(component);
    }

    components
}

/// Render layout as Graphviz DOT
///
/// Emits a `digraph pebbles_flow` with `rankdir=LR` and `node [shape=box]`
/// defaults, so consumers can render it with `dot -Tsvg flow.dot > flow.svg`.
/// Nodes are labeled with the truncated issue title and colored by status
/// (green done, yellow todo, blue in_progress, red blocked, white
/// proposed); edges are labeled with the relationship type
/// (`required_for`/`scheduled_before`); disconnected components become
/// `subgraph cluster_*` blocks.
///
/// `_goal` is the goal issue whose critical chain should be highlighted.
/// Threading it here is the seam for the --goal loan (e4937f0b); the
/// highlighting itself is implemented in that loan, which renames this
/// parameter to `goal` as it consumes it.
///
/// ```rust
/// # use pebbles::flow::{render_dot, FlowLayout};
/// # use pebbles::models::{Issue, IssueType};
/// # use std::collections::HashMap;
/// let layout = FlowLayout {
///     coordinates: vec![(0, (0.0, 0.0))],
///     width: 100.0,
///     height: 30.0,
/// };
/// let dot = render_dot(
///     &vec![Issue::new("A".to_string(), None, IssueType::Bug)],
///     &layout,
///     &HashMap::new(),
///     None,
/// );
/// assert!(dot.contains("digraph pebbles_flow {"));
/// assert!(dot.contains("[label=\"A\" color=\"white\" style=filled]"));
/// ```
pub fn render_dot(
    issues: &[Issue],
    layout: &FlowLayout,
    id_prefixes: &HashMap<Uuid, String>,
    _goal: Option<Uuid>,
) -> String {
    let mut output = String::new();
    output.push_str("digraph pebbles_flow {\n");
    output.push_str("  rankdir=LR;\n");
    output.push_str("  node [shape=box];\n");

    if layout.coordinates.is_empty() {
        output.push_str("}\n");
        return output;
    }

    output.push('\n');

    let edges = labeled_dependency_edges(issues);
    let components = dependency_components(issues);

    // Components with edges become cluster subgraphs; standalone issues
    // stay at the top level, mirroring the text renderer's "Issues with
    // dependencies" then "Standalone issues" section order.
    for comp in 0..components.len() {
        let component = &components[comp];
        if component.len() == 1 {
            continue;
        }
        output.push_str(&format!("  subgraph cluster_{} {{\n", comp));
        for member in 0..component.len() {
            let idx = component[member];
            let issue = &issues[idx];
            output.push_str(&format!(
                "    \"{}\" [label=\"{}\" color=\"{}\" style=filled];\n",
                escape_dot(&issue_id_prefix(issue.id, id_prefixes)),
                escape_dot(&issue.title.chars().take(30).collect::<String>()),
                status_dot_color(issue.status),
            ));
        }
        // Only edges whose source lives in this component (all dependency
        // edges stay inside their component by construction).
        output.push('\n');
        for &(source, target, ref rel_type) in &edges {
            if !component.contains(&(source as usize)) {
                continue;
            }
            output.push_str(&format!(
                "    \"{}\" -> \"{}\" [label=\"{}\"];\n",
                escape_dot(&issue_id_prefix(issues[source as usize].id, id_prefixes)),
                escape_dot(&issue_id_prefix(issues[target as usize].id, id_prefixes)),
                rel_type.clone(),
            ));
        }
        output.push_str("  }\n\n");
    }

    for comp in 0..components.len() {
        let component = &components[comp];
        if component.len() != 1 {
            continue;
        }
        let idx = component[0];
        let issue = &issues[idx];
        output.push_str(&format!(
            "  \"{}\" [label=\"{}\" color=\"{}\" style=filled];\n",
            escape_dot(&issue_id_prefix(issue.id, id_prefixes)),
            escape_dot(&issue.title.chars().take(30).collect::<String>()),
            status_dot_color(issue.status),
        ));
    }

    output.push_str("}\n");
    output
}

/// One node of the flow JSON export (r[flow-json-schema], documented in
/// docs/flow-json-schema.md): the issue's full id, its 8-char `short_id`
/// prefix, title, type, status, and layout position. `in_critical_path` is
/// omitted when no goal thread is given; when `render_json` receives a goal
/// the key is present as `false` for every node (the critical-chain
/// computation is the separate loan e4937f0b).
#[derive(Serialize)]
struct FlowJsonNode {
    id: String,
    short_id: String,
    title: String,
    #[serde(rename = "type")]
    issue_type: IssueType,
    status: Status,
    x: f64,
    y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_critical_path: Option<bool>,
}

/// One dependency edge of the flow JSON export: the source and target issue
/// ids plus the relationship type that created the edge. `PartOf` edges are
/// excluded, exactly as in `build_dependency_graph`.
#[derive(Serialize)]
struct FlowJsonEdge {
    source: String,
    target: String,
    #[serde(rename = "type")]
    relation_type: RelationType,
}

/// Layout summary of the flow JSON export: the layout's overall width and
/// height plus the number of distinct dependency layers.
#[derive(Serialize)]
struct FlowJsonLayout {
    width: f64,
    height: f64,
    layers: u64,
}

/// The flow JSON document: nodes, edges, and the layout summary
/// (r[flow-json-schema]).
#[derive(Serialize)]
struct FlowJson {
    nodes: Vec<FlowJsonNode>,
    edges: Vec<FlowJsonEdge>,
    layout: FlowJsonLayout,
}

/// Count distinct dependency layers in `layout`: the number of distinct
/// `round(x / 50)` buckets over issues that have at least one dependency
/// edge, matching the "Layer N:" columns the text renderer emits. Zero when
/// no issue has a dependency edge.
fn flow_layer_count(issues: &[Issue], layout: &FlowLayout) -> u64 {
    let edges = build_dependency_graph(issues);
    let mut has_edges = vec![false; issues.len()];
    for &(source, target) in &edges {
        has_edges[source as usize] = true;
        has_edges[target as usize] = true;
    }
    let mut seen_layers: std::collections::HashSet<i32> = std::collections::HashSet::new();
    for &(idx, (x, _)) in &layout.coordinates {
        if has_edges[idx] {
            seen_layers.insert((x / 50.0).round() as i32);
        }
    }
    seen_layers.len() as u64
}

/// Render `layout` as deterministic JSON for programmatic consumers. The
/// document (r[flow-json-schema], documented in docs/flow-json-schema.md)
/// carries `nodes` (one object per issue, in issue order; `x` is the
/// node's layer coordinate in layout units and `y` its position within the
/// layer), `edges` (one object per `required_for`/`scheduled_before`
/// dependency, `PartOf` excluded, in the order `labeled_dependency_edges`
/// yields), and `layout` (width, height, and dependency layer count).
/// `goal` controls only whether each node carries `in_critical_path`:
/// `Some` emits `false` for every issue, `None` omits the key (the
/// critical-chain computation is loan e4937f0b). Output is stable: struct
/// field order, 2-space indent, no timestamps. Pure: no storage or I/O.
///
/// ```rust
/// # use pebbles::flow::{render_json, FlowLayout};
/// # use pebbles::models::{Issue, IssueType};
/// # use std::collections::HashMap;
/// let layout = FlowLayout {
///     coordinates: vec![(0, (0.0, 0.0))],
///     width: 100.0,
///     height: 30.0,
/// };
/// let json = render_json(
///     &vec![Issue::new("A".to_string(), None, IssueType::Bug)],
///     &layout,
///     &HashMap::new(),
///     None,
/// );
/// assert!(json.contains("\"nodes\""));
/// assert!(json.contains("\"layout\""));
/// ```
pub fn render_json(
    issues: &[Issue],
    layout: &FlowLayout,
    _id_prefixes: &HashMap<Uuid, String>,
    goal: Option<Uuid>,
) -> String {
    // Index -> (x, y), as the text and SVG renderers use it.
    let coord_map: HashMap<usize, (f64, f64)> = layout
        .coordinates
        .iter()
        .map(|&(idx, coords)| (idx, coords))
        .collect();

    let in_critical_path = if goal.is_some() { Some(false) } else { None };
    let mut nodes: Vec<FlowJsonNode> = Vec::new();
    for (idx, issue) in issues.iter().enumerate() {
        let (x, y) = match coord_map.get(&idx) {
            Some(&(x, y)) => (x, y),
            None => (0.0, 0.0),
        };
        nodes.push(FlowJsonNode {
            id: issue.id.to_string(),
            short_id: issue.short_id(),
            title: issue.title.clone(),
            issue_type: issue.issue_type.clone(),
            status: issue.status.clone(),
            x,
            y,
            in_critical_path,
        });
    }

    let mut edges: Vec<FlowJsonEdge> = Vec::new();
    for (source, target, relation_type) in labeled_dependency_edges(issues) {
        edges.push(FlowJsonEdge {
            source: issues[source as usize].id.to_string(),
            target: issues[target as usize].id.to_string(),
            relation_type: relation_type.clone(),
        });
    }

    let document = FlowJson {
        nodes,
        edges,
        layout: FlowJsonLayout {
            width: layout.width,
            height: layout.height,
            layers: flow_layer_count(issues, layout),
        },
    };
    let mut content = serde_json::to_string_pretty(&document).unwrap();
    content.push('\n');
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Issue, IssueType, Status};
    use serde_json::Value;

    fn issue(title: &str) -> Issue {
        Issue::new(title.to_string(), None, IssueType::Bug)
    }

    fn issue_with_status(title: &str, status: Status) -> Issue {
        let mut issue = issue(title);
        issue.status = status;
        issue
    }

    /// `render_text` lays each dependency layer out as a column, pads
    /// every cell to one shared width per column, and separates
    /// disconnected components with an empty line.
    #[test]
    fn test_render_text_columns_and_components() {
        // Component 1: A -> B; component 2: C -> D (drawn below it).
        let mut a = issue("A");
        let b = issue("B");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });
        let mut c = issue("C");
        let d = issue("D");
        c.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: d.id,
        });

        let issues = vec![a, b, c, d];
        let layout = FlowLayout {
            coordinates: vec![
                (0, (0.0, 0.0)),
                (1, (50.0, 0.0)),
                (2, (0.0, 100.0)),
                (3, (50.0, 100.0)),
            ],
            width: 100.0,
            height: 130.0,
        };
        let mut id_prefixes: HashMap<Uuid, String> = HashMap::new();
        id_prefixes.insert(issues[0].id, "aaa".to_string());
        id_prefixes.insert(issues[1].id, "bbb".to_string());
        id_prefixes.insert(issues[2].id, "ccc".to_string());
        id_prefixes.insert(issues[3].id, "ddd".to_string());

        let text = render_text(&issues, &layout, &id_prefixes, None);
        assert_eq!(
            text,
            concat!(
                "═══ Project Flow ═══\n\n",
                "Layer 0:   Layer 1:\n",
                "→ aaa A  ⚠ bbb B\n",
                "\n",
                "→ ccc C  ⚠ ddd D\n",
            ),
        );
    }

    /// Blocked wins the indicator over blocking, a `Done` issue keeps the
    /// arrow only when it still gates other work, and standalone issues
    /// render without a glyph in their own trailing list.
    #[test]
    fn test_render_text_indicators() {
        let mut gate = issue_with_status("Gate", Status::Todo);
        let mut middle = issue_with_status("Middle", Status::Todo);
        let leaf = issue_with_status("Leaf", Status::Todo);
        gate.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: middle.id,
        });
        middle.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: leaf.id,
        });
        let mut done_gate = issue_with_status("Done gate", Status::Done);
        let ready = issue_with_status("Ready", Status::Todo);
        done_gate.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: ready.id,
        });
        let lone_done = issue_with_status("Lone done", Status::Done);

        let issues = vec![gate, middle, leaf, done_gate, ready, lone_done];
        let layout = FlowLayout {
            coordinates: vec![
                (0, (0.0, 0.0)),
                (1, (50.0, 0.0)),
                (2, (100.0, 0.0)),
                (3, (0.0, 100.0)),
                (4, (50.0, 100.0)),
                (5, (0.0, 200.0)),
            ],
            width: 150.0,
            height: 230.0,
        };
        let mut id_prefixes: HashMap<Uuid, String> = HashMap::new();
        id_prefixes.insert(issues[0].id, "aaa".to_string());
        id_prefixes.insert(issues[1].id, "bbb".to_string());
        id_prefixes.insert(issues[2].id, "ccc".to_string());
        id_prefixes.insert(issues[3].id, "ddd".to_string());
        id_prefixes.insert(issues[4].id, "eee".to_string());
        id_prefixes.insert(issues[5].id, "fff".to_string());

        let text = render_text(&issues, &layout, &id_prefixes, None);
        assert_eq!(
            text,
            concat!(
                "═══ Project Flow ═══\n\n",
                "Layer 0:           Layer 1:        Layer 2:\n",
                "→ aaa Gate       ⚠ bbb Middle  ⚠ ccc Leaf\n",
                "\n",
                "→ ddd Done gate  ✓ eee Ready   \n",
                "\n",
                "Standalone issues (1 total):\n",
                "  fff Lone done\n",
            ),
        );
    }

    /// Titles longer than `FLOW_TITLE_MAX` characters are truncated with
    /// "..." at a consistent width.
    #[test]
    fn test_render_text_truncates_titles() {
        let long = "0123456789012345678901234567890123456789"; // 40 chars
        let mut a = issue(long);
        let b = issue("B");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });
        let issues = vec![a, b];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0)), (1, (50.0, 0.0))],
            width: 100.0,
            height: 30.0,
        };
        let text = render_text(&issues, &layout, &HashMap::new(), None);
        assert!(
            text.contains("012345678901234567890123456..."),
            "truncated title missing: {text}"
        );
        assert!(
            !text.contains("01234567890123456789012345678901"),
            "title leaked past the truncation width: {text}"
        );
    }

    /// A flow with only standalone issues renders them as a plain list and
    /// no layer header row.
    #[test]
    fn test_render_text_standalone_only() {
        let issues = vec![issue_with_status("Lone", Status::Todo)];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0))],
            width: 100.0,
            height: 30.0,
        };
        let text = render_text(&issues, &layout, &HashMap::new(), None);
        assert_eq!(
            text,
            format!(
                "═══ Project Flow ═══\n\nStandalone issues (1 total):\n  {} Lone\n",
                issues[0].id.to_string()[..8].to_string()
            ),
        );
    }

    /// An empty layout yields the same "nothing to show" message as before.
    #[test]
    fn test_render_text_empty() {
        let text = render_text(
            &vec![],
            &FlowLayout {
                coordinates: Vec::new(),
                width: 0.0,
                height: 0.0,
            },
            &HashMap::new(),
            None,
        );
        assert_eq!(text, "No issues to visualize.");
    }

    /// `compute_flow_layout` must reject cyclic dependency graphs with an
    /// error instead of panicking inside rust-sugiyama.
    #[test]
    fn test_compute_flow_layout_rejects_cycle() {
        let mut a = issue("A");
        let mut b = issue("B");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: a.id,
        });
        let result = compute_flow_layout(&[a, b]);
        assert!(
            result.is_err(),
            "cyclic graph must be an error, not a panic"
        );
    }

    /// A self-edge is the smallest possible cycle.
    #[test]
    fn test_compute_flow_layout_rejects_self_cycle() {
        let mut a = issue("A");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: a.id,
        });
        let result = compute_flow_layout(&[a]);
        assert!(result.is_err(), "self-edge must be an error, not a panic");
    }

    /// SVG text content must be XML-escaped.
    #[test]
    fn test_render_svg_escapes_titles() {
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0))],
            width: 100.0,
            height: 30.0,
        };
        let issues = vec![issue("Fix <script> & \"quotes\" bug")];
        let svg = render_svg(&issues, &layout, &HashMap::new(), None);
        assert!(!svg.contains("<script>"), "raw markup leaked into SVG");
        assert!(svg.contains("&lt;script&gt;"), "escaped text missing");
        assert!(svg.contains("&amp;"), "& not escaped");
    }

    /// `render_dot` emits a deterministic digraph: one cluster subgraph per
    /// connected component, relationship labels on edges (part_of excluded),
    /// per-status node colors, and standalone issues at the top level.
    #[test]
    fn test_render_dot_fixture() {
        let mut a = issue_with_status("A", Status::Done);
        let mut b = issue("B");
        b.status = Status::Todo;
        let c = issue("C");
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::PartOf,
            target_id: a.id,
        });
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: a.id,
        });
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::ScheduledBefore,
            target_id: a.id,
        });

        let mut id_prefixes: HashMap<Uuid, String> = HashMap::new();
        id_prefixes.insert(a.id, "aaa".to_string());
        id_prefixes.insert(b.id, "bbb".to_string());
        id_prefixes.insert(c.id, "ccc".to_string());

        let issues = vec![a, b, c];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0)), (1, (0.0, 30.0)), (2, (0.0, 60.0))],
            width: 100.0,
            height: 90.0,
        };
        let dot = render_dot(&issues, &layout, &id_prefixes, None);
        assert_eq!(
            dot,
            concat!(
                "digraph pebbles_flow {\n",
                "  rankdir=LR;\n",
                "  node [shape=box];\n",
                "\n",
                "  subgraph cluster_0 {\n",
                "    \"aaa\" [label=\"A\" color=\"green\" style=filled];\n",
                "    \"bbb\" [label=\"B\" color=\"yellow\" style=filled];\n",
                "\n",
                "    \"bbb\" -> \"aaa\" [label=\"required_for\"];\n",
                "    \"bbb\" -> \"aaa\" [label=\"scheduled_before\"];\n",
                "  }\n",
                "\n",
                "  \"ccc\" [label=\"C\" color=\"white\" style=filled];\n",
                "}\n",
            ),
        );
    }

    /// DOT labels must escape quotes, backslashes, and newlines so the
    /// output stays a single valid double-quoted DOT string.
    #[test]
    fn test_render_dot_escapes_titles() {
        let issue = issue("Say \"hi\" \\ line\nnext");
        let mut id_prefixes: HashMap<Uuid, String> = HashMap::new();
        id_prefixes.insert(issue.id, "xxx".to_string());
        let issues = vec![issue];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0))],
            width: 100.0,
            height: 30.0,
        };
        let dot = render_dot(&issues, &layout, &id_prefixes, None);
        assert_eq!(
            dot,
            concat!(
                "digraph pebbles_flow {\n",
                "  rankdir=LR;\n",
                "  node [shape=box];\n",
                "\n",
                "  \"xxx\" [label=\"Say \\\"hi\\\" \\\\ line\\nnext\" color=\"white\" style=filled];\n",
                "}\n",
            ),
        );
    }

    /// DOT labels truncate the title to 30 chars like the SVG renderer.
    #[test]
    fn test_render_dot_truncates_titles() {
        let issue = issue("0123456789012345678901234567890123456789");
        let dot = render_dot(
            &vec![issue],
            &FlowLayout {
                coordinates: vec![(0, (0.0, 0.0))],
                width: 100.0,
                height: 30.0,
            },
            &HashMap::new(),
            None,
        );
        assert!(
            dot.contains("label=\"012345678901234567890123456789\""),
            "title not truncated to 30 chars: {dot}"
        );
        assert!(
            !dot.contains("label=\"0123456789012345678901234567890"),
            "title leaked past 30 chars: {dot}"
        );
    }

    /// An empty layout still yields a valid empty digraph that Graphviz
    /// can consume.
    #[test]
    fn test_render_dot_empty() {
        let dot = render_dot(
            &vec![],
            &FlowLayout {
                coordinates: Vec::new(),
                width: 0.0,
                height: 0.0,
            },
            &HashMap::new(),
            None,
        );
        assert_eq!(
            dot,
            concat!(
                "digraph pebbles_flow {\n",
                "  rankdir=LR;\n",
                "  node [shape=box];\n",
                "}\n",
            ),
        );
    }

    fn issue_with_id(title: &str, id: &str) -> Issue {
        let mut issue = issue(title);
        issue.id = Uuid::parse_str(id).unwrap();
        issue
    }

    /// `render_json` emits the full schema for a three-issue chain: one
    /// node per issue with id, short_id, title, type, status, and layout
    /// position; one edge per dependency with its relationship type
    /// (`part_of` excluded); and the layout summary with the layer count.
    #[test]
    fn test_render_json_chain_fixture() {
        let mut a = issue_with_id("Design auth flow", "11111111-1111-1111-1111-111111111111");
        a.status = Status::Done;
        a.issue_type = IssueType::Usecase;
        let mut b = issue_with_id("Implement login", "22222222-2222-2222-2222-222222222222");
        b.status = Status::InProgress;
        b.issue_type = IssueType::UxProblem;
        let c = issue_with_id("Login bug", "33333333-3333-3333-3333-333333333333");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: c.id,
        });
        b.relationships.push(crate::models::Relationship {
            relation_type: RelationType::PartOf,
            target_id: a.id,
        });

        let issues = vec![a, b, c];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0)), (1, (50.0, 30.0)), (2, (100.0, 60.0))],
            width: 150.0,
            height: 90.0,
        };
        let json = render_json(&issues, &layout, &HashMap::new(), None);
        // The document parses as JSON.
        let root = serde_json::from_str::<Value>(&json).unwrap();
        assert!(root.is_object());

        let nodes = root.get("nodes").unwrap().as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let n0 = nodes[0].clone();
        assert_eq!(
            n0.get("id").unwrap().as_str().unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(n0.get("short_id").unwrap().as_str().unwrap(), "11111111");
        assert_eq!(
            n0.get("title").unwrap().as_str().unwrap(),
            "Design auth flow"
        );
        assert_eq!(n0.get("type").unwrap().as_str().unwrap(), "usecase");
        assert_eq!(n0.get("status").unwrap().as_str().unwrap(), "done");
        assert_eq!(n0.get("x").unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(n0.get("y").unwrap().as_f64().unwrap(), 0.0);
        // Without a goal thread no node carries in_critical_path.
        assert!(n0.get("in_critical_path").is_none());
        let n1 = nodes[1].clone();
        assert_eq!(n1.get("type").unwrap().as_str().unwrap(), "ux_problem");
        assert_eq!(n1.get("status").unwrap().as_str().unwrap(), "in_progress");
        assert_eq!(n1.get("x").unwrap().as_f64().unwrap(), 50.0);

        let edges = root.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges.len(), 2);
        let e0 = edges[0].clone();
        assert_eq!(
            e0.get("source").unwrap().as_str().unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(
            e0.get("target").unwrap().as_str().unwrap(),
            "22222222-2222-2222-2222-222222222222"
        );
        assert_eq!(e0.get("type").unwrap().as_str().unwrap(), "required_for");
        let e1 = edges[1].clone();
        assert_eq!(
            e1.get("source").unwrap().as_str().unwrap(),
            "22222222-2222-2222-2222-222222222222"
        );
        assert_eq!(
            e1.get("target").unwrap().as_str().unwrap(),
            "33333333-3333-3333-3333-333333333333"
        );
        assert_eq!(e1.get("type").unwrap().as_str().unwrap(), "required_for");

        let layout_json = root.get("layout").unwrap();
        assert_eq!(layout_json.get("width").unwrap().as_f64().unwrap(), 150.0);
        assert_eq!(layout_json.get("height").unwrap().as_f64().unwrap(), 90.0);
        assert_eq!(layout_json.get("layers").unwrap().as_u64().unwrap(), 3u64);
    }

    /// When a goal thread is given every node carries
    /// `"in_critical_path": false` (the critical-chain computation itself
    /// is loan e4937f0b).
    #[test]
    fn test_render_json_goal_thread_emits_in_critical_path() {
        let a = issue_with_id("A", "11111111-1111-1111-1111-111111111111");
        let b = issue_with_id("B", "22222222-2222-2222-2222-222222222222");
        let issues = vec![a, b];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0)), (1, (0.0, 30.0))],
            width: 100.0,
            height: 60.0,
        };
        let json = render_json(&issues, &layout, &HashMap::new(), Some(issues[1].id));
        let root = serde_json::from_str::<Value>(&json).unwrap();
        let nodes = root.get("nodes").unwrap().as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        for i in 0..nodes.len() {
            assert_eq!(
                nodes[i].get("in_critical_path").unwrap().as_bool().unwrap(),
                false
            );
        }
    }

    /// An empty flow renders an empty document: no nodes, no edges, and a
    /// zeroed layout.
    #[test]
    fn test_render_json_empty() {
        let json = render_json(
            &vec![],
            &FlowLayout {
                coordinates: Vec::new(),
                width: 0.0,
                height: 0.0,
            },
            &HashMap::new(),
            None,
        );
        let root = serde_json::from_str::<Value>(&json).unwrap();
        assert!(root.get("nodes").unwrap().as_array().unwrap().is_empty());
        assert!(root.get("edges").unwrap().as_array().unwrap().is_empty());
        let layout_json = root.get("layout").unwrap();
        assert_eq!(layout_json.get("width").unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(layout_json.get("height").unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(layout_json.get("layers").unwrap().as_u64().unwrap(), 0u64);
    }

    /// `render_json` is deterministic: two calls with identical inputs
    /// yield identical output bytes.
    #[test]
    fn test_render_json_deterministic() {
        let mut a = issue_with_id("A", "11111111-1111-1111-1111-111111111111");
        let b = issue_with_id("B", "22222222-2222-2222-2222-222222222222");
        a.relationships.push(crate::models::Relationship {
            relation_type: RelationType::ScheduledBefore,
            target_id: b.id,
        });
        let issues = vec![a, b];
        let layout = FlowLayout {
            coordinates: vec![(0, (0.0, 0.0)), (1, (50.0, 0.0))],
            width: 100.0,
            height: 30.0,
        };
        let first = render_json(&issues, &layout, &HashMap::new(), None);
        let second = render_json(&issues, &layout, &HashMap::new(), None);
        assert_eq!(first, second);
    }

    /// Titles containing quotes and backslashes are JSON-escaped and
    /// round-trip through a JSON parser unchanged.
    #[test]
    fn test_render_json_escapes_titles() {
        let escaped = issue("Say \"hi\" \\ line");
        let json = render_json(
            &vec![escaped],
            &FlowLayout {
                coordinates: vec![(0, (0.0, 0.0))],
                width: 100.0,
                height: 30.0,
            },
            &HashMap::new(),
            None,
        );
        assert!(json.contains("Say \\\"hi\\\" \\\\ line"));
        let root = serde_json::from_str::<Value>(&json).unwrap();
        let title = root.get("nodes").unwrap().as_array().unwrap()[0]
            .get("title")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(title, "Say \"hi\" \\ line");
    }
}
