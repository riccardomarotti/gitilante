//! GTK user interface.

pub mod app;
pub mod changes;
pub mod diff_view;
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
}

impl Selection {
    /// Path of the selected entry.
    pub fn path(&self) -> &Path {
        match self {
            Self::Staged(path) | Self::Unstaged(path) | Self::Untracked(path) => path,
        }
    }
}

/// Side of the repository a diff is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    /// The index (`git diff --cached`).
    Staged,
    /// The working tree (`git diff`).
    Unstaged,
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
