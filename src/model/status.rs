//! Repository status model.
//!
//! Mirrors what `git status --porcelain=v2` reports: a branch summary plus one
//! entry per path, where tracked entries carry separate index ("staged") and
//! worktree ("unstaged") states.

use std::path::PathBuf;

/// Overall state of a repository working tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// State of HEAD (and its upstream, if any).
    pub branch: BranchInfo,
    /// One entry per changed path, in the order reported by Git.
    pub entries: Vec<StatusEntry>,
}

impl Status {
    /// Entries with at least one change staged in the index.
    pub fn staged_entries(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|entry| entry.is_staged())
    }

    /// Entries with at least one unstaged change in the working tree.
    pub fn unstaged_entries(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|entry| entry.is_unstaged())
    }

    /// Untracked entries.
    pub fn untracked_entries(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|entry| entry.is_untracked())
    }
}

/// Summary of HEAD as reported by `git status --branch`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchInfo {
    /// Commit OID HEAD points to; `None` on an unborn branch (`(initial)`).
    pub oid: Option<String>,
    /// Branch HEAD points to.
    pub head: Head,
    /// Upstream configured for the current branch, if any.
    pub upstream: Option<String>,
    /// Commits ahead of the upstream, if an upstream is configured.
    pub ahead: Option<u64>,
    /// Commits behind the upstream, if an upstream is configured.
    pub behind: Option<u64>,
}

/// What HEAD points to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Head {
    /// HEAD does not point at a named branch and is not simply detached.
    #[default]
    Unknown,
    /// HEAD is detached from any branch.
    Detached,
    /// HEAD points at the given branch (which may be unborn).
    Branch(String),
}

/// A single path reported by `git status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// Original path of a renamed or copied entry.
    pub orig_path: Option<PathBuf>,
    /// Kind of status record this entry came from.
    pub kind: StatusEntryKind,
}

impl StatusEntry {
    /// True for untracked files.
    pub fn is_untracked(&self) -> bool {
        matches!(self.kind, StatusEntryKind::Untracked)
    }

    /// True for ignored files.
    pub fn is_ignored(&self) -> bool {
        matches!(self.kind, StatusEntryKind::Ignored)
    }

    /// True for unmerged (conflicted) paths.
    pub fn is_unmerged(&self) -> bool {
        matches!(self.kind, StatusEntryKind::Unmerged)
    }

    /// Change staged in the index, if any.
    pub fn staged_change(&self) -> Option<ChangeKind> {
        match self.kind {
            StatusEntryKind::Tracked { index, .. } if index != ChangeKind::Unmodified => {
                Some(index)
            }
            _ => None,
        }
    }

    /// Change in the working tree not staged in the index, if any.
    pub fn unstaged_change(&self) -> Option<ChangeKind> {
        match self.kind {
            StatusEntryKind::Tracked { worktree, .. } if worktree != ChangeKind::Unmodified => {
                Some(worktree)
            }
            _ => None,
        }
    }

    /// True if the entry has staged changes.
    pub fn is_staged(&self) -> bool {
        self.staged_change().is_some()
    }

    /// True if the entry has unstaged changes.
    pub fn is_unstaged(&self) -> bool {
        self.unstaged_change().is_some()
    }
}

/// Kind of status record behind an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusEntryKind {
    /// Tracked path with separate index and worktree states (`1`/`2` records).
    Tracked {
        index: ChangeKind,
        worktree: ChangeKind,
    },
    /// Unmerged path (`u` record).
    Unmerged,
    /// Untracked path (`?` record).
    Untracked,
    /// Ignored path (`!` record).
    Ignored,
}

/// Single-letter change kind of a tracked path, as reported by Git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// No change in this stage.
    Unmodified,
    /// Content modified.
    Modified,
    /// File type changed (e.g. symlink replaced by a regular file).
    TypeChanged,
    /// Added to this stage.
    Added,
    /// Deleted from this stage.
    Deleted,
    /// Renamed in this stage.
    Renamed,
    /// Copied in this stage.
    Copied,
}

impl ChangeKind {
    /// Parses a single `X`/`Y` byte from a porcelain status field.
    pub fn from_byte(byte: u8) -> Result<Self, String> {
        match byte {
            b'.' => Ok(Self::Unmodified),
            b'M' => Ok(Self::Modified),
            b'T' => Ok(Self::TypeChanged),
            b'A' => Ok(Self::Added),
            b'D' => Ok(Self::Deleted),
            b'R' => Ok(Self::Renamed),
            b'C' => Ok(Self::Copied),
            other => Err(format!("unknown change kind: {:?}", other as char)),
        }
    }
}
