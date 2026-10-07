//! Graph gutter rendering for the History (BRANCH.md sections 32-38).
//!
//! Each History row owns a [`gtk4::DrawingArea`] that draws the commit node and
//! the lane segments prepared by the layout engine (BRANCH.md section 53: the
//! draw callback never computes the DAG). Lanes keep a constant width and the
//! gutter is as wide as the widest row (BRANCH.md section 68).

use gtk4::DrawingArea;
use gtk4::cairo::{self, LineCap, LineJoin};
use gtk4::prelude::*;

use crate::graph::{GraphNodeKind, GraphPoint, GraphRow};

/// Horizontal distance between two lanes (BRANCH.md section 34).
const LANE_WIDTH: f64 = 16.0;
/// Radius of the commit node (BRANCH.md section 34).
const NODE_RADIUS: f64 = 4.0;
/// Width of the lane segments (BRANCH.md section 34).
const EDGE_WIDTH: f64 = 2.0;
/// Extend row-to-row edges beyond the DrawingArea clip. Each History row is
/// rasterized independently; ending a stroked path exactly on the clip boundary
/// can leave a one-pixel antialias seam even with square caps. Overdrawing keeps
/// the boundary in the middle of the stroke, then GTK clips it flush.
const EDGE_OVERDRAW: f64 = EDGE_WIDTH;
/// Padding around the lanes (BRANCH.md section 34).
const GUTTER_PADDING: f64 = 6.0;

/// Lane palette slots (BRANCH.md sections 29-31): the layout assigns a
/// `color_slot` and the renderer cycles these colors. The light palette is
/// darker, the dark palette brighter, for enough contrast on both themes
/// (BRANCH.md section 65).
const PALETTE_LIGHT: [[f64; 3]; 8] = [
    [0.31, 0.27, 0.80],
    [0.00, 0.44, 0.60],
    [0.55, 0.29, 0.62],
    [0.70, 0.35, 0.05],
    [0.10, 0.48, 0.22],
    [0.62, 0.10, 0.32],
    [0.42, 0.36, 0.05],
    [0.05, 0.45, 0.50],
];

const PALETTE_DARK: [[f64; 3]; 8] = [
    [0.55, 0.51, 0.95],
    [0.35, 0.72, 0.85],
    [0.78, 0.55, 0.85],
    [0.95, 0.65, 0.35],
    [0.45, 0.78, 0.48],
    [0.90, 0.45, 0.62],
    [0.80, 0.72, 0.35],
    [0.40, 0.75, 0.78],
];

/// Gutter width for a graph with `max_lanes` lanes. Every row uses the same
/// width so all rows stay aligned (BRANCH.md section 68).
pub fn width(max_lanes: usize) -> f64 {
    2.0 * GUTTER_PADDING + max_lanes as f64 * LANE_WIDTH
}

/// Creates the gutter widget for one row.
///
/// The area fills the whole row height so the lanes continue without breaks
/// between rows (BRANCH.md section 35).
pub fn new(row: &GraphRow, max_lanes: usize, dark: bool) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_content_width(width(max_lanes) as i32);
    area.set_vexpand(true);

    let palette = if dark { PALETTE_DARK } else { PALETTE_LIGHT };
    let row = row.clone();
    area.set_draw_func(move |_, context, width, height| {
        draw(context, width as f64, height as f64, &row, &palette);
    });
    area
}

/// Draws the segments and the node of one row.
fn draw(
    context: &cairo::Context,
    _width: f64,
    height: f64,
    row: &GraphRow,
    palette: &[[f64; 3]; 8],
) {
    let center_y = height / 2.0;
    let lane_x = |lane: usize| GUTTER_PADDING + LANE_WIDTH / 2.0 + lane as f64 * LANE_WIDTH;

    context.set_line_width(EDGE_WIDTH);
    // Every row owns a separate DrawingArea. Round caps ending exactly at the
    // clipping edge are only partially rasterized, which leaves a faint seam
    // where two otherwise continuous lane segments meet. Square caps extend
    // half a stroke beyond the row and are clipped flush, so adjacent rows
    // join at full opacity without changing the curve geometry.
    context.set_line_cap(LineCap::Square);
    context.set_line_join(LineJoin::Round);

    // Segments first: the node covers their endpoints.  Top/bottom coordinates
    // deliberately live outside the DrawingArea so independently rendered rows
    // meet with a fully opaque stroke at the shared boundary.
    let top_y = -EDGE_OVERDRAW;
    let bottom_y = height + EDGE_OVERDRAW;
    let mut has_incoming = false;
    let mut has_outgoing = false;
    for edge in &row.edges {
        let color = palette[edge.color_slot % palette.len()];
        context.set_source_rgb(color[0], color[1], color[2]);
        match (edge.from, edge.to) {
            (GraphPoint::Top(from), GraphPoint::Bottom(to)) => {
                segment(context, lane_x(from), top_y, lane_x(to), bottom_y);
                context.stroke().ok();
                seal_top_boundary(context, lane_x(from));
                seal_bottom_boundary(context, lane_x(to), height);
            }
            (GraphPoint::Top(from), GraphPoint::Node) => {
                has_incoming = true;
                segment(context, lane_x(from), top_y, lane_x(row.node.lane), center_y);
                context.stroke().ok();
                seal_top_boundary(context, lane_x(from));
            }
            (GraphPoint::Node, GraphPoint::Bottom(to)) => {
                has_outgoing = true;
                segment(context, lane_x(row.node.lane), center_y, lane_x(to), bottom_y);
                context.stroke().ok();
                seal_bottom_boundary(context, lane_x(to), height);
            }
            _ => {}
        }
    }

    let node_color = palette[row.node.color_slot % palette.len()];
    context.set_source_rgb(node_color[0], node_color[1], node_color[2]);

    // The line of a root commit ends just below its node (BRANCH.md section 38).
    // It is not a row-boundary continuation, so keep the rounded terminal cap.
    context.set_line_cap(LineCap::Round);
    if has_incoming && !has_outgoing {
        context.move_to(lane_x(row.node.lane), center_y);
        context.line_to(
            lane_x(row.node.lane),
            (center_y + 2.0 * NODE_RADIUS).min(height),
        );
        context.stroke().ok();
    }

    // The commit node (BRANCH.md section 37).
    let node_x = lane_x(row.node.lane);
    context.arc(node_x, center_y, NODE_RADIUS, 0.0, std::f64::consts::TAU);
    context.fill().ok();

    // A ring marks merge commits.
    if matches!(row.node.kind, GraphNodeKind::Merge) {
        context.set_line_width(1.5);
        context.arc(
            node_x,
            center_y,
            NODE_RADIUS + 1.5,
            0.0,
            std::f64::consts::TAU,
        );
        context.stroke().ok();
    }
}

/// Paints the first logical pixel of a lane at a row boundary at full opacity.
///
/// GTK composites each row's DrawingArea independently. Even when the stroke is
/// overdrawn past the clip, the clip edge can still slightly attenuate the
/// boundary pixel. Filling the first/last in-row pixel explicitly removes that
/// final hairline without changing the visible lane geometry.
fn seal_top_boundary(context: &cairo::Context, x: f64) {
    context.rectangle(x - EDGE_WIDTH / 2.0, 0.0, EDGE_WIDTH, 1.0);
    context.fill().ok();
}

fn seal_bottom_boundary(context: &cairo::Context, x: f64, height: f64) {
    context.rectangle(
        x - EDGE_WIDTH / 2.0,
        (height - 1.0).max(0.0),
        EDGE_WIDTH,
        1.0,
    );
    context.fill().ok();
}

/// Strokes a segment between two points: vertical when the lane continues,
/// a smooth curve when the line shifts between lanes (BRANCH.md sections 27
/// and 36).
fn segment(context: &cairo::Context, from_x: f64, from_y: f64, to_x: f64, to_y: f64) {
    context.move_to(from_x, from_y);
    if (from_x - to_x).abs() < 0.5 {
        context.line_to(to_x, to_y);
    } else {
        let middle = (from_y + to_y) / 2.0;
        context.curve_to(from_x, middle, to_x, middle, to_x, to_y);
    }
}
