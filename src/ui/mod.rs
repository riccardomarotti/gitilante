//! GTK user interface.

pub mod app;
pub mod changes;
pub mod diff_view;
pub mod graph_gutter;
pub mod history;
pub mod main_window;
pub mod worker;

use std::path::{Path, PathBuf};

use crate::model::status::ChangeKind;
/// What the user currently selected in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// A change staged in the index.
    Staged(PathBuf),
    /// A change in the working tree not staged in the index.
    Unstaged(PathBuf),
    /// An untracked file.
    Untracked(PathBuf),
    /// A commit of the History view (by object name).
    Commit(String),
    /// A conflicted (unmerged) file, shown read-only.
    Conflicted(PathBuf),
}

/// A hunk that has keyboard focus, with its file and side.
#[derive(Debug, Clone)]
pub struct HunkTarget {
    /// File diff the hunk belongs to.
    pub file: crate::model::diff::FileDiff,
    /// The hunk itself.
    pub hunk: crate::model::diff::Hunk,
    /// Side the diff was taken from.
    pub side: DiffSide,
}

/// Side of the repository a diff is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    /// The index (`git diff --cached`).
    Staged,
    /// The working tree (`git diff`).
    Unstaged,
    /// A historical commit (`git show`); hunks get their actions in Fase 6.
    History,
    /// A conflicted file (combined diff), read-only.
    Conflicted,
}

/// Short letter describing a change kind.
pub fn change_letter(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Unmodified => ' ',
        ChangeKind::Modified => 'M',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Added => 'A',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
    }
}

/// Display name of an entry, showing the original path of renames.
pub fn display_name(path: &Path, orig_path: Option<&Path>) -> String {
    match orig_path {
        Some(orig_path) => format!("{} (from {})", path.display(), orig_path.display()),
        None => path.display().to_string(),
    }
}

/// Path shortened with `~` for the window subtitle.
pub fn short_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rest) = path.strip_prefix(Path::new(&home)) {
            if !rest.as_os_str().is_empty() {
                return format!("~/{}", rest.display());
            }
            return "~".to_owned();
        }
    }
    path.display().to_string()
}
