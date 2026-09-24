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

#[cfg(test)]
mod tests {
    use super::*;

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
