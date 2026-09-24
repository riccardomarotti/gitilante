//! Commit refs and HEAD model (BRANCH.md sections 9-11).

/// Kind of ref pointing at a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// A local branch under `refs/heads`.
    LocalBranch,
    /// A remote branch under `refs/remotes` (symbolic remote HEADs excluded).
    RemoteBranch,
}

/// A ref pointing at a commit, shown as a badge in the History
/// (BRANCH.md section 39).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommitRef {
    /// Display name: `main` for a local branch, `origin/main` for a remote one.
    pub name: String,
    /// Ref kind (a future `Tag` must not break the graph, BRANCH.md section 13).
    pub kind: RefKind,
}

/// The checked out HEAD (BRANCH.md section 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadRef {
    /// The commit HEAD points at.
    pub oid: String,
    /// The checked out branch name; `None` when HEAD is detached.
    pub branch: Option<String>,
}
