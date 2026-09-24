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

    /// Unmerged (conflicted) entries.
    pub fn unmerged_entries(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|entry| entry.is_unmerged())
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
        matches!(self.kind, StatusEntryKind::Unmerged(_))
    }

    /// Full stage metadata of an unmerged path, when conflicted.
    pub fn unmerged(&self) -> Option<&UnmergedInfo> {
        match &self.kind {
            StatusEntryKind::Unmerged(info) => Some(info),
            _ => None,
        }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEntryKind {
    /// Tracked path with separate index and worktree states (`1`/`2` records).
    Tracked {
        index: ChangeKind,
        worktree: ChangeKind,
    },
    /// Unmerged path (`u` record) with its full stage metadata.
    Unmerged(UnmergedInfo),
    /// Untracked path (`?` record).
    Untracked,
    /// Ignored path (`!` record).
    Ignored,
}

/// Full metadata of an unmerged (conflicted) path.
///
/// Kept verbatim from the `u` record so the conflict solver can read the stage
/// blobs through their OIDs without running further plumbing commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmergedInfo {
    /// Conflict kind as reported by the XY field.
    pub code: UnmergedCode,
    /// Submodule state field (a conflict may involve submodule content).
    pub submodule: SubmoduleState,
    /// Stage 1: common base version.
    pub base: ConflictStage,
    /// Stage 2: "our" version (see the conflict solver spec for UI labels).
    pub stage2: ConflictStage,
    /// Stage 3: "their" version (see the conflict solver spec for UI labels).
    pub stage3: ConflictStage,
    /// Mode of the working tree file, when it exists.
    pub worktree_mode: Option<u32>,
}

/// A single index stage of an unmerged entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictStage {
    /// File mode (e.g. `0o100644`); `None` when the stage does not exist.
    pub mode: Option<u32>,
    /// Object OID; `None` when the stage does not exist (all-zero OID).
    pub oid: Option<String>,
}

impl ConflictStage {
    /// True when this stage carries no content at all.
    pub fn is_absent(&self) -> bool {
        self.mode.is_none() && self.oid.is_none()
    }
}

/// Conflict kind of an unmerged entry, from the XY field of a `u` record.
///
/// Internal names follow Git terminology on purpose; the UI translates them
/// into concrete, operation-aware actions instead of "ours"/"theirs".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmergedCode {
    /// `UU`: both sides modified the file.
    BothModified,
    /// `AA`: both sides added the file.
    BothAdded,
    /// `AU`: added by us, absent on the other side.
    AddedByUs,
    /// `UA`: added by them, absent on our side.
    AddedByThem,
    /// `DU`: deleted by us, modified by them.
    DeletedByUs,
    /// `UD`: modified by us, deleted by them.
    DeletedByThem,
    /// `DD`: deleted by both sides.
    BothDeleted,
}

impl UnmergedCode {
    /// Parses a two-character XY field.
    pub fn from_xy(xy: &[u8]) -> Result<Self, String> {
        match xy {
            b"UU" => Ok(Self::BothModified),
            b"AA" => Ok(Self::BothAdded),
            b"AU" => Ok(Self::AddedByUs),
            b"UA" => Ok(Self::AddedByThem),
            b"DU" => Ok(Self::DeletedByUs),
            b"UD" => Ok(Self::DeletedByThem),
            b"DD" => Ok(Self::BothDeleted),
            other => Err(format!(
                "unknown unmerged code: {:?}",
                String::from_utf8_lossy(other)
            )),
        }
    }

    /// The two-character XY field for this code.
    pub fn as_xy(self) -> &'static str {
        match self {
            Self::BothModified => "UU",
            Self::BothAdded => "AA",
            Self::AddedByUs => "AU",
            Self::AddedByThem => "UA",
            Self::DeletedByUs => "DU",
            Self::DeletedByThem => "UD",
            Self::BothDeleted => "DD",
        }
    }
}

/// Submodule state field of a status record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleState {
    /// `N...`: the path is not a submodule.
    NotSubmodule,
    /// `S<c><m><u>`: a submodule, with its state letters preserved verbatim.
    Submodule {
        /// Commit comparison state letter.
        commit: u8,
        /// Tracked content state letter.
        tracked: u8,
        /// Untracked content state letter.
        untracked: u8,
    },
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
