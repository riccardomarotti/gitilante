//! History provider: commit metadata search
//! (GITILANTE_SEARCH_SPEC.md sections 23-26).
//!
//! One query runs four facets (section 24): object name prefixes and ref names
//! resolve through Git and the ref map, commit messages through
//! `git log --grep`, authors through `git log --author`. Results are ranked by
//! facet (section 25) and deduplicated keeping the best rank.

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::git::history;
use crate::git::repository::Repository;
use crate::model::commit::Commit;
use crate::model::refs::CommitRef;
use crate::search::SEARCH_RESULTS_PER_PROVIDER;
use crate::search::matcher::{text_contains, text_equals, text_starts_with};
use crate::search::query::SearchQuery;
use crate::search::result::{CommitSearchResult, SearchResult};

/// Ranking tiers (GITILANTE_SEARCH_SPEC.md section 25).
const TIER_REVISION: usize = 0;
const TIER_REF: usize = 1;
const TIER_SUBJECT: usize = 2;
const TIER_AUTHOR: usize = 3;

/// Searches commit metadata and ref names.
pub fn search(
    query: &SearchQuery,
    repo: &Repository,
    refs: &HashMap<String, Vec<CommitRef>>,
) -> Vec<SearchResult> {
    if query.text.is_empty() {
        return Vec::new();
    }
    let case_sensitive = query.is_case_sensitive();
    let mut hits: HashMap<String, (usize, Option<Commit>)> = HashMap::new();
    let mut resolved: Vec<String> = Vec::new();

    // An object name prefix or an exact ref name resolves through Git.
    if let Ok(Some(oid)) = history::resolve_commit(repo, &query.text) {
        let tier = if is_object_name(&query.text) {
            TIER_REVISION
        } else {
            TIER_REF
        };
        insert(&mut hits, oid.clone(), tier, None);
        resolved.push(oid);
    }

    // Ref names: exact, prefix and substring matches (section 23).
    for (oid, names) in refs {
        for commit_ref in names {
            let matches = text_equals(&commit_ref.name, &query.text, case_sensitive)
                || text_starts_with(&commit_ref.name, &query.text, case_sensitive)
                || text_contains(&commit_ref.name, &query.text, case_sensitive);
            if matches {
                insert(&mut hits, oid.clone(), TIER_REF, None);
                resolved.push(oid.clone());
            }
        }
    }

    // Commit messages through Git (sections 23-24).
    let mut filters = vec![
        "--fixed-strings".to_owned(),
        format!("--grep={}", query.text),
    ];
    if !case_sensitive {
        filters.push("--regexp-ignore-case".to_owned());
    }
    if let Ok(commits) = history::log_filtered(repo, &filters, SEARCH_RESULTS_PER_PROVIDER) {
        for commit in commits {
            insert(&mut hits, commit.oid.clone(), TIER_SUBJECT, Some(commit));
        }
    }

    // Authors through Git (section 23).
    let mut filters = vec![format!("--author={}", regex_escape(&query.text))];
    if !case_sensitive {
        filters.push("--regexp-ignore-case".to_owned());
    }
    if let Ok(commits) = history::log_filtered(repo, &filters, SEARCH_RESULTS_PER_PROVIDER) {
        for commit in commits {
            insert(&mut hits, commit.oid.clone(), TIER_AUTHOR, Some(commit));
        }
    }

    // Fill in the metadata of the resolved revisions (section 26).
    let missing: Vec<String> = resolved
        .iter()
        .filter(|oid| hits.get(*oid).is_some_and(|entry| entry.1.is_none()))
        .cloned()
        .collect();
    if let Ok(commits) = history::commits_by_oid(repo, &missing) {
        for commit in commits {
            if let Some(entry) = hits.get_mut(&commit.oid) {
                entry.1 = Some(commit);
            }
        }
    }

    // Rank by facet, then keep the normal History order (section 25).
    let mut ranked: Vec<(usize, Reverse<i64>, String, Commit)> = hits
        .into_iter()
        .filter_map(|(oid, (tier, commit))| {
            commit.map(|commit| (tier, Reverse(commit.author_time), oid, commit))
        })
        .collect();
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    ranked
        .into_iter()
        .take(SEARCH_RESULTS_PER_PROVIDER)
        .map(|(_, _, oid, commit)| {
            let names = refs
                .get(&oid)
                .map(|names| {
                    names
                        .iter()
                        .map(|commit_ref| commit_ref.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            SearchResult::Commit(CommitSearchResult {
                author: format!(
                    "{} · {}",
                    commit.author_name,
                    crate::ui::history::relative_time(commit.author_time)
                ),
                refs: names,
                oid: commit.oid,
                subject: commit.subject,
            })
        })
        .collect()
}

/// Keeps the best rank for a commit.
fn insert(
    hits: &mut HashMap<String, (usize, Option<Commit>)>,
    oid: String,
    tier: usize,
    commit: Option<Commit>,
) {
    let entry = hits.entry(oid).or_insert((tier, None));
    entry.0 = entry.0.min(tier);
    if entry.1.is_none() {
        entry.1 = commit;
    }
}

/// True for hexadecimal queries: abbreviated or full object names.
fn is_object_name(text: &str) -> bool {
    (4..=40).contains(&text.len()) && text.chars().all(|char_| char_.is_ascii_hexdigit())
}

/// Escapes a literal for the regex-based `--author` filter (section 49: the
/// query is a fixed string unless asked otherwise).
fn regex_escape(text: &str) -> String {
    let mut escaped = String::new();
    for char_ in text.chars() {
        if "\\^$.|?*+()[]{}".contains(char_) {
            escaped.push('\\');
        }
        escaped.push(char_);
    }
    escaped
}
