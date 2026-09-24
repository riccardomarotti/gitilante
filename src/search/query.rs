//! Search query model (GITILANTE_SEARCH_SPEC.md sections 7-10).

/// The scope of a search: one provider each, `All` grouping the normal ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    /// Every normal provider except History Changes (section 31).
    #[default]
    All,
    /// The added/removed lines of the diffs currently loaded (section 9).
    Changes,
    /// File paths of the working tree (section 13).
    Files,
    /// Working tree file contents (section 17).
    Contents,
    /// Commit metadata (section 23).
    History,
    /// Commits that added or removed the query (section 27).
    HistoryChanges,
}

/// How the query case is handled (GITILANTE_SEARCH_SPEC.md section 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMode {
    /// Sensitive only when the query contains an uppercase character.
    #[default]
    Smart,
    Sensitive,
    Insensitive,
}

/// What changed lines the Changes scope looks at
/// (GITILANTE_SEARCH_SPEC.md section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeFilter {
    #[default]
    AddedAndRemoved,
    AddedOnly,
    RemovedOnly,
}

/// One search request (GITILANTE_SEARCH_SPEC.md section 7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    /// The text to find. Empty runs no expensive provider (section 34).
    pub text: String,
    /// What to search.
    pub scope: SearchScope,
    /// How to treat the query case.
    pub case_mode: CaseMode,
    /// True when `text` is a regular expression (section 49).
    pub regex: bool,
    /// Added/removed filter of the Changes scope.
    pub change_filter: ChangeFilter,
}

impl SearchQuery {
    /// True when the query carries no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// True when matching must respect the case (section 8).
    pub fn is_case_sensitive(&self) -> bool {
        match self.case_mode {
            CaseMode::Smart => crate::search::matcher::is_case_sensitive(&self.text),
            CaseMode::Sensitive => true,
            CaseMode::Insensitive => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_case_follows_the_query() {
        let query = SearchQuery {
            text: "timeout".to_owned(),
            ..SearchQuery::default()
        };
        assert!(!query.is_case_sensitive());
        let query = SearchQuery {
            text: "Timeout".to_owned(),
            ..SearchQuery::default()
        };
        assert!(query.is_case_sensitive());
    }

    #[test]
    fn case_mode_overrides_smart_case() {
        let query = SearchQuery {
            text: "Timeout".to_owned(),
            case_mode: CaseMode::Insensitive,
            ..SearchQuery::default()
        };
        assert!(!query.is_case_sensitive());
    }
}
