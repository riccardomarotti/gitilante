//! Files provider: fuzzy path matching
//! (GITILANTE_SEARCH_SPEC.md sections 13-15).
//!
//! The query is split on whitespace and every token must match the path
//! ("hist rs"). Results are ranked by the best tier per token: exact basename,
//! basename prefix, basename substring, basename fuzzy, whole path
//! (section 14). The ranking is small and deterministic.

use std::path::Path;

use crate::search::SEARCH_RESULTS_PER_PROVIDER;
use crate::search::matcher::chars_match;
use crate::search::query::SearchQuery;
use crate::search::result::{FileSearchResult, SearchResult};

/// Fuzzy file search over the paths of the working tree.
pub fn search(query: &SearchQuery, files: &[impl AsRef<Path>]) -> Vec<SearchResult> {
    let tokens: Vec<&str> = query.text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let case_sensitive = query.is_case_sensitive();

    let mut scored: Vec<(usize, &Path)> = Vec::new();
    for file in files {
        let path = file.as_ref();
        if let Some(tier) = path_tier(path, &tokens, case_sensitive) {
            scored.push((tier, path));
        }
    }
    // Deterministic: worst token tier first, then the path itself.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    scored
        .into_iter()
        .take(SEARCH_RESULTS_PER_PROVIDER)
        .map(|(_, path)| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            SearchResult::File(FileSearchResult {
                path: path.to_path_buf(),
                name,
            })
        })
        .collect()
}

/// Tier of a path: the worst tier among the tokens, `None` when a token does
/// not match at all (GITILANTE_SEARCH_SPEC.md section 14).
fn path_tier(path: &Path, tokens: &[&str], case_sensitive: bool) -> Option<usize> {
    let full = path.to_string_lossy();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut worst = 0;
    for token in tokens {
        let tier = token_tier(&name, &full, token, case_sensitive)?;
        worst = worst.max(tier);
    }
    Some(worst)
}

fn token_tier(name: &str, full: &str, token: &str, case_sensitive: bool) -> Option<usize> {
    if equals(name, token, case_sensitive) {
        return Some(0);
    }
    if starts_with(name, token, case_sensitive) {
        return Some(1);
    }
    if substring(name, token, case_sensitive) {
        return Some(2);
    }
    if fuzzy(name, token, case_sensitive) {
        return Some(3);
    }
    if substring(full, token, case_sensitive) {
        return Some(4);
    }
    None
}

fn fold(text: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        text.to_owned()
    } else {
        text.to_lowercase()
    }
}

fn equals(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive) == fold(needle, case_sensitive)
}

fn starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive).starts_with(&fold(needle, case_sensitive))
}

fn substring(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive).contains(&fold(needle, case_sensitive))
}

/// Subsequence match: every needle character appears in order (section 14).
fn fuzzy(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|needle_char| {
        chars
            .by_ref()
            .any(|hay_char| chars_match(needle_char, hay_char, case_sensitive))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn files(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.to_owned(),
            ..SearchQuery::default()
        }
    }

    fn names(results: &[SearchResult]) -> Vec<&str> {
        results
            .iter()
            .map(|result| match result {
                SearchResult::File(file) => file.path.to_str().unwrap(),
                _ => panic!("expected a file result"),
            })
            .collect()
    }

    #[test]
    fn fuzzy_query_prefers_basename_matches() {
        // GITILANTE_SEARCH_SPEC.md section 62.
        let all = files(&[
            "src/git/history.rs",
            "src/ui/history.rs",
            "tests/history.rs",
            "src/git/status.rs",
        ]);
        let results = search(&query("hist rs"), &all);
        assert_eq!(
            names(&results),
            vec![
                "src/git/history.rs",
                "src/ui/history.rs",
                "tests/history.rs",
            ]
        );
    }

    #[test]
    fn ranking_prefers_basename_tiers() {
        // Exact basename first, then prefix, substring and path-only matches.
        let all = files(&["src/history.rs", "src/hist/main.c", "tests/history.rs"]);
        let results = search(&query("hist"), &all);
        assert_eq!(
            names(&results),
            vec!["src/history.rs", "tests/history.rs", "src/hist/main.c"]
        );
    }

    #[test]
    fn smart_case_applies_to_files_too() {
        let all = files(&["src/History.rs", "src/history.rs"]);
        // Lowercase query: smart case is insensitive, both match.
        assert_eq!(
            names(&search(&query("history"), &all)),
            vec!["src/History.rs", "src/history.rs"]
        );
        assert_eq!(
            names(&search(&query("History"), &all)),
            vec!["src/History.rs"]
        );
    }

    #[test]
    fn empty_query_returns_nothing() {
        assert!(search(&query(""), &files(&["src/app.rs"])).is_empty());
        assert!(search(&query("   "), &files(&["src/app.rs"])).is_empty());
    }

    #[test]
    fn every_token_must_match() {
        let all = files(&["src/git/history.rs", "src/main.rs"]);
        assert!(search(&query("hist zz"), &all).is_empty());
    }
}
