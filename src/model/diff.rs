//! Unified diff model.
//!
//! The model keeps the raw Git output for every structural part (file header,
//! extended headers, hunk header, hunk lines) so that a patch containing a
//! single hunk can be rebuilt byte for byte and fed back to `git apply`. This
//! is why contents are raw bytes instead of `String`: diffs can legitimately
//! contain non-UTF-8 text, and a lossy conversion would corrupt any rebuilt
//! patch. Use the `*_text()` accessors for display, where lossy output is fine.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// A complete diff, as produced by `git diff` or `git show`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// One entry per changed file, in Git output order.
    pub files: Vec<FileDiff>,
}

impl Diff {
    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Diff of a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Raw `diff --git a/... b/...` line.
    pub header: Vec<u8>,
    /// Raw extended header lines between `diff --git` and the first hunk, in
    /// order and verbatim (`index`, `old mode`, `---`, `+++`, rename headers,
    /// binary markers, ...).
    pub metadata: Vec<Vec<u8>>,
    /// Path before the change; `None` for added files.
    pub old_path: Option<PathBuf>,
    /// Path after the change; `None` for deleted files.
    pub new_path: Option<PathBuf>,
    /// Kind of change.
    pub status: FileStatus,
    /// True for binary files, which have no textual hunks.
    pub binary: bool,
    /// Hunks of the file, in Git output order.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Path to display for this file: the new one, falling back to the old one.
    pub fn path(&self) -> Option<&Path> {
        self.new_path.as_deref().or(self.old_path.as_deref())
    }

    /// Display form of the `diff --git` line.
    pub fn header_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.header)
    }
}

/// Kind of change of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Content changed.
    Modified,
    /// Whole file added.
    Added,
    /// Whole file deleted.
    Deleted,
    /// File renamed (possibly with content changes).
    Renamed,
    /// File copied (possibly with content changes).
    Copied,
    /// Only the mode changed, with no content hunks.
    ModeChanged,
}

/// A single hunk of a file diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// First line of the old side.
    pub old_start: usize,
    /// Number of lines of the old side.
    pub old_count: usize,
    /// First line of the new side.
    pub new_start: usize,
    /// Number of lines of the new side.
    pub new_count: usize,
    /// Raw `@@ ... @@ ...` header line.
    pub header: Vec<u8>,
    /// Hunk lines, including `No newline` markers.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// Display form of the hunk header.
    pub fn header_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.header)
    }
}

/// A single line inside a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Kind of line.
    pub kind: DiffLineKind,
    /// Raw line content without its leading marker byte (`+`, `-`, ` ` or `\`).
    pub content: Vec<u8>,
}

impl DiffLine {
    /// Display form of the line content.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.content)
    }
}

/// Kind of line inside a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Unchanged line (` `).
    Context,
    /// Added line (`+`).
    Addition,
    /// Removed line (`-`).
    Deletion,
    /// `\ No newline at end of file` marker, which follows a line that has no
    /// trailing newline.
    NoNewlineMarker,
}
