//! Contents provider: working tree file contents
//! (GITILANTE_SEARCH_SPEC.md sections 17-22).
//!
//! Tracked files go through `git grep` (section 18); untracked, non-ignored
//! files are scanned in Rust with the same matcher (section 19), skipping
//! binaries and oversized files (section 20). `Contents` means the working
//! tree: the history has its own `History Changes` scope (section 17).

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::git::repository::Repository;
use crate::search::SEARCH_RESULTS_PER_PROVIDER;
use crate::search::matcher::find_matches_auto;
use crate::search::query::SearchQuery;
use crate::search::result::{ContentSearchResult, MatchRange, SearchResult};

/// Minimum query length for this scope (GITILANTE_SEARCH_SPEC.md section 34).
const MIN_QUERY_CHARS: usize = 2;

/// Files bigger than this are never scanned (section 20).
const MAX_SEARCH_FILE_SIZE: u64 = 4 * 1024 * 1024;

/// Prefix inspected for binary detection (section 20).
const BINARY_SNIFF: usize = 8192;

/// Searches the working tree contents: tracked files with Git, untracked and
/// non-ignored files with a defensive Rust scan (sections 18-19).
pub fn search(query: &SearchQuery, repo: &Repository, untracked: &[PathBuf]) -> Vec<SearchResult> {
    if query.text.chars().count() < MIN_QUERY_CHARS {
        return Vec::new();
    }
    let case_sensitive = query.is_case_sensitive();
    let mut results = Vec::new();

    // Tracked files: Git already knows them best (section 18).
    if let Ok(found) = crate::git::grep::grep(repo, &query.text, case_sensitive, query.regex) {
        for match_line in found {
            if results.len() >= SEARCH_RESULTS_PER_PROVIDER {
                return results;
            }
            let ranges = ranges(&match_line.text, query);
            if ranges.is_empty() {
                continue;
            }
            results.push(SearchResult::Content(ContentSearchResult {
                path: match_line.path,
                line_number: match_line.line_number,
                column: match_line.column,
                snippet: match_line.text,
                match_ranges: ranges,
            }));
        }
    }

    // Untracked, non-ignored files (section 19).
    let root = repo.root();
    for path in untracked {
        if results.len() >= SEARCH_RESULTS_PER_PROVIDER {
            break;
        }
        scan_file(root, path, query, &mut results);
    }
    results
}

/// Streams one file line by line, defensively (GITILANTE_SEARCH_SPEC.md
/// sections 19-20): binaries and oversized files are skipped silently.
fn scan_file(root: &Path, path: &Path, query: &SearchQuery, results: &mut Vec<SearchResult>) {
    let full = root.join(path);
    let Ok(metadata) = std::fs::metadata(&full) else {
        return;
    };
    if metadata.len() > MAX_SEARCH_FILE_SIZE {
        return;
    }
    let Ok(mut file) = std::fs::File::open(&full) else {
        return;
    };

    // A NUL in the prefix marks a binary file (section 20).
    let mut sniff = vec![0u8; BINARY_SNIFF.min(metadata.len() as usize)];
    if !sniff.is_empty() {
        if file.read_exact(&mut sniff).is_err() {
            return;
        }
        if sniff.contains(&0) {
            return;
        }
    }
    if file.seek(SeekFrom::Start(0)).is_err() {
        return;
    }

    let mut reader = BufReader::new(file);
    let mut line_number = 0;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        line_number += 1;
        if results.len() >= SEARCH_RESULTS_PER_PROVIDER {
            return;
        }
        let text = String::from_utf8_lossy(&buffer);
        let snippet = text.trim_end_matches('\n');
        let ranges = ranges(snippet, query);
        if ranges.is_empty() {
            continue;
        }
        results.push(SearchResult::Content(ContentSearchResult {
            path: path.to_path_buf(),
            line_number,
            column: ranges.first().map(|range| range.start + 1),
            snippet: snippet.to_owned(),
            match_ranges: ranges,
        }));
    }
}

/// Match ranges of one line (always recomputed with the shared matcher so
/// every scope agrees, GITILANTE_SEARCH_SPEC.md section 8).
fn ranges(text: &str, query: &SearchQuery) -> Vec<MatchRange> {
    find_matches_auto(text, query)
        .unwrap_or_default()
        .into_iter()
        .map(|(start, end)| MatchRange { start, end })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.to_owned(),
            ..SearchQuery::default()
        }
    }

    fn contents(results: &[SearchResult]) -> Vec<(String, usize, String)> {
        results
            .iter()
            .map(|result| match result {
                SearchResult::Content(content) => (
                    content.path.display().to_string(),
                    content.line_number,
                    content.snippet.clone(),
                ),
                _ => panic!("expected a content result"),
            })
            .collect()
    }

    #[test]
    fn scans_untracked_files_with_line_numbers() {
        let dir = std::env::temp_dir().join(format!(
            "gitilante-contents-{}-{}",
            std::process::id(),
            "lines"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "nothing\nneedle here\n").unwrap();

        let mut results = Vec::new();
        scan_file(&dir, Path::new("a.txt"), &query("needle"), &mut results);
        assert_eq!(
            contents(&results),
            vec![("a.txt".to_owned(), 2, "needle here".to_owned())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_binary_files() {
        // GITILANTE_SEARCH_SPEC.md sections 20 and 65.
        let dir =
            std::env::temp_dir().join(format!("gitilante-contents-{}-bin", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blob.bin"), b"needle\x00needle").unwrap();

        let mut results = Vec::new();
        scan_file(&dir, Path::new("blob.bin"), &query("needle"), &mut results);
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_oversized_files() {
        let dir =
            std::env::temp_dir().join(format!("gitilante-contents-{}-big", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("huge.txt")).unwrap();
        file.set_len(MAX_SEARCH_FILE_SIZE + 1).unwrap();

        let mut results = Vec::new();
        scan_file(&dir, Path::new("huge.txt"), &query("needle"), &mut results);
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_queries_run_nothing() {
        // GITILANTE_SEARCH_SPEC.md section 34. CI checkouts need not contain
        // Git metadata, so this test must use its own repository.
        let dir =
            std::env::temp_dir().join(format!("gitilante-contents-{}-short", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&dir)
            .status()
            .unwrap();
        assert!(status.success());
        let repo = Repository::discover(&dir).unwrap();
        assert!(search(&query("x"), &repo, &[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unicode_offsets_are_character_based() {
        let mut results = Vec::new();
        let dir =
            std::env::temp_dir().join(format!("gitilante-contents-{}-uni", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("città.txt"), "😀 città città\n").unwrap();
        scan_file(&dir, Path::new("città.txt"), &query("città"), &mut results);
        let SearchResult::Content(content) = &results[0] else {
            panic!("expected a content result");
        };
        assert_eq!(
            content.match_ranges,
            vec![
                MatchRange { start: 2, end: 7 },
                MatchRange { start: 8, end: 13 }
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
