//! Per-document UI state for presented split hunks.
//!
//! The Git diff remains untouched; this store only remembers which original
//! hunk keys should be expanded into synthetic children during rendering.

use std::collections::{HashMap, HashSet};

use crate::model::diff::Diff;
use crate::ui::folding::{DiffDocumentKey, HunkFoldKey};

/// Split state for each displayed diff document.
#[derive(Debug, Clone, Default)]
pub struct SplitStateStore {
    documents: HashMap<DiffDocumentKey, HashSet<HunkFoldKey>>,
}

impl SplitStateStore {
    /// Whether this original Git hunk is currently presented as children.
    pub fn is_split(&self, document: &DiffDocumentKey, key: &HunkFoldKey) -> bool {
        self.documents
            .get(document)
            .is_some_and(|keys| keys.contains(key))
    }

    /// Toggles the original hunk between its Git presentation and split parts.
    pub fn toggle(&mut self, document: &DiffDocumentKey, key: HunkFoldKey) {
        let keys = self.documents.entry(document.clone()).or_default();
        if !keys.remove(&key) {
            keys.insert(key);
        }
    }

    /// Split keys for one document, or an empty set when none are split.
    pub fn keys(&self, document: &DiffDocumentKey) -> HashSet<HunkFoldKey> {
        self.documents.get(document).cloned().unwrap_or_default()
    }

    /// Drops split state whose original hunk no longer exists in the raw diff.
    pub fn prune(&mut self, document: &DiffDocumentKey, raw_diff: &Diff) {
        let live: HashSet<HunkFoldKey> = raw_diff
            .files
            .iter()
            .flat_map(|file| {
                file.hunks
                    .iter()
                    .filter(|hunk| crate::hunk_split::can_split(file, hunk))
                    .map(move |hunk| HunkFoldKey::from_hunk(file, hunk))
            })
            .collect();
        if let Some(keys) = self.documents.get_mut(document) {
            keys.retain(|key| live.contains(key));
            if keys.is_empty() {
                self.documents.remove(document);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::diff::{DiffLine, DiffLineKind, FileDiff, FileStatus, Hunk};
    use std::path::PathBuf;

    fn diff() -> Diff {
        Diff {
            files: vec![FileDiff {
                header: b"diff --git a/a.rs b/a.rs".to_vec(),
                metadata: Vec::new(),
                old_path: Some(PathBuf::from("a.rs")),
                new_path: Some(PathBuf::from("a.rs")),
                status: FileStatus::Modified,
                binary: false,
                hunks: vec![Hunk {
                    old_start: 1,
                    old_count: 3,
                    new_start: 1,
                    new_count: 3,
                    header: b"@@ -1,3 +1,3 @@".to_vec(),
                    lines: vec![
                        DiffLine {
                            kind: DiffLineKind::Deletion,
                            content: b"old one\n".to_vec(),
                            intraline: Vec::new(),
                        },
                        DiffLine {
                            kind: DiffLineKind::Addition,
                            content: b"new one\n".to_vec(),
                            intraline: Vec::new(),
                        },
                        DiffLine {
                            kind: DiffLineKind::Context,
                            content: b"context\n".to_vec(),
                            intraline: Vec::new(),
                        },
                        DiffLine {
                            kind: DiffLineKind::Deletion,
                            content: b"old two\n".to_vec(),
                            intraline: Vec::new(),
                        },
                        DiffLine {
                            kind: DiffLineKind::Addition,
                            content: b"new two\n".to_vec(),
                            intraline: Vec::new(),
                        },
                    ],
                }],
            }],
        }
    }

    #[test]
    fn split_state_is_document_scoped_toggleable_and_pruned_from_raw_diff() {
        let diff = diff();
        let file = &diff.files[0];
        let key = HunkFoldKey::from_hunk(file, &file.hunks[0]);
        let document = DiffDocumentKey::Staged(PathBuf::from("a.rs"));
        let other_document = DiffDocumentKey::Unstaged(PathBuf::from("a.rs"));
        let mut store = SplitStateStore::default();
        store.toggle(&document, key.clone());
        assert!(store.is_split(&document, &key));
        assert!(!store.is_split(&other_document, &key));
        store.prune(&document, &diff);
        assert!(store.is_split(&document, &key));
        store.toggle(&document, key.clone());
        assert!(!store.is_split(&document, &key));
        store.toggle(&document, key.clone());
        store.prune(&document, &Diff::default());
        assert!(!store.is_split(&document, &key));
    }
}
