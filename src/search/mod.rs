//! Repository-wide search: scopes, providers and models
//! (GITILANTE_SEARCH_SPEC.md sections 6-7).
//!
//! Every scope is served by an independent, testable provider. The UI never
//! runs Git on the main thread: providers receive their inputs already loaded
//! or run through the worker (GITILANTE_SEARCH_SPEC.md section 52).

pub mod changes;
pub mod contents;
pub mod files;
pub mod matcher;
pub mod query;
pub mod result;

pub use query::{CaseMode, ChangeFilter, SearchQuery, SearchScope};
pub use result::{
    ChangeSearchResult, CommitSearchResult, ContentSearchResult, FileSearchResult,
    HistoryChangeSearchResult, MatchRange, SearchResult,
};

/// Maximum results kept per provider (GITILANTE_SEARCH_SPEC.md section 32).
pub const SEARCH_RESULTS_PER_PROVIDER: usize = 100;
