//! Revision comparison metadata, independent of GTK.

use crate::model::diff::Diff;

/// Kind of a suggested revision input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RevisionKind {
    /// The current checked-out commit.
    Head,
    /// A local branch.
    LocalBranch,
    /// A remote-tracking branch.
    RemoteBranch,
    /// A tag, lightweight or annotated.
    Tag,
}

impl RevisionKind {
    /// Human-readable kind label for suggestion lists.
    pub fn label(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::LocalBranch => "branch",
            Self::RemoteBranch => "remote",
            Self::Tag => "tag",
        }
    }
}

/// A suggestion for the From/To revision fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionCandidate {
    /// Name shown to the user.
    pub label: String,
    /// Unambiguous Git revision expression inserted into the field.
    pub spec: String,
    /// Kind shown beside the name.
    pub kind: RevisionKind,
}

/// A read-only diff between two resolved commit object IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionComparison {
    /// User-entered From expression, trimmed at submit time.
    pub from_input: String,
    /// User-entered To expression, trimmed at submit time.
    pub to_input: String,
    /// Full object ID resolved from `from_input`.
    pub from_oid: String,
    /// Full object ID resolved from `to_input`.
    pub to_oid: String,
    /// Standard parsed and intraline-enriched Git diff.
    pub diff: Diff,
}
