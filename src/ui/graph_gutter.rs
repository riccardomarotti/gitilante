//! Graph gutter rendering for the History (BRANCH.md sections 32-38).
//!
//! The whole repository History graph is rendered by one overlaid
//! [`gtk4::DrawingArea`]. Commit rows only reserve the gutter width. Drawing
//! all lanes in a single Cairo surface removes rasterization seams at ListBox
//! row boundaries while keeping the pure DAG layout in [`crate::graph`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::cairo::{self, LineCap, LineJoin};
use gtk4::prelude::*;
use gtk4::{Align, DrawingArea, ListBox};

use crate::graph::{GraphNodeKind, GraphPoint, GraphRow, HistoryGraph};

/// Horizontal distance between two lanes (BRANCH.md section 34).
const LANE_WIDTH: f64 = 16.0;
/// Radius of the commit node (BRANCH.md section 34).
const NODE_RADIUS: f64 = 4.0;
/// Width of the lane segments (BRANCH.md section 34).
const EDGE_WIDTH: f64 = 2.0;
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

/// Gutter width for a graph with `max_lanes` lanes. Every row reserves the
/// same width so graph nodes and commit text stay aligned.
pub fn width(max_lanes: usize) -> f64 {
    2.0 * GUTTER_PADDING + max_lanes as f64 * LANE_WIDTH
}

struct State {
    graph: RefCell<Option<HistoryGraph>>,
    dark: Cell<bool>,
}

/// One graph canvas shared by all repository History rows.
pub struct GraphGutter {
    area: DrawingArea,
    state: Rc<State>,
}

impl GraphGutter {
    pub fn new(list: &ListBox) -> Self {
        let area = DrawingArea::new();
        area.set_halign(Align::Start);
        area.set_valign(Align::Fill);
        area.set_vexpand(true);
        area.set_can_target(false);
        area.set_focusable(false);
        area.set_visible(false);

        let state = Rc::new(State {
            graph: RefCell::new(None),
            dark: Cell::new(false),
        });

        let list = list.downgrade();
        let draw_state = state.clone();
        area.set_draw_func(move |_, context, _, height| {
            let Some(list) = list.upgrade() else {
                return;
            };
            let graph = draw_state.graph.borrow();
            let Some(graph) = graph.as_ref() else {
                return;
            };
            let palette = if draw_state.dark.get() {
                &PALETTE_DARK
            } else {
                &PALETTE_LIGHT
            };
            draw_graph(context, height as f64, &list, graph, palette);
        });

        Self { area, state }
    }

    pub fn widget(&self) -> &DrawingArea {
        &self.area
    }

    /// Replaces the graph shown by the canvas. `None` is used by File History,
    /// which intentionally has no commit graph.
    pub fn update(&self, graph: Option<&HistoryGraph>, dark: bool) {
        self.state.dark.set(dark);
        *self.state.graph.borrow_mut() = graph.cloned();

        if let Some(graph) = graph {
            self.area.set_content_width(width(graph.max_lanes) as i32);
            self.area.set_visible(!graph.rows.is_empty());
        } else {
            self.area.set_visible(false);
        }
        self.area.queue_draw();
    }
}

#[derive(Debug, Clone, Copy)]
struct RowGeometry {
    top: f64,
    center: f64,
    bottom: f64,
}

/// Draws all loaded commit rows in one Cairo surface.
///
/// The seam between two logical graph rows is defined once and shared by both
/// rows. Even if the ListBox theme leaves fractional spacing between widgets,
/// the outgoing edge above and incoming edge below meet at exactly the same
/// coordinate.
fn draw_graph(
    context: &cairo::Context,
    canvas_height: f64,
    list: &ListBox,
    graph: &HistoryGraph,
    palette: &[[f64; 3]; 8],
) {
    let mut bounds = Vec::with_capacity(graph.rows.len());
    for index in 0..graph.rows.len() {
        let Some(row) = list.row_at_index(index as i32) else {
            break;
        };
        let Some(rect) = row.compute_bounds(list) else {
            break;
        };
        let top = f64::from(rect.y());
        bounds.push((top, top + f64::from(rect.height())));
    }

    if bounds.is_empty() {
        return;
    }

    let mut geometry = Vec::with_capacity(bounds.len());
    for index in 0..bounds.len() {
        let (row_top, row_bottom) = bounds[index];
        let top = if index == 0 {
            row_top.max(0.0)
        } else {
            (bounds[index - 1].1 + row_top) / 2.0
        };
        let bottom = if index + 1 == bounds.len() {
            row_bottom.min(canvas_height)
        } else {
            (row_bottom + bounds[index + 1].0) / 2.0
        };
        geometry.push(RowGeometry {
            top,
            center: (row_top + row_bottom) / 2.0,
            bottom,
        });
    }

    for (row, geometry) in graph.rows.iter().zip(geometry) {
        draw_row(context, row, geometry, palette);
    }
}

/// Draws the segments and commit node for one logical History row.
fn draw_row(
    context: &cairo::Context,
    row: &GraphRow,
    geometry: RowGeometry,
    palette: &[[f64; 3]; 8],
) {
    let lane_x = |lane: usize| GUTTER_PADDING + LANE_WIDTH / 2.0 + lane as f64 * LANE_WIDTH;

    context.set_line_width(EDGE_WIDTH);
    // All rows are now painted into the same Cairo surface and adjacent
    // segments share the exact same boundary coordinate. Square caps would
    // extend half a stroke past that coordinate, so two neighbouring rows
    // overlap and leave a visible little joint. Butt caps meet exactly at the
    // shared boundary with no overlap and no per-widget clipping seam.
    context.set_line_cap(LineCap::Butt);
    context.set_line_join(LineJoin::Round);

    // Segments first: the node covers their endpoints.
    let mut has_incoming = false;
    let mut has_outgoing = false;
    for edge in &row.edges {
        let color = palette[edge.color_slot % palette.len()];
        context.set_source_rgb(color[0], color[1], color[2]);
        match (edge.from, edge.to) {
            (GraphPoint::Top(from), GraphPoint::Bottom(to)) => {
                segment(
                    context,
                    lane_x(from),
                    geometry.top,
                    lane_x(to),
                    geometry.bottom,
                );
            }
            (GraphPoint::Top(from), GraphPoint::Node) => {
                has_incoming = true;
                segment(
                    context,
                    lane_x(from),
                    geometry.top,
                    lane_x(row.node.lane),
                    geometry.center,
                );
            }
            (GraphPoint::Node, GraphPoint::Bottom(to)) => {
                has_outgoing = true;
                segment(
                    context,
                    lane_x(row.node.lane),
                    geometry.center,
                    lane_x(to),
                    geometry.bottom,
                );
            }
            _ => continue,
        }
        context.stroke().ok();
    }

    let node_color = palette[row.node.color_slot % palette.len()];
    context.set_source_rgb(node_color[0], node_color[1], node_color[2]);

    // The line of a root commit ends just below its node (BRANCH.md section 38).
    if has_incoming && !has_outgoing {
        context.set_line_cap(LineCap::Round);
        context.move_to(lane_x(row.node.lane), geometry.center);
        context.line_to(
            lane_x(row.node.lane),
            (geometry.center + 2.0 * NODE_RADIUS).min(geometry.bottom),
        );
        context.stroke().ok();
    }

    // The commit node (BRANCH.md section 37).
    let node_x = lane_x(row.node.lane);
    context.arc(
        node_x,
        geometry.center,
        NODE_RADIUS,
        0.0,
        std::f64::consts::TAU,
    );
    context.fill().ok();

    // A ring marks merge commits.
    if matches!(row.node.kind, GraphNodeKind::Merge) {
        context.set_line_width(1.5);
        context.arc(
            node_x,
            geometry.center,
            NODE_RADIUS + 1.5,
            0.0,
            std::f64::consts::TAU,
        );
        context.stroke().ok();
    }
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
