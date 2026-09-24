//! Diff folding state: stable keys and collapse state, entirely outside the
//! Git model (GITILANTE_DIFF_FOLDING_SPEC.md sections 11-16).
//!
//! `Diff`, `FileDiff` and `Hunk` stay pure Git data: what is collapsed lives
//! here, keyed by stable fingerprints (paths for files, header and line
//! ranges for hunks) so a refresh keeps the state best-effort and never
//! points at the wrong element (sections 12-14).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::model::diff::{Diff, FileDiff, Hunk};
use crate::ui::Selection;

/// Stable identity of a file (GITILANTE_DIFF_FOLDING_SPEC.md section 12).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileFoldKey {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
}

impl FileFoldKey {
    /// The key of a file diff: works for added, deleted and renamed files.
    pub fn from_file(file: &FileDiff) -> Self {
        Self {
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
        }
    }
}

/// Stable identity of a hunk (GITILANTE_DIFF_FOLDING_SPEC.md section 13):
/// never a bare index, which changes when the diff shifts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HunkFoldKey {
    pub file: FileFoldKey,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
}

impl HunkFoldKey {
    /// The key of a hunk of `file`.
    pub fn from_hunk(file: &FileDiff, hunk: &Hunk) -> Self {
        Self {
            file: FileFoldKey::from_file(file),
            old_start: hunk.old_start,
            old_count: hunk.old_count,
            new_start: hunk.new_start,
            new_count: hunk.new_count,
            header: hunk.header_text().into_owned(),
        }
    }
}

/// What is collapsed in one rendered diff (GITILANTE_DIFF_FOLDING_SPEC.md
/// section 44). File and hunk states are independent and collapsing a parent
/// never touches its children (section 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffFoldState {
    collapsed_files: HashSet<FileFoldKey>,
    collapsed_hunks: HashSet<HunkFoldKey>,
}

impl DiffFoldState {
    pub fn is_file_collapsed(&self, key: &FileFoldKey) -> bool {
        self.collapsed_files.contains(key)
    }

    pub fn is_hunk_collapsed(&self, key: &HunkFoldKey) -> bool {
        self.collapsed_hunks.contains(key)
    }

    /// Expands a collapsed element and collapses an expanded one.
    pub fn toggle_file(&mut self, key: FileFoldKey) {
        if !self.collapsed_files.remove(&key) {
            self.collapsed_files.insert(key);
        }
    }

    pub fn toggle_hunk(&mut self, key: HunkFoldKey) {
        if !self.collapsed_hunks.remove(&key) {
            self.collapsed_hunks.insert(key);
        }
    }

    /// Collapses every file of `diff`, leaving the hunk states untouched
    /// (GITILANTE_DIFF_FOLDING_SPEC.md section 22).
    pub fn collapse_all_files(&mut self, diff: &Diff) {
        for file in &diff.files {
            self.collapsed_files.insert(FileFoldKey::from_file(file));
        }
    }

    pub fn expand_all_files(&mut self, diff: &Diff) {
        for file in &diff.files {
            self.collapsed_files.remove(&FileFoldKey::from_file(file));
        }
    }

    pub fn collapse_all_hunks(&mut self, diff: &Diff) {
        for file in &diff.files {
            for hunk in &file.hunks {
                self.collapsed_hunks
                    .insert(HunkFoldKey::from_hunk(file, hunk));
            }
        }
    }

    pub fn expand_all_hunks(&mut self, diff: &Diff) {
        for file in &diff.files {
            for hunk in &file.hunks {
                self.collapsed_hunks
                    .remove(&HunkFoldKey::from_hunk(file, hunk));
            }
        }
    }

    /// Reveals a file and a hunk for the search navigation
    /// (GITILANTE_DIFF_FOLDING_SPEC.md sections 30-31).
    pub fn reveal_hunk(&mut self, file: &FileDiff, hunk: &Hunk) {
        self.collapsed_files.remove(&FileFoldKey::from_file(file));
        self.collapsed_hunks
            .remove(&HunkFoldKey::from_hunk(file, hunk));
    }

    /// Forgets the keys that no longer exist in `diff`
    /// (GITILANTE_DIFF_FOLDING_SPEC.md section 46).
    pub fn prune(&mut self, diff: &Diff) {
        let files: HashSet<FileFoldKey> = diff.files.iter().map(FileFoldKey::from_file).collect();
        let hunks: HashSet<HunkFoldKey> = diff
            .files
            .iter()
            .flat_map(|file| {
                file.hunks
                    .iter()
                    .map(move |hunk| HunkFoldKey::from_hunk(file, hunk))
            })
            .collect();
        self.collapsed_files.retain(|key| files.contains(key));
        self.collapsed_hunks.retain(|key| hunks.contains(key));
    }
}

/// Which rendered document a fold state belongs to
/// (GITILANTE_DIFF_FOLDING_SPEC.md section 15).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffDocumentKey {
    Unstaged(PathBuf),
    Staged(PathBuf),
    Conflicted(PathBuf),
    Commit(String),
}

impl DiffDocumentKey {
    /// The document rendered for `selection`, if it can contain hunks.
    pub fn from_selection(selection: &Selection) -> Option<Self> {
        match selection {
            Selection::Unstaged(path) => Some(DiffDocumentKey::Unstaged(path.clone())),
            Selection::Staged(path) => Some(DiffDocumentKey::Staged(path.clone())),
            Selection::Conflicted(path) => Some(DiffDocumentKey::Conflicted(path.clone())),
            Selection::Commit(oid) => Some(DiffDocumentKey::Commit(oid.clone())),
            Selection::Untracked(_) | Selection::FilePreview { .. } => None,
        }
    }
}

/// The document key of a single-file diff, when it can contain hunks.
pub fn document_key_for_file(
    side: crate::ui::DiffSide,
    path: &std::path::Path,
) -> Option<DiffDocumentKey> {
    match side {
        crate::ui::DiffSide::Staged => Some(DiffDocumentKey::Staged(path.to_path_buf())),
        crate::ui::DiffSide::Unstaged => Some(DiffDocumentKey::Unstaged(path.to_path_buf())),
        crate::ui::DiffSide::Conflicted => Some(DiffDocumentKey::Conflicted(path.to_path_buf())),
        crate::ui::DiffSide::History => None,
    }
}

/// The fold states of the session, one per document
/// (GITILANTE_DIFF_FOLDING_SPEC.md sections 15-16 and 45).
#[derive(Debug, Clone, Default)]
pub struct FoldStateStore {
    documents: HashMap<DiffDocumentKey, DiffFoldState>,
}

impl FoldStateStore {
    /// The state of one document, created expanded on first use.
    pub fn for_document(&mut self, key: &DiffDocumentKey) -> &mut DiffFoldState {
        self.documents.entry(key.clone()).or_default()
    }

    /// Applies section 46: keep recognizable states, drop dead keys.
    pub fn prune(&mut self, key: &DiffDocumentKey, diff: &Diff) {
        if let Some(state) = self.documents.get_mut(key) {
            state.prune(diff);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::diff::{DiffLine, DiffLineKind, FileStatus};

    fn line(kind: DiffLineKind) -> DiffLine {
        DiffLine {
            kind,
            content: b"change\n".to_vec(),
            intraline: Vec::new(),
        }
    }

    fn file(path: &str, starts: &[usize]) -> FileDiff {
        FileDiff {
            header: format!("diff --git a/{path} b/{path}").into_bytes(),
            metadata: Vec::new(),
            old_path: Some(PathBuf::from(path)),
            new_path: Some(PathBuf::from(path)),
            status: FileStatus::Modified,
            binary: false,
            hunks: starts
                .iter()
                .map(|start| Hunk {
                    old_start: *start,
                    old_count: 2,
                    new_start: *start,
                    new_count: 3,
                    header: format!("@@ -{start},2 +{start},3 @@").into_bytes(),
                    lines: vec![line(DiffLineKind::Addition), line(DiffLineKind::Deletion)],
                })
                .collect(),
        }
    }

    fn sample() -> Diff {
        Diff {
            files: vec![file("a.rs", &[10, 40]), file("b.rs", &[5])],
        }
    }

    #[test]
    fn toggles_files_and_hunks_independently() {
        // GITILANTE_DIFF_FOLDING_SPEC.md section 47.
        let diff = sample();
        let mut state = DiffFoldState::default();
        let file_key = FileFoldKey::from_file(&diff.files[0]);
        let hunk_key = HunkFoldKey::from_hunk(&diff.files[0], &diff.files[0].hunks[0]);

        state.toggle_file(file_key.clone());
        state.toggle_hunk(hunk_key.clone());
        assert!(state.is_file_collapsed(&file_key));
        assert!(state.is_hunk_collapsed(&hunk_key));
        assert!(!state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[1])));

        state.toggle_file(file_key.clone());
        assert!(!state.is_file_collapsed(&file_key));
        assert!(
            state.is_hunk_collapsed(&hunk_key),
            "states stay independent"
        );
    }

    #[test]
    fn collapsing_a_file_preserves_the_children() {
        // GITILANTE_DIFF_FOLDING_SPEC.md sections 4 and 48.
        let diff = sample();
        let mut state = DiffFoldState::default();
        let first = HunkFoldKey::from_hunk(&diff.files[0], &diff.files[0].hunks[0]);
        let second = HunkFoldKey::from_hunk(&diff.files[0], &diff.files[0].hunks[1]);
        state.toggle_hunk(first.clone());

        let file_key = FileFoldKey::from_file(&diff.files[0]);
        state.toggle_file(file_key.clone());
        state.toggle_file(file_key);
        assert!(state.is_hunk_collapsed(&first), "hunk 1 stays collapsed");
        assert!(!state.is_hunk_collapsed(&second), "hunk 2 stays expanded");
    }

    #[test]
    fn global_actions_touch_only_their_level() {
        // GITILANTE_DIFF_FOLDING_SPEC.md section 22.
        let diff = sample();
        let mut state = DiffFoldState::default();
        let hunk = HunkFoldKey::from_hunk(&diff.files[0], &diff.files[0].hunks[0]);
        state.toggle_hunk(hunk.clone());

        state.collapse_all_files(&diff);
        assert!(state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[0])));
        assert!(state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[1])));
        assert!(state.is_hunk_collapsed(&hunk), "hunk states unchanged");

        state.collapse_all_hunks(&diff);
        assert!(
            state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[0])),
            "file states unchanged"
        );

        state.expand_all_files(&diff);
        assert!(!state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[0])));
        assert!(state.is_hunk_collapsed(&hunk), "hunk states unchanged");
    }

    #[test]
    fn keys_survive_a_refresh_and_new_hunks_default_to_expanded() {
        // GITILANTE_DIFF_FOLDING_SPEC.md section 49.
        let diff = sample();
        let mut state = DiffFoldState::default();
        let hunk = HunkFoldKey::from_hunk(&diff.files[0], &diff.files[0].hunks[1]);
        state.toggle_hunk(hunk.clone());

        // The same diff regenerated produces the same keys.
        let refreshed = sample();
        assert!(state.is_hunk_collapsed(&HunkFoldKey::from_hunk(
            &refreshed.files[0],
            &refreshed.files[0].hunks[1]
        )));

        // A changed hunk is a new key: expanded by default.
        let changed = Diff {
            files: vec![file("a.rs", &[10, 100]), file("b.rs", &[5])],
        };
        state.prune(&changed);
        assert!(!state.is_hunk_collapsed(&HunkFoldKey::from_hunk(
            &changed.files[0],
            &changed.files[0].hunks[1]
        )));
    }

    #[test]
    fn prune_forgets_dead_keys() {
        // GITILANTE_DIFF_FOLDING_SPEC.md section 46.
        let diff = sample();
        let mut state = DiffFoldState::default();
        state.collapse_all_files(&diff);
        state.collapse_all_hunks(&diff);

        let smaller = Diff {
            files: vec![file("a.rs", &[10])],
        };
        state.prune(&smaller);
        assert!(state.is_file_collapsed(&FileFoldKey::from_file(&smaller.files[0])));
        assert_eq!(state.collapsed_files.len(), 1);
        assert_eq!(state.collapsed_hunks.len(), 1);
    }

    #[test]
    fn reveal_hunk_expands_its_parents() {
        // GITILANTE_DIFF_FOLDING_SPEC.md section 51.
        let diff = sample();
        let mut state = DiffFoldState::default();
        state.collapse_all_files(&diff);
        state.collapse_all_hunks(&diff);

        state.reveal_hunk(&diff.files[0], &diff.files[0].hunks[1]);
        assert!(!state.is_file_collapsed(&FileFoldKey::from_file(&diff.files[0])));
        assert!(!state.is_hunk_collapsed(&HunkFoldKey::from_hunk(
            &diff.files[0],
            &diff.files[0].hunks[1]
        )));
        assert!(state.is_hunk_collapsed(&HunkFoldKey::from_hunk(
            &diff.files[0],
            &diff.files[0].hunks[0]
        )));
    }
}
