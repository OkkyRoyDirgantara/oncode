//! Per-row graph slice geometry: lane positions, edge shapes, and graph column width.

use oncode_git::{Edge, EdgeKind, GraphRow};

pub const LANE_WIDTH: f32 = 14.0;
pub const ROW_HEIGHT: f32 = 26.0;
pub const DOT_RADIUS: f32 = 4.0;
/// Lanes beyond this limit are not drawn (the graph column is clipped).
pub const MAX_LANES: usize = 20;
/// Number of lane colors.
pub const LANE_COLORS: usize = 8;

/// The x coordinate of a lane's center.
pub fn lane_x(lane: u16) -> f32 {
    lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0
}

/// Lane color index (0..LANE_COLORS) for any color value.
pub fn lane_color_index(color: i32) -> usize {
    color.rem_euclid(LANE_COLORS as i32) as usize
}

/// One step of a path, with the same meaning as an SVG path command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// A quadratic curve towards `to` with control point `ctrl`.
    QuadTo { ctrl: (f32, f32), to: (f32, f32) },
}

/// Path for one edge in a row of height `height`.
/// - `Through`: straight from top to bottom.
/// - `Top`: from lane `from` at the top edge to the commit dot in lane `to`.
/// - `Bottom`: from the commit dot in lane `from` to lane `to` at the bottom edge.
///
/// An edge that changes lanes turns with a rounded corner and then runs horizontally to the commit dot.
pub fn edge_path(from: u16, to: u16, kind: EdgeKind, height: f32) -> Vec<Segment> {
    use Segment::*;
    let mid = height / 2.0;
    let x1 = lane_x(from);
    let x2 = lane_x(to);
    if kind == EdgeKind::Through {
        return vec![MoveTo(x1, 0.0), LineTo(x1, height)];
    }
    if x1 == x2 {
        return match kind {
            EdgeKind::Top => vec![MoveTo(x1, 0.0), LineTo(x1, mid)],
            _ => vec![MoveTo(x1, mid), LineTo(x1, height)],
        };
    }
    let dir = if x2 > x1 { 1.0 } else { -1.0 };
    let r = (LANE_WIDTH / 2.0).min(mid).min((x2 - x1).abs());
    if kind == EdgeKind::Top {
        // Go down the source lane, turn, then run horizontally to the commit dot.
        return vec![MoveTo(x1, 0.0), LineTo(x1, mid - r), QuadTo { ctrl: (x1, mid), to: (x1 + dir * r, mid) }, LineTo(x2, mid)];
    }
    // Run horizontally from the commit dot, turn, then go down the target lane.
    vec![MoveTo(x1, mid), LineTo(x2 - dir * r, mid), QuadTo { ctrl: (x2, mid), to: (x2, mid + r) }, LineTo(x2, height)]
}

pub fn edge_segments(edge: &Edge, height: f32) -> Vec<Segment> {
    edge_path(edge.from, edge.to, edge.kind, height)
}

/// The path in SVG path notation (used by tests and for checking shapes).
pub fn to_svg(segments: &[Segment]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cursor = (0.0f32, 0.0f32);
    for segment in segments {
        match *segment {
            Segment::MoveTo(x, y) => {
                out.push(format!("M {x} {y}"));
                cursor = (x, y);
            }
            Segment::LineTo(x, y) => {
                out.push(if x == cursor.0 { format!("V {y}") } else { format!("H {x}") });
                cursor = (x, y);
            }
            Segment::QuadTo { ctrl, to } => {
                out.push(format!("Q {} {} {} {}", ctrl.0, ctrl.1, to.0, to.1));
                cursor = to;
            }
        }
    }
    out.join(" ")
}

/// The rightmost lane used by these rows (-1 if empty).
pub fn max_lane<'a>(rows: impl IntoIterator<Item = &'a GraphRow>) -> i32 {
    let mut max = -1;
    for row in rows {
        max = max.max(row.column as i32);
        for edge in &row.edges {
            max = max.max(edge.from as i32).max(edge.to as i32);
        }
    }
    max
}

/// Width of the graph column for the rightmost lane `lane`, capped at `MAX_LANES`.
pub fn graph_width(lane: i32) -> f32 {
    ((lane + 1).max(1) as usize).min(MAX_LANES) as f32 * LANE_WIDTH
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::linear_rows;

    fn svg(from: u16, to: u16, kind: EdgeKind, height: f32) -> String {
        to_svg(&edge_path(from, to, kind, height))
    }

    #[test]
    fn lane_x_is_the_centre_of_a_14px_column() {
        assert_eq!(LANE_WIDTH, 14.0);
        assert_eq!(lane_x(0), 7.0);
        assert_eq!(lane_x(1), 21.0);
        assert_eq!(lane_x(3), 49.0);
        assert!(DOT_RADIUS * 2.0 < LANE_WIDTH);
        assert!(DOT_RADIUS * 2.0 < ROW_HEIGHT);
    }

    #[test]
    fn lane_colors_wrap() {
        assert_eq!(lane_color_index(0), 0);
        assert_eq!(lane_color_index(7), 7);
        assert_eq!(lane_color_index(8), 0);
        assert_eq!(lane_color_index(-1), 7);
    }

    #[test]
    fn straight_edges() {
        let h = ROW_HEIGHT;
        assert_eq!(svg(2, 2, EdgeKind::Through, h), "M 35 0 V 26");
        assert_eq!(svg(0, 0, EdgeKind::Top, h), "M 7 0 V 13");
        assert_eq!(svg(1, 1, EdgeKind::Bottom, h), "M 21 13 V 26");
    }

    #[test]
    fn top_edges_bend_towards_the_dot() {
        let h = ROW_HEIGHT;
        // Lane 2 merges into the dot in lane 0: down, turn left, horizontal.
        assert_eq!(svg(2, 0, EdgeKind::Top, h), "M 35 0 V 6 Q 35 13 28 13 H 7");
        assert_eq!(svg(0, 1, EdgeKind::Top, h), "M 7 0 V 6 Q 7 13 14 13 H 21");
    }

    #[test]
    fn bottom_edges_leave_the_dot_and_bend_down() {
        let h = ROW_HEIGHT;
        // The dot in lane 0 branches to lane 2: horizontal, turn, down.
        assert_eq!(svg(0, 2, EdgeKind::Bottom, h), "M 7 13 H 28 Q 35 13 35 20 V 26");
        assert_eq!(svg(3, 1, EdgeKind::Bottom, h), "M 49 13 H 28 Q 21 13 21 20 V 26");
    }

    #[test]
    fn scales_with_a_custom_row_height() {
        assert_eq!(svg(0, 0, EdgeKind::Through, 40.0), "M 7 0 V 40");
        assert_eq!(svg(0, 0, EdgeKind::Top, 40.0), "M 7 0 V 20");
        assert_eq!(svg(1, 0, EdgeKind::Top, 40.0), "M 21 0 V 13 Q 21 20 14 20 H 7");
    }

    #[test]
    fn max_lane_and_graph_width() {
        assert_eq!(max_lane(&[]), -1);
        assert_eq!(graph_width(-1), LANE_WIDTH);

        let mut rows = linear_rows(0, 4);
        rows[2].column = 1;
        rows[2].edges = vec![Edge { from: 1, to: 4, color: 2, kind: EdgeKind::Bottom }];
        rows[3].edges = vec![Edge { from: 3, to: 0, color: 1, kind: EdgeKind::Top }];
        assert_eq!(max_lane(&rows), 4);
        assert_eq!(graph_width(4), 5.0 * LANE_WIDTH);

        assert_eq!(graph_width(MAX_LANES as i32 - 1), MAX_LANES as f32 * LANE_WIDTH);
        assert_eq!(graph_width(200), MAX_LANES as f32 * LANE_WIDTH);
    }
}
