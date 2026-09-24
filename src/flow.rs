use crate::models::{Issue, RelationType, Status};
use anyhow::{bail, Result};
use rust_sugiyama::configure::Config;
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

/// Render layout as text in columns
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

    // Identify issues with dependencies (in edges)
    let edges = build_dependency_graph(issues);
    let mut has_edges: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (source, target) in &edges {
        has_edges.insert(*source as usize);
        has_edges.insert(*target as usize);
    }

    // Group by x-coordinate (layer)
    // After swap, X is the layer coordinate (was Y in sugiyama)
    // rust-sugiyama uses spacing of ~50px between layers
    let mut layers: HashMap<i32, Vec<(usize, f64)>> = HashMap::new();
    let mut standalone: Vec<(usize, f64)> = Vec::new();

    for &(idx, (x, y)) in &layout.coordinates {
        if has_edges.contains(&idx) {
            // Use 50px buckets to match rust-sugiyama's layer spacing
            let layer = (x / 50.0).round() as i32;
            layers.entry(layer).or_default().push((idx, y));
        } else {
            standalone.push((idx, y));
        }
    }

    let mut sorted_layers: Vec<_> = layers.iter().collect();
    sorted_layers.sort_by_key(|(layer, _)| *layer);

    let mut output = String::new();
    output.push_str("═══ Project Flow ═══\n\n");

    // Show dependency chains first
    if !sorted_layers.is_empty() {
        output.push_str("Issues with dependencies:\n\n");
        for (layer_num, layer_issues) in sorted_layers {
            output.push_str(&format!("Layer {}:\n", layer_num));

            // Sort by y coordinate (vertical position within layer)
            let mut sorted_issues = layer_issues.clone();
            sorted_issues.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            for (idx, _y) in sorted_issues {
                let issue = &issues[idx];
                let short_id = issue_id_prefix(issues[idx].id, id_prefixes);
                output.push_str(&format!(
                    "  {} [{}] [{}] {}\n",
                    short_id,
                    issue.issue_type.short_code(),
                    issue.status,
                    issue.title
                ));
            }
            output.push('\n');
        }
    }

    // Show standalone issues at the end
    if !standalone.is_empty() {
        output.push_str(&format!(
            "\nStandalone issues ({} total):\n",
            standalone.len()
        ));
        standalone.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        for (idx, _y) in standalone {
            let issue = &issues[idx];
            let short_id = issue_id_prefix(issues[idx].id, id_prefixes);
            output.push_str(&format!(
                "  {} [{}] [{}] {}\n",
                short_id,
                issue.issue_type.short_code(),
                issue.status,
                issue.title
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Issue, IssueType, Status};

    fn issue(title: &str) -> Issue {
        Issue::new(title.to_string(), None, IssueType::Bug)
    }

    fn issue_with_status(title: &str, status: Status) -> Issue {
        let mut issue = issue(title);
        issue.status = status;
        issue
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
}
