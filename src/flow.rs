use crate::models::{Issue, RelationType};
use anyhow::Result;
use rust_sugiyama::configure::Config;
use std::collections::HashMap;

/// Represents the layout result for visualization
pub struct FlowLayout {
    /// Coordinates for each issue: (issue_index, (x, y))
    pub coordinates: Vec<(usize, (f64, f64))>,
    /// Width of the layout
    pub width: f64,
    /// Height of the layout
    pub height: f64,
    /// Mapping from layout index back to issue
    pub _index_to_issue: HashMap<usize, usize>,
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

/// Compute layout using rust-sugiyama
pub fn compute_flow_layout(issues: &[Issue]) -> Result<FlowLayout> {
    if issues.is_empty() {
        return Ok(FlowLayout {
            coordinates: Vec::new(),
            width: 0.0,
            height: 0.0,
            _index_to_issue: HashMap::new(),
        });
    }

    let edges = build_dependency_graph(issues);

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

    let _index_to_issue: HashMap<usize, usize> =
        all_coordinates.iter().map(|&(idx, _)| (idx, idx)).collect();

    Ok(FlowLayout {
        coordinates: all_coordinates,
        width: max_width,
        height: total_height,
        _index_to_issue,
    })
}

/// Render layout as text in columns
pub fn render_text(issues: &[Issue], layout: &FlowLayout) -> String {
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
                let short_id = &issue.id.to_string()[..8];
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
            let short_id = &issue.id.to_string()[..8];
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
pub fn render_svg(issues: &[Issue], layout: &FlowLayout) -> String {
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
        let short_id = &issue.id.to_string()[..8];
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
            short_id,
            &issue.title.chars().take(30).collect::<String>()
        ));
    }

    svg.push_str("</svg>\n");
    svg
}
