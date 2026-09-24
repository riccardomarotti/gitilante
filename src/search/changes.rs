//! Changes provider: matches on the added/removed lines of the diffs already
//! loaded (GITILANTE_SEARCH_SPEC.md sections 9-11 and 53).
//!
//! The provider is pure and works on the `Diff` values already in the UI state:
//! it never re-runs `git diff` (section 53) and answers while the user types.

use crate::model::diff::{Diff, DiffLineKind};
use crate::search::SEARCH_RESULTS_PER_PROVIDER;
use crate::search::matcher::find_matches_auto;
use crate::search::query::{ChangeFilter, SearchQuery};
use crate::search::result::{ChangeSearchResult, MatchRange, SearchResult};
use crate::ui::DiffSide;

/// Searches the changed lines of `worktree` (Unstaged) and `staged` (Staged).
///
/// Only Addition/Deletion lines are searched, never context lines
/// (GITILANTE_SEARCH_SPEC.md section 9).
pub fn search(query: &SearchQuery, worktree: &Diff, staged: &Diff) -> Vec<SearchResult> {
    let mut results = Vec::new();
    collect(query, worktree, DiffSide::Unstaged, &mut results);
    collect(query, staged, DiffSide::Staged, &mut results);
    results
}

fn collect(query: &SearchQuery, diff: &Diff, side: DiffSide, results: &mut Vec<SearchResult>) {
    if results.len() >= SEARCH_RESULTS_PER_PROVIDER {
        return;
    }
    for file in &diff.files {
        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            for (line_index, line) in hunk.lines.iter().enumerate() {
                if results.len() >= SEARCH_RESULTS_PER_PROVIDER {
                    return;
                }
                if !wanted(line.kind, query.change_filter) {
                    continue;
                }
                let snippet = line.text().into_owned();
                let ranges: Vec<MatchRange> = find_matches_auto(&snippet, query)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(start, end)| MatchRange { start, end })
                    .collect();
                if ranges.is_empty() {
                    continue;
                }
                results.push(SearchResult::Change(ChangeSearchResult {
                    path: file
                        .path()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_default(),
                    side,
                    hunk_index,
                    line_index,
                    kind: line.kind,
                    snippet,
                    match_ranges: ranges,
                }));
            }
        }
    }
}

/// Applies the Added/Removed filter (GITILANTE_SEARCH_SPEC.md section 10).
fn wanted(kind: DiffLineKind, filter: ChangeFilter) -> bool {
    matches!(
        (kind, filter),
        (
            DiffLineKind::Addition,
            ChangeFilter::AddedAndRemoved | ChangeFilter::AddedOnly
        ) | (
            DiffLineKind::Deletion,
            ChangeFilter::AddedAndRemoved | ChangeFilter::RemovedOnly
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::diff::{DiffLine, FileDiff, FileStatus, Hunk};
    use std::path::PathBuf;

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            content: text.as_bytes().to_vec(),
            intraline: Vec::new(),
        }
    }

    fn file(path: &str, lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            header: format!("diff --git a/{path} b/{path}").into_bytes(),
            metadata: Vec::new(),
            old_path: Some(PathBuf::from(path)),
            new_path: Some(PathBuf::from(path)),
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 2,
                header: b"@@ -1,2 +1,2 @@".to_vec(),
                lines,
            }],
        }
    }

    fn sample_diff() -> Diff {
        // GITILANTE_SEARCH_SPEC.md section 59:
        //   context timeout / -old_timeout = 10 / +new_timeout = 30
        Diff {
            files: vec![file(
                "src/config/timeout.rs",
                vec![
                    line(DiffLineKind::Context, "context timeout\n"),
                    line(DiffLineKind::Deletion, "old_timeout = 10\n"),
                    line(DiffLineKind::Addition, "new_timeout = 30\n"),
                ],
            )],
        }
    }

    fn query(text: &str, filter: ChangeFilter) -> SearchQuery {
        SearchQuery {
            text: text.to_owned(),
            change_filter: filter,
            ..SearchQuery::default()
        }
    }

    #[test]
    fn finds_only_changed_lines() {
        // GITILANTE_SEARCH_SPEC.md sections 59-60.
        let results = search(
            &query("timeout", ChangeFilter::AddedAndRemoved),
            &sample_diff(),
            &sample_diff(),
        );
        assert_eq!(results.len(), 4, "two matches per diff: {results:#?}");
        for result in &results {
            let SearchResult::Change(change) = result else {
                panic!("expected a change result");
            };
            assert_ne!(change.kind, DiffLineKind::Context);
        }

        let results = search(
            &query("timeout", ChangeFilter::AddedOnly),
            &sample_diff(),
            &sample_diff(),
        );
        assert_eq!(results.len(), 2);
        for result in &results {
            let SearchResult::Change(change) = result else {
                panic!("expected a change result");
            };
            assert_eq!(change.kind, DiffLineKind::Addition);
            assert_eq!(change.snippet, "new_timeout = 30\n");
        }

        let results = search(
            &query("timeout", ChangeFilter::RemovedOnly),
            &sample_diff(),
            &sample_diff(),
        );
        assert_eq!(results.len(), 2);
        for result in &results {
            let SearchResult::Change(change) = result else {
                panic!("expected a change result");
            };
            assert_eq!(change.kind, DiffLineKind::Deletion);
        }
    }

    #[test]
    fn context_lines_are_never_returned() {
        // The context line contains "timeout" but must not match (section 9).
        let results = search(
            &query("context", ChangeFilter::AddedAndRemoved),
            &sample_diff(),
            &sample_diff(),
        );
        assert!(results.is_empty());
    }

    #[test]
    fn staged_and_unstaged_are_distinguished() {
        // GITILANTE_SEARCH_SPEC.md section 61.
        let results = search(
            &query("new_timeout", ChangeFilter::AddedAndRemoved),
            &sample_diff(),
            &Diff::default(),
        );
        assert_eq!(results.len(), 1);
        let SearchResult::Change(change) = &results[0] else {
            panic!("expected a change result");
        };
        assert_eq!(change.side, DiffSide::Unstaged);
        assert_eq!(change.hunk_index, 0);
        assert_eq!(change.line_index, 2);
        assert_eq!(change.match_ranges, vec![MatchRange { start: 0, end: 11 }]);
    }

    #[test]
    fn unicode_offsets_are_character_based() {
        // GITILANTE_SEARCH_SPEC.md sections 40 and 69.
        let diff = Diff {
            files: vec![file(
                "città.txt",
                vec![line(DiffLineKind::Addition, "città città\n")],
            )],
        };
        let results = search(&query("città", ChangeFilter::AddedAndRemoved), &diff, &diff);
        let SearchResult::Change(change) = &results[0] else {
            panic!("expected a change result");
        };
        assert_eq!(
            change.match_ranges,
            vec![
                MatchRange { start: 0, end: 5 },
                MatchRange { start: 6, end: 11 }
            ]
        );
    }
}
