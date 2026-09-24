//! Pure commit-graph layout for the History (BRANCH.md sections 14-30).
//!
//! The layout is computed from the commit DAG alone (object names and parent
//! lists) over the topological order produced by `git log --topo-order`, where
//! every commit comes before its parents (BRANCH.md section 6). Nothing here
//! knows about GTK: the drawing callback only renders these rows
//! (BRANCH.md section 53).
//!
//! Rows speak in lane indices. Every row has a top edge (the incoming lines), a
//! commit node and a bottom edge (the lines towards the parents):
//!
//! ```text
//! Top(i) ───▶ Node ───▶ Bottom(j)
//! ```
//!
//! A lane crossing the row connects `Top(i)` to `Bottom(j)`; when lanes end and
//! the survivors compact left, `i` and `j` record the shift so the renderer can
//! draw the transition (BRANCH.md section 27).

use crate::model::commit::Commit;

/// Shape of the commit node (BRANCH.md section 37).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphNodeKind {
    /// A commit with zero or one parents.
    Normal,
    /// A merge commit (two or more parents).
    Merge,
}

/// The commit node of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphNode {
    /// Lane index of the node; stable across the top and bottom of the row.
    pub lane: usize,
    /// Color slot: constant along a line, mapped to a palette by the renderer
    /// (BRANCH.md sections 29-31).
    pub color_slot: usize,
    /// Node shape.
    pub kind: GraphNodeKind,
}

/// Where a graph edge touches a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphPoint {
    /// The top edge of the row, at the given lane (incoming lines).
    Top(usize),
    /// The commit node.
    Node,
    /// The bottom edge of the row, at the given lane (lines to the parents).
    Bottom(usize),
}

/// A line segment crossing a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphEdge {
    /// Where the segment starts (top of the row or the node).
    pub from: GraphPoint,
    /// Where the segment ends (bottom of the row or the node).
    pub to: GraphPoint,
    /// Color slot of the segment (BRANCH.md section 30).
    pub color_slot: usize,
}

/// Everything needed to draw one History row (BRANCH.md section 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Commit shown by the row.
    pub oid: String,
    /// The commit node.
    pub node: GraphNode,
    /// Segments crossing the row.
    pub edges: Vec<GraphEdge>,
    /// Lanes needed to draw the row (highest lane index + 1).
    pub lane_count: usize,
}

/// The layout of the loaded history (BRANCH.md section 54).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryGraph {
    /// One row per commit, in the same order as the input.
    pub rows: Vec<GraphRow>,
    /// Widest row: the gutter is this wide for every row (BRANCH.md section 68).
    pub max_lanes: usize,
}

/// A lane still open while walking down the history (BRANCH.md section 18).
#[derive(Debug, Clone)]
struct ActiveLane {
    /// Commit the lane is waiting for.
    target_oid: String,
    /// Color of the lane.
    color_slot: usize,
}

/// Where a parent of the current commit continues (BRANCH.md sections 23-26).
enum ParentSlot {
    /// The parent is already expected on the given incoming lane.
    Existing(usize),
    /// The parent needs a new lane.
    New,
    /// The same parent was already placed by an earlier position.
    Duplicate,
}

/// Computes the graph layout of the given topologically ordered commits.
pub fn layout_commits(commits: &[Commit]) -> HistoryGraph {
    let mut active: Vec<ActiveLane> = Vec::new();
    let mut next_color: usize = 0;
    let mut rows: Vec<GraphRow> = Vec::new();

    for commit in commits {
        let incoming_lanes = active.len();
        let mut edges: Vec<GraphEdge> = Vec::new();

        // The lanes already waiting for this commit converge into its node; the
        // leftmost one is the node lane (BRANCH.md sections 19 and 26).
        let matching: Vec<usize> = active
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.target_oid == commit.oid)
            .map(|(index, _)| index)
            .collect();
        let (node_lane, node_color) = match matching.first() {
            Some(&index) => (index, active[index].color_slot),
            None => {
                // A branch tip nobody expects yet: open a new lane on the right
                // (BRANCH.md section 20).
                let color_slot = next_color;
                next_color += 1;
                (incoming_lanes, color_slot)
            }
        };
        for &index in &matching {
            edges.push(GraphEdge {
                from: GraphPoint::Top(index),
                to: GraphPoint::Node,
                color_slot: active[index].color_slot,
            });
        }

        // Classify the parents: reuse the lane of an already expected parent
        // (BRANCH.md section 25) and never open two lanes for the same parent
        // (BRANCH.md section 23).
        let mut slots: Vec<ParentSlot> = Vec::with_capacity(commit.parents.len());
        for (position, parent) in commit.parents.iter().enumerate() {
            let duplicate = commit.parents[..position]
                .iter()
                .any(|earlier| earlier == parent);
            let existing = active.iter().position(|lane| &lane.target_oid == parent);
            slots.push(match (duplicate, existing) {
                (true, _) => ParentSlot::Duplicate,
                (false, Some(lane)) => ParentSlot::Existing(lane),
                (false, None) => ParentSlot::New,
            });
        }

        // Outgoing lane state: the resolved lanes free their slot for the new
        // parent lanes (the first parent keeps the node lane and color, the
        // others open next to it), while the surviving lanes keep their order
        // and compact left (BRANCH.md sections 21-27).
        let mut next: Vec<ActiveLane> = Vec::new();
        let mut relocated: Vec<Option<usize>> = vec![None; incoming_lanes];
        let mut parent_lane: Vec<Option<usize>> = vec![None; commit.parents.len()];
        let mut spliced = false;
        for (index, lane) in active.iter().enumerate() {
            if lane.target_oid == commit.oid {
                if !spliced {
                    spliced = true;
                    splice_parents(
                        commit,
                        &slots,
                        node_color,
                        &mut next,
                        &mut parent_lane,
                        &mut next_color,
                    );
                }
                continue;
            }
            relocated[index] = Some(next.len());
            next.push(ActiveLane {
                target_oid: lane.target_oid.clone(),
                color_slot: lane.color_slot,
            });
        }
        if !spliced {
            // The node opened a new lane: its parents continue below it.
            splice_parents(
                commit,
                &slots,
                node_color,
                &mut next,
                &mut parent_lane,
                &mut next_color,
            );
        }

        // The segments from the node to each parent lane.
        for (position, slot) in slots.iter().enumerate() {
            let bottom = match slot {
                ParentSlot::Duplicate => continue,
                ParentSlot::Existing(lane) => relocated[*lane],
                ParentSlot::New => parent_lane[position],
            };
            let Some(bottom) = bottom else {
                continue;
            };
            edges.push(GraphEdge {
                from: GraphPoint::Node,
                to: GraphPoint::Bottom(bottom),
                // A converging line takes the color of the surviving lane
                // (BRANCH.md section 30).
                color_slot: next[bottom].color_slot,
            });
        }

        // ...and the lanes crossing the row untouched.
        for (index, lane) in active.iter().enumerate() {
            if let Some(bottom) = relocated[index] {
                edges.push(GraphEdge {
                    from: GraphPoint::Top(index),
                    to: GraphPoint::Bottom(bottom),
                    color_slot: lane.color_slot,
                });
            }
        }

        let lane_count = incoming_lanes.max(next.len()).max(node_lane + 1);
        rows.push(GraphRow {
            oid: commit.oid.clone(),
            node: GraphNode {
                lane: node_lane,
                color_slot: node_color,
                kind: if commit.parents.len() > 1 {
                    GraphNodeKind::Merge
                } else {
                    GraphNodeKind::Normal
                },
            },
            edges,
            lane_count,
        });
        active = next;
    }

    let max_lanes = rows.iter().map(|row| row.lane_count).max().unwrap_or(0);
    HistoryGraph { rows, max_lanes }
}

/// Places the new parent lanes into the slot freed by the resolved lanes.
fn splice_parents(
    commit: &Commit,
    slots: &[ParentSlot],
    node_color: usize,
    next: &mut Vec<ActiveLane>,
    parent_lane: &mut [Option<usize>],
    next_color: &mut usize,
) {
    for (position, slot) in slots.iter().enumerate() {
        if !matches!(slot, ParentSlot::New) {
            continue;
        }
        // First-parent continuity: a new first parent keeps the node lane and
        // its color (BRANCH.md sections 21, 24 and 30).
        let color_slot = if position == 0 {
            node_color
        } else {
            let color_slot = *next_color;
            *next_color += 1;
            color_slot
        };
        parent_lane[position] = Some(next.len());
        next.push(ActiveLane {
            target_oid: commit.parents[position].clone(),
            color_slot,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic commit (the layout only needs the DAG).
    fn commit(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.to_owned(),
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
            author_name: "Test".to_owned(),
            author_email: "test@example.com".to_owned(),
            author_time: 0,
            subject: format!("commit {oid}"),
        }
    }

    /// Builds a linear chain `c{n-1} -> ... -> c0 -> tail`.
    fn chain(prefix: &str, count: usize, tail: &[&str]) -> Vec<Commit> {
        (0..count)
            .map(|index| {
                let parents: Vec<String> = if index + 1 < count {
                    vec![format!("{prefix}{}", index + 1)]
                } else {
                    tail.iter().map(|parent| (*parent).to_owned()).collect()
                };
                Commit {
                    oid: format!("{prefix}{index}"),
                    parents,
                    author_name: "Test".to_owned(),
                    author_email: "test@example.com".to_owned(),
                    author_time: 0,
                    subject: format!("commit {prefix}{index}"),
                }
            })
            .collect()
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        // Caso A (BRANCH.md section 58).
        let commits = vec![commit("A", &["B"]), commit("B", &["C"]), commit("C", &[])];
        let graph = layout_commits(&commits);
        assert_eq!(graph.max_lanes, 1);
        for row in &graph.rows {
            assert_eq!(row.node.lane, 0);
            assert_eq!(row.lane_count, 1);
        }
        // No horizontal movement anywhere.
        for edge in graph.rows.iter().flat_map(|row| &row.edges) {
            match (edge.from, edge.to) {
                (GraphPoint::Top(0), GraphPoint::Node)
                | (GraphPoint::Node, GraphPoint::Bottom(0))
                | (GraphPoint::Top(0), GraphPoint::Bottom(0)) => {}
                other => panic!("unexpected edge {other:?}"),
            }
        }
    }

    #[test]
    fn simple_merge_splits_and_converges() {
        // Caso B (BRANCH.md section 58).
        let commits = vec![
            commit("A", &["B", "C"]),
            commit("B", &["D"]),
            commit("C", &["D"]),
            commit("D", &[]),
        ];
        let graph = layout_commits(&commits);

        assert_eq!(graph.rows[0].node.kind, GraphNodeKind::Merge);
        assert_eq!(graph.rows[0].node.lane, 0);
        assert_eq!(graph.rows[0].lane_count, 2);
        // The merge opens one lane per parent.
        let bottoms: Vec<usize> = graph.rows[0]
            .edges
            .iter()
            .filter_map(|edge| match (edge.from, edge.to) {
                (GraphPoint::Node, GraphPoint::Bottom(bottom)) => Some(bottom),
                _ => None,
            })
            .collect();
        assert_eq!(bottoms, vec![0, 1]);

        // B keeps lane 0, C lives on lane 1.
        assert_eq!(graph.rows[1].node.lane, 0);
        assert_eq!(graph.rows[2].node.lane, 1);

        // C converges into D's lane instead of opening a third one.
        assert_eq!(graph.rows[3].node.lane, 0);
        assert!(graph.rows[2].edges.contains(&GraphEdge {
            from: GraphPoint::Node,
            to: GraphPoint::Bottom(0),
            color_slot: 0,
        }));
        assert_eq!(graph.max_lanes, 2);
    }

    #[test]
    fn long_lived_branch_keeps_lane_and_color() {
        // Caso C (BRANCH.md section 58).
        let commits = vec![
            commit("A", &["B", "C"]),
            commit("B", &["D"]),
            commit("C", &["E"]),
            commit("D", &["F"]),
            commit("E", &["G"]),
            commit("F", &["H"]),
            commit("G", &["H"]),
            commit("H", &[]),
        ];
        let graph = layout_commits(&commits);

        // Two lanes, each keeping its position and color across the branches.
        let lanes: Vec<usize> = graph.rows.iter().map(|row| row.node.lane).collect();
        assert_eq!(lanes, vec![0, 0, 1, 0, 1, 0, 1, 0]);
        // Colors follow the lanes: the main line keeps slot 0, the branch slot 1.
        let colors: Vec<usize> = graph.rows.iter().map(|row| row.node.color_slot).collect();
        assert_eq!(colors, vec![0, 0, 1, 0, 1, 0, 1, 0]);
        // G converges into the lane already waiting for H (opened by F).
        assert!(graph.rows[6].edges.contains(&GraphEdge {
            from: GraphPoint::Node,
            to: GraphPoint::Bottom(0),
            color_slot: 0,
        }));
        assert_eq!(graph.max_lanes, 2);
    }

    #[test]
    fn branch_tips_open_new_lanes_and_compact() {
        // Caso D (BRANCH.md section 58).
        let commits = vec![
            commit("A", &["B"]),
            commit("C", &["D"]),
            commit("B", &[]),
            commit("D", &[]),
        ];
        let graph = layout_commits(&commits);
        assert_eq!(graph.rows[0].node.lane, 0);
        // The second tip opens a new lane while the first is still open.
        assert_eq!(graph.rows[1].node.lane, 1);
        assert_eq!(graph.rows[1].lane_count, 2);
        // Once B ends, D compacts back to lane 0 (BRANCH.md section 27).
        assert_eq!(graph.rows[3].node.lane, 0);
        assert_eq!(
            graph.rows[3].edges,
            vec![GraphEdge {
                from: GraphPoint::Top(0),
                to: GraphPoint::Node,
                color_slot: 1,
            }]
        );
        assert_eq!(graph.max_lanes, 2);
    }

    #[test]
    fn an_expected_parent_opens_no_duplicate_lane() {
        // Caso E (BRANCH.md section 58).
        let commits = vec![
            commit("X", &["D"]),
            commit("M", &["B", "D"]),
            commit("B", &["D"]),
            commit("D", &[]),
        ];
        let graph = layout_commits(&commits);
        // M connects to the lane already waiting for D and only B opens a lane.
        assert_eq!(graph.rows[1].node.lane, 1);
        assert_eq!(graph.rows[1].lane_count, 2);
        assert!(graph.rows[1].edges.contains(&GraphEdge {
            from: GraphPoint::Node,
            to: GraphPoint::Bottom(0),
            color_slot: 0,
        }));
        assert_eq!(graph.rows[3].node.lane, 0);
    }

    #[test]
    fn root_commit_ends_the_lane() {
        // Caso F (BRANCH.md section 58).
        let graph = layout_commits(&[commit("R", &[])]);
        assert_eq!(graph.rows[0].node.lane, 0);
        assert_eq!(graph.rows[0].lane_count, 1);
        assert!(graph.rows[0].edges.is_empty());
    }

    #[test]
    fn disconnected_histories_do_not_panic() {
        // Caso G (BRANCH.md section 58).
        let commits = vec![
            commit("A", &["B"]),
            commit("C", &["D"]),
            commit("B", &[]),
            commit("D", &[]),
        ];
        let graph = layout_commits(&commits);
        assert_eq!(graph.rows.len(), 4);
        for row in &graph.rows {
            assert!(row.node.lane < row.lane_count);
        }
    }

    #[test]
    fn octopus_merge_opens_one_lane_per_new_parent() {
        // Caso H (BRANCH.md section 58): each parent keeps its lane open on a
        // continuation, so the three lines are visible at the same time.
        let commits = vec![
            commit("M", &["B", "C", "D"]),
            commit("B", &["B2"]),
            commit("C", &["C2"]),
            commit("D", &["D2"]),
            commit("B2", &[]),
            commit("C2", &[]),
            commit("D2", &[]),
        ];
        let graph = layout_commits(&commits);
        assert_eq!(graph.rows[0].node.kind, GraphNodeKind::Merge);
        assert_eq!(graph.rows[0].lane_count, 3);
        let bottoms: Vec<usize> = graph.rows[0]
            .edges
            .iter()
            .filter_map(|edge| match (edge.from, edge.to) {
                (GraphPoint::Node, GraphPoint::Bottom(bottom)) => Some(bottom),
                _ => None,
            })
            .collect();
        assert_eq!(bottoms, vec![0, 1, 2]);
        let lanes: Vec<usize> = graph.rows[1..4].iter().map(|row| row.node.lane).collect();
        assert_eq!(lanes, vec![0, 1, 2]);
    }

    #[test]
    fn layout_is_stable_when_more_commits_are_loaded() {
        // BRANCH.md sections 48 and 59: loading the next page must not move the
        // rows already laid out.
        let mut commits = vec![commit("T", &["A0", "B0"])];
        commits.extend(chain("A", 10, &[]));
        commits.extend(chain("B", 5, &[]));
        assert_eq!(commits.len(), 16);

        let full = layout_commits(&commits);
        let first_page = layout_commits(&commits[..7]);
        assert_eq!(first_page.rows, full.rows[..7]);

        let second_page = layout_commits(&commits[..12]);
        assert_eq!(second_page.rows, full.rows[..12]);
    }

    #[test]
    fn empty_history_has_no_rows() {
        // BRANCH.md section 7.
        let graph = layout_commits(&[]);
        assert!(graph.rows.is_empty());
        assert_eq!(graph.max_lanes, 0);
    }
}
