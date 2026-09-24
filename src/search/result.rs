//! Search result model (GITILANTE_SEARCH_SPEC.md sections 11, 21, 25, 29
//! and 39).
//!
//! Every variant carries everything needed to navigate to the result: the UI
//! never re-runs a search to understand where to go (section 39).

use std::path::PathBuf;

use crate::model::diff::DiffLineKind;
use crate::ui::DiffSide;

/// A match range as character offsets into the snippet (section 40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchRange {
    pub start: usize,
    pub end: usize,
}

/// A match on an added/removed line of the diffs currently loaded (section 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSearchResult {
    /// File path, relative to the repository root.
    pub path: PathBuf,
    /// Staged or Unstaged source of the diff.
    pub side: DiffSide,
    /// Index of the hunk inside its file diff.
    pub hunk_index: usize,
    /// Index of the line inside the hunk.
    pub line_index: usize,
    /// Addition or Deletion.
    pub kind: DiffLineKind,
    /// The full text of the matched line.
    pub snippet: String,
    /// Matches as character offsets into `snippet`.
    pub match_ranges: Vec<MatchRange>,
}

/// A file path match (section 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchResult {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// File name, for the two-line result row.
    pub name: String,
}

/// A match inside a working tree file (section 21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSearchResult {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// 1-based line number.
    pub line_number: usize,
    /// Character offset of the first match in the line, when known.
    pub column: Option<usize>,
    /// The full text of the matched line.
    pub snippet: String,
    /// Matches as character offsets into `snippet`.
    pub match_ranges: Vec<MatchRange>,
}

/// A commit metadata match (section 25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSearchResult {
    /// Full object name.
    pub oid: String,
    /// First line of the commit message.
    pub subject: String,
    /// Author name and date, for the result row.
    pub author: String,
    /// Refs pointing at the commit, for the badges (section 25).
    pub refs: Vec<String>,
}

/// A commit whose diff added or removed the query (section 29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryChangeSearchResult {
    /// Full object name.
    pub oid: String,
    /// First line of the commit message.
    pub subject: String,
    /// Author name and date, for the result row.
    pub author: String,
    /// Paths involved, when cheap to know.
    pub paths: Vec<PathBuf>,
}

/// One search result (section 39).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchResult {
    Change(ChangeSearchResult),
    File(FileSearchResult),
    Content(ContentSearchResult),
    Commit(CommitSearchResult),
    HistoryChange(HistoryChangeSearchResult),
}
