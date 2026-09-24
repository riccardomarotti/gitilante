//! History Changes provider: commits that added or removed a string
//! (GITILANTE_SEARCH_SPEC.md sections 27-30).
//!
//! The two semantics stay distinct (section 27): fixed-string queries use
//! `git log -S` (the pickaxe: commits where the number of occurrences
//! changes), regular expressions use `git log -G` (commits whose diffs match).
//! This scope never runs as part of `All` (section 28).

use crate::git::history;
use crate::git::repository::Repository;
use crate::search::SEARCH_RESULTS_PER_PROVIDER;
use crate::search::query::SearchQuery;
use crate::search::result::{HistoryChangeSearchResult, SearchResult};

/// Minimum query length for this expensive scope
/// (GITILANTE_SEARCH_SPEC.md section 34).
const MIN_QUERY_CHARS: usize = 2;

/// Searches the history for commits that added or removed the query.
///
/// Returns the provider error (an invalid regular expression, for example) as
/// a message for the search UI (GITILANTE_SEARCH_SPEC.md section 38).
pub fn search(query: &SearchQuery, repo: &Repository) -> Result<Vec<SearchResult>, String> {
    if query.text.chars().count() < MIN_QUERY_CHARS {
        return Ok(Vec::new());
    }

    // Section 27: -S for the fixed string, -G for the regular expression.
    let pickaxe = if query.regex {
        format!("-G{}", query.text)
    } else {
        format!("-S{}", query.text)
    };
    let mut filters = vec![pickaxe];
    if !query.is_case_sensitive() {
        filters.push("--regexp-ignore-case".to_owned());
    }

    let commits = history::log_filtered(repo, &filters, SEARCH_RESULTS_PER_PROVIDER)
        .map_err(|error| error.to_string())?;
    Ok(commits
        .into_iter()
        .map(|commit| {
            SearchResult::HistoryChange(HistoryChangeSearchResult {
                author: format!(
                    "{} · {}",
                    commit.author_name,
                    crate::ui::history::relative_time(commit.author_time)
                ),
                oid: commit.oid,
                subject: commit.subject,
                // Paths are optional in this scope (section 29).
                paths: Vec::new(),
            })
        })
        .collect())
}
