//! Operation-aware conflict context and UI labels.
//!
//! The UI must never expose Git's `ours`/`theirs` wording: labels describe
//! where each version comes from for the operation actually in progress. When
//! the provenance is uncertain the labels stay neutral instead of guessing.
//! See docs/GITILANTE_CONFLICT_SOLVER_SPEC.md, sections 4-10 and 23-24.

/// The Git operation in progress that produced the conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictOperation {
    /// A merge stopped on conflicts (`MERGE_HEAD`).
    Merge,
    /// A rebase replayed a commit and stopped (`REBASE_HEAD`).
    Rebase,
    /// A cherry-pick stopped on conflicts (`CHERRY_PICK_HEAD`).
    CherryPick,
    /// A revert stopped on conflicts (`REVERT_HEAD`).
    Revert,
    /// The operation could not be determined.
    Unknown,
}

/// Raw identity of a commit involved in a conflict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitIdentity {
    /// Full OID of the commit, when known.
    pub oid: Option<String>,
    /// Commit subject line, when known.
    pub subject: Option<String>,
    /// Branch short names pointing at this commit.
    pub refs: Vec<String>,
}

/// One side of a conflict with its resolved display label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictSource {
    /// Display label resolved for the operation (never `ours`/`theirs`).
    pub label: String,
    /// Full OID of this side, when known.
    pub oid: Option<String>,
    /// Commit subject of this side, when known.
    pub subject: Option<String>,
    /// Branch short names pointing at this side.
    pub refs: Vec<String>,
}

/// Everything the renderer needs to present a conflict, labels included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictContext {
    /// Operation that produced the conflict.
    pub operation: ConflictOperation,
    /// Branch HEAD points at, when it points at one.
    pub current_branch: Option<String>,
    /// Side the result is being built from (stage 2 side).
    pub source_a: ConflictSource,
    /// Side being merged, replayed, cherry-picked or reverted (stage 3 side).
    pub source_b: ConflictSource,
}

impl ConflictContext {
    /// Composes the operation-aware labels for both sides.
    ///
    /// `current` is the HEAD side; `incoming` is the commit being merged,
    /// replayed, cherry-picked or reverted.
    pub fn new(
        operation: ConflictOperation,
        current_branch: Option<String>,
        current: CommitIdentity,
        incoming: CommitIdentity,
    ) -> Self {
        let incoming_named = describe_named(&incoming);
        let incoming_commit = describe_commit(&incoming);
        let merge_current = match &current_branch {
            Some(branch) => format!("CURRENT · {branch}"),
            None => "CURRENT".to_owned(),
        };
        let (label_a, label_b) = match operation {
            ConflictOperation::Merge => (merge_current, format!("INCOMING · {incoming_named}")),
            ConflictOperation::Rebase => (
                "REBASED RESULT".to_owned(),
                format!("COMMIT BEING REPLAYED · {incoming_commit}"),
            ),
            ConflictOperation::CherryPick => (
                "CURRENT RESULT".to_owned(),
                format!("CHERRY-PICKED COMMIT · {incoming_commit}"),
            ),
            ConflictOperation::Revert => (
                "CURRENT RESULT".to_owned(),
                format!("REVERTED VERSION · from reverting {incoming_commit}"),
            ),
            ConflictOperation::Unknown => ("VERSION A".to_owned(), "VERSION B".to_owned()),
        };
        Self {
            operation,
            current_branch,
            source_a: ConflictSource {
                label: label_a,
                oid: current.oid,
                subject: current.subject,
                refs: current.refs,
            },
            source_b: ConflictSource {
                label: label_b,
                oid: incoming.oid,
                subject: incoming.subject,
                refs: incoming.refs,
            },
        }
    }

    /// Header line describing the operation, or `None` when unknown (§33).
    pub fn description(&self) -> Option<String> {
        match self.operation {
            ConflictOperation::Merge => {
                let incoming = describe_named(&CommitIdentity {
                    oid: self.source_b.oid.clone(),
                    subject: self.source_b.subject.clone(),
                    refs: self.source_b.refs.clone(),
                });
                match &self.current_branch {
                    Some(branch) => Some(format!("Merge {incoming} into {branch}")),
                    None => Some(format!("Merge {incoming}")),
                }
            }
            ConflictOperation::Rebase => Some(format!("Rebase of {}", self.source_b_commit())),
            ConflictOperation::CherryPick => {
                Some(format!("Cherry-pick of {}", self.source_b_commit()))
            }
            ConflictOperation::Revert => Some(format!("Revert of {}", self.source_b_commit())),
            ConflictOperation::Unknown => None,
        }
    }

    /// Action label keeping only the first alternative (§23).
    pub fn use_a_action(&self) -> &'static str {
        match self.operation {
            ConflictOperation::Merge => "Use current",
            ConflictOperation::Rebase => "Use rebased result",
            ConflictOperation::CherryPick => "Use current result",
            ConflictOperation::Revert => "Use current result",
            ConflictOperation::Unknown => "Use version A",
        }
    }

    /// Action label keeping only the second alternative (§23).
    pub fn use_b_action(&self) -> &'static str {
        match self.operation {
            ConflictOperation::Merge => "Use incoming",
            ConflictOperation::Rebase => "Use replayed commit",
            ConflictOperation::CherryPick => "Use cherry-picked commit",
            ConflictOperation::Revert => "Use reverted version",
            ConflictOperation::Unknown => "Use version B",
        }
    }

    /// Action label keeping both alternatives, A first (§24).
    pub fn a_then_b_action(&self) -> &'static str {
        match self.operation {
            ConflictOperation::Merge => "Current then incoming",
            ConflictOperation::Rebase => "Rebased result then replayed commit",
            ConflictOperation::CherryPick => "Current result then cherry-picked commit",
            ConflictOperation::Revert => "Current result then reverted version",
            ConflictOperation::Unknown => "Version A then version B",
        }
    }

    /// Action label keeping both alternatives, B first (§24).
    pub fn b_then_a_action(&self) -> &'static str {
        match self.operation {
            ConflictOperation::Merge => "Incoming then current",
            ConflictOperation::Rebase => "Replayed commit then rebased result",
            ConflictOperation::CherryPick => "Cherry-picked commit then current result",
            ConflictOperation::Revert => "Reverted version then current result",
            ConflictOperation::Unknown => "Version B then version A",
        }
    }

    /// Commit-style description of the second side (`a81c932 "subject"`).
    fn source_b_commit(&self) -> String {
        describe_commit(&CommitIdentity {
            oid: self.source_b.oid.clone(),
            subject: self.source_b.subject.clone(),
            refs: self.source_b.refs.clone(),
        })
    }
}

/// Describes a commit by branch name when unambiguous, else by OID/subject.
///
/// Used for merge sides, where the branch names are the natural labels. Never
/// picks a name when several branches point at the commit (§5).
fn describe_named(identity: &CommitIdentity) -> String {
    if let [single] = identity.refs.as_slice() {
        return single.clone();
    }
    describe_commit(identity)
}

/// Describes a commit by short OID and subject (§6-8).
fn describe_commit(identity: &CommitIdentity) -> String {
    let short = identity.oid.as_deref().map(short_oid);
    match (short, &identity.subject) {
        (Some(short), Some(subject)) => format!("{short} \"{subject}\""),
        (Some(short), None) => short.to_owned(),
        (None, Some(subject)) => subject.clone(),
        (None, None) => match identity.refs.as_slice() {
            [single] => single.clone(),
            _ => "unknown commit".to_owned(),
        },
    }
}

/// First seven characters of an OID.
fn short_oid(oid: &str) -> &str {
    let end = oid.len().min(7);
    &oid[..end]
}
