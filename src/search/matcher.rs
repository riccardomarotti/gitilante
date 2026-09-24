//! Shared text matching for every search scope
//! (GITILANTE_SEARCH_SPEC.md sections 8 and 40).
//!
//! The local search and the providers use the same matcher, so the semantics
//! never drift apart. Offsets are always character based: GTK text iterators
//! and the result models speak characters, never bytes.

/// Smart case: sensitive only when the query contains an uppercase character
/// (GITILANTE_SEARCH_SPEC.md section 8).
pub fn is_case_sensitive(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// Compares two characters under the requested case mode (shared by the
/// providers, GITILANTE_SEARCH_SPEC.md section 8).
pub fn chars_match(needle: char, haystack: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        needle == haystack
    } else {
        needle.to_lowercase().eq(haystack.to_lowercase())
    }
}

fn fold(text: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        text.to_owned()
    } else {
        text.to_lowercase()
    }
}

/// Whole-text equality under the requested case mode.
pub fn text_equals(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive) == fold(needle, case_sensitive)
}

/// Prefix match under the requested case mode.
pub fn text_starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive).starts_with(&fold(needle, case_sensitive))
}

/// Substring match under the requested case mode.
pub fn text_contains(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive).contains(&fold(needle, case_sensitive))
}

/// Finds the non-overlapping matches of `query` as character ranges.
pub fn find_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        if needle.iter().enumerate().all(|(offset, &needle_char)| {
            chars_match(needle_char, haystack[index + offset], case_sensitive)
        }) {
            matches.push((index, index + needle.len()));
            index += needle.len();
        } else {
            index += 1;
        }
    }
    matches
}

/// Validates a query pattern before the providers run
/// (GITILANTE_SEARCH_SPEC.md section 38).
pub fn validate(query: &crate::search::query::SearchQuery) -> Result<(), String> {
    if query.regex {
        build_regex(&query.text, query.is_case_sensitive()).map(|_| ())
    } else {
        Ok(())
    }
}

/// Finds matches of the query, fixed-string or regular expression
/// (GITILANTE_SEARCH_SPEC.md section 49).
pub fn find_matches_auto(
    text: &str,
    query: &crate::search::query::SearchQuery,
) -> Result<Vec<(usize, usize)>, String> {
    if query.regex {
        find_matches_regex(text, &query.text, query.is_case_sensitive())
    } else {
        Ok(find_matches(text, &query.text, query.is_case_sensitive()))
    }
}

/// Regex matching with character offsets: the regex crate speaks bytes, GTK
/// speaks characters (GITILANTE_SEARCH_SPEC.md section 40).
pub fn find_matches_regex(
    text: &str,
    pattern: &str,
    case_sensitive: bool,
) -> Result<Vec<(usize, usize)>, String> {
    let regex = build_regex(pattern, case_sensitive)?;
    Ok(regex
        .find_iter(text)
        .filter(|found| found.start() < found.end())
        .map(|found| {
            (
                text[..found.start()].chars().count(),
                text[..found.end()].chars().count(),
            )
        })
        .collect())
}

fn build_regex(pattern: &str, case_sensitive: bool) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_matches_keep_character_offsets() {
        // GITILANTE_SEARCH_SPEC.md sections 40 and 49.
        let found = find_matches_regex("città 42, café 7", r"\d+", true).unwrap();
        assert_eq!(found, vec![(6, 8), (15, 16)]);
        let found = find_matches_regex("😀 value = 42", r"value = \d+", true).unwrap();
        assert_eq!(found, vec![(2, 12)]);
    }

    #[test]
    fn invalid_regex_reports_an_error() {
        assert!(find_matches_regex("text", "[invalid(", true).is_err());
    }

    #[test]
    fn regex_respects_the_case_mode() {
        let found = find_matches_regex("Value value", r"value", false).unwrap();
        assert_eq!(found, vec![(0, 5), (6, 11)]);
        let found = find_matches_regex("Value value", r"value", true).unwrap();
        assert_eq!(found, vec![(6, 11)]);
    }

    #[test]
    fn finds_plain_matches() {
        assert_eq!(
            find_matches("ab abc ab", "ab", true),
            vec![(0, 2), (3, 5), (7, 9)]
        );
        assert!(find_matches("abc", "abcd", true).is_empty());
        assert!(find_matches("abc", "", true).is_empty());
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_matches("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn smart_case_follows_the_query() {
        assert!(!is_case_sensitive("timeout"));
        assert!(is_case_sensitive("Timeout"));
        assert_eq!(
            find_matches("Timeout timeout TIMEOUT", "timeout", false),
            vec![(0, 7), (8, 15), (16, 23)]
        );
        assert_eq!(
            find_matches("Timeout timeout", "Timeout", true),
            vec![(0, 7)]
        );
    }

    #[test]
    fn offsets_are_character_based() {
        // GITILANTE_SEARCH_SPEC.md section 40: città, café, 日本語, 😀.
        assert_eq!(
            find_matches("città città", "città", true),
            vec![(0, 5), (6, 11)]
        );
        assert_eq!(
            find_matches("café café", "café", true),
            vec![(0, 4), (5, 9)]
        );
        assert_eq!(
            find_matches("日本語のテキスト", "日本語", true),
            vec![(0, 3)]
        );
        assert_eq!(find_matches("😀😀", "😀", true), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn insensitive_matching_spans_lines() {
        let text = "first line\nsecond line\nfirst again";
        assert_eq!(find_matches(text, "first", false), vec![(0, 5), (23, 28)]);
    }
}
