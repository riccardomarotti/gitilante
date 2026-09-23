//! Intraline (inline) diff analysis.
//!
//! Finds which parts of a pair of corresponding lines actually changed, so
//! the renderer can highlight them more intensely inside the normal
//! removed/added line colors.
//!
//! This analysis is purely visual (DIFF.md sections 2 and 13): Git remains the
//! source of truth for line-level diffs, and nothing here may influence patch
//! reconstruction, hunk boundaries or Git operations. The original line always
//! stays available as-is.
//!
//! The algorithm is deliberately language agnostic (DIFF.md section 45):
//!
//! 1. each line is split into tokens: runs of alphanumeric graphemes, with
//!    every other grapheme (whitespace, punctuation, operators, `_`, `.`, ...)
//!    as a token of its own (DIFF.md section 18);
//! 2. the two token sequences are aligned with a longest-common-subsequence
//!    pass, which keeps the line order stable (DIFF.md section 7);
//! 3. deleted and inserted tokens that meet form a change block, and are
//!    paired positionally inside it;
//! 4. a pair is refined to character level only when the two tokens are mostly
//!    the same (a common affix of at least three quarters); otherwise the
//!    whole tokens are highlighted, which is less noisy and more honest
//!    (DIFF.md sections 17 and 38);
//! 5. unpaired tokens are highlighted entirely.
//!
//! When a line or a block is too large (DIFF.md section 34) the analysis gives
//! up and returns no spans: the plain line diff remains correct.

use unicode_segmentation::UnicodeSegmentation;

/// Maximum line length (in characters) analyzed for intraline changes.
pub const MAX_INTRALINE_LINE_LENGTH: usize = 4000;

/// Maximum number of tokens aligned in one line.
pub const MAX_TOKENS: usize = 512;

/// Maximum number of graphemes refined inside a changed token pair.
pub const MAX_REFINE_UNITS: usize = 256;

/// Minimum common affix (in graphemes) of a changed token pair, as
/// `common * REFINE_COMMON_DENOMINATOR >= total * REFINE_COMMON_NUMERATOR`.
const REFINE_COMMON_NUMERATOR: usize = 3;
const REFINE_COMMON_DENOMINATOR: usize = 4;

/// A changed part of a line.
///
/// Offsets are character offsets (Unicode scalar values) into the line text and
/// always fall on character boundaries, never inside a grapheme cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntralineSpan {
    /// First changed character, included.
    pub start: usize,
    /// Character after the last changed one.
    pub end: usize,
}

/// Changed parts of a pair of corresponding lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntralineSpans {
    /// Changed parts on the old (removed) side.
    pub old: Vec<IntralineSpan>,
    /// Changed parts on the new (added) side.
    pub new: Vec<IntralineSpan>,
}

/// Finds the parts of `old` and `new` that actually changed.
///
/// Returns no spans when the lines are too different to pair meaningfully is
/// not decided here (the caller pairs lines and applies similarity thresholds,
/// see DIFF.md sections 5 and 6): this function only reports what changed
/// inside two lines already considered counterparts. Very large input falls
/// back to no spans (DIFF.md section 34).
pub fn string_intraline(old: &str, new: &str) -> IntralineSpans {
    let mut spans = IntralineSpans::default();
    if old == new
        || old.chars().count() > MAX_INTRALINE_LINE_LENGTH
        || new.chars().count() > MAX_INTRALINE_LINE_LENGTH
    {
        return spans;
    }

    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    if old_tokens.is_empty()
        || new_tokens.is_empty()
        || old_tokens.len() > MAX_TOKENS
        || new_tokens.len() > MAX_TOKENS
    {
        // Pure insertion or deletion is handled by the caller at line level;
        // oversized input falls back to the plain line diff.
        return spans;
    }

    for block in change_blocks(&old_tokens, &new_tokens, |a, b| a.text == b.text) {
        let pairs = block.old.len().min(block.new.len());
        for index in 0..pairs {
            let old_token = old_tokens[block.old[index]];
            let new_token = new_tokens[block.new[index]];
            match refine_token_pair(old_token.text, new_token.text) {
                Some((old_inner, new_inner)) => {
                    spans.old.extend(
                        old_inner
                            .into_iter()
                            .map(|span| shift(span, old_token.start)),
                    );
                    spans.new.extend(
                        new_inner
                            .into_iter()
                            .map(|span| shift(span, new_token.start)),
                    );
                }
                None => {
                    spans.old.push(IntralineSpan {
                        start: old_token.start,
                        end: old_token.end,
                    });
                    spans.new.push(IntralineSpan {
                        start: new_token.start,
                        end: new_token.end,
                    });
                }
            }
        }
        // Tokens with no counterpart are new or removed entirely.
        spans.old.extend(block.old[pairs..].iter().map(|&index| {
            let token = old_tokens[index];
            IntralineSpan {
                start: token.start,
                end: token.end,
            }
        }));
        spans.new.extend(block.new[pairs..].iter().map(|&index| {
            let token = new_tokens[index];
            IntralineSpan {
                start: token.start,
                end: token.end,
            }
        }));
    }

    merge(&mut spans.old);
    merge(&mut spans.new);
    spans
}

/// A run of changed units (tokens or graphemes) between two equal ones.
#[derive(Debug, Default)]
struct ChangeBlock {
    old: Vec<usize>,
    new: Vec<usize>,
}

/// Aligns two unit sequences and returns the changed blocks in order.
///
/// Equality is given by `equals`; the alignment is a longest common
/// subsequence, so the order of the lines is preserved (DIFF.md section 7).
fn change_blocks<T, F>(old: &[T], new: &[T], equals: F) -> Vec<ChangeBlock>
where
    F: Fn(&T, &T) -> bool,
{
    let (n, m) = (old.len(), new.len());
    if n == 0 && m == 0 {
        return Vec::new();
    }

    // Longest common subsequence lengths, computed from the end.
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * (m + 1) + j] = if equals(&old[i], &new[j]) {
                lcs[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
            };
        }
    }

    let mut blocks = Vec::new();
    let mut block = ChangeBlock::default();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && equals(&old[i], &new[j]) {
            if !block.old.is_empty() || !block.new.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
            block.old.push(i);
            i += 1;
        } else {
            block.new.push(j);
            j += 1;
        }
    }
    if !block.old.is_empty() || !block.new.is_empty() {
        blocks.push(block);
    }
    blocks
}

/// A token: a word, or a single separator grapheme.
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    text: &'a str,
    /// Character offset of the token in its line.
    start: usize,
    /// Character offset just past the token.
    end: usize,
}

/// Splits a line into tokens with their character offsets.
fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = 0;
    // Character and byte offset of the word run being collected, if any.
    let mut word: Option<(usize, usize)> = None;

    for (byte, grapheme) in text.grapheme_indices(true) {
        let char_start = chars;
        chars += grapheme.chars().count();
        if grapheme.chars().all(char::is_alphanumeric) {
            word.get_or_insert((char_start, byte));
        } else {
            if let Some((start, byte_start)) = word.take() {
                tokens.push(Token {
                    text: &text[byte_start..byte],
                    start,
                    end: char_start,
                });
            }
            tokens.push(Token {
                text: grapheme,
                start: char_start,
                end: chars,
            });
        }
    }
    if let Some((start, byte_start)) = word {
        tokens.push(Token {
            text: &text[byte_start..],
            start,
            end: chars,
        });
    }
    tokens
}

/// Refines a changed token pair to character level, or `None` when the two
/// tokens are too different to refine meaningfully (DIFF.md sections 17 and 21).
fn refine_token_pair(old: &str, new: &str) -> Option<(Vec<IntralineSpan>, Vec<IntralineSpan>)> {
    let old_units: Vec<&str> = old.graphemes(true).collect();
    let new_units: Vec<&str> = new.graphemes(true).collect();
    if old_units.is_empty()
        || new_units.is_empty()
        || old_units.len() > MAX_REFINE_UNITS
        || new_units.len() > MAX_REFINE_UNITS
    {
        return None;
    }

    // Mostly-common tokens are refined; mostly-different ones are kept whole.
    let mut prefix = 0;
    while prefix < old_units.len()
        && prefix < new_units.len()
        && old_units[prefix] == new_units[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_units.len() - prefix
        && suffix < new_units.len() - prefix
        && old_units[old_units.len() - 1 - suffix] == new_units[new_units.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let common = prefix + suffix;
    let total = old_units.len().max(new_units.len());
    if common * REFINE_COMMON_DENOMINATOR < total * REFINE_COMMON_NUMERATOR {
        return None;
    }

    let old_spans = changed_unit_spans(&old_units, &new_units);
    let new_spans = changed_unit_spans(&new_units, &old_units);
    Some((old_spans, new_spans))
}

/// Spans of the units of one side that changed, as character offsets inside
/// the token.
fn changed_unit_spans(units: &[&str], other: &[&str]) -> Vec<IntralineSpan> {
    let mut spans = Vec::new();
    for block in change_blocks(units, other, |a, b| a == b) {
        // On the old side blocks list removed units first; the indexes are the
        // same kind of unit for both calls (see callers).
        let indices = &block.old;
        if indices.is_empty() {
            continue;
        }
        let start: usize = units[..indices[0]]
            .iter()
            .map(|unit| unit.chars().count())
            .sum();
        let end: usize = units[..=*indices.last().expect("non empty")]
            .iter()
            .map(|unit| unit.chars().count())
            .sum();
        spans.push(IntralineSpan { start, end });
    }
    merge(&mut spans);
    spans
}

/// Sorts spans and merges the ones that touch or overlap.
fn merge(spans: &mut Vec<IntralineSpan>) {
    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<IntralineSpan> = Vec::with_capacity(spans.len());
    for span in spans.iter().copied() {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    *spans = merged;
}

/// Moves a token-relative span by the token start.
fn shift(span: IntralineSpan, offset: usize) -> IntralineSpan {
    IntralineSpan {
        start: span.start + offset,
        end: span.end + offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changed texts of one side, for readable expectations.
    fn changed(text: &str, spans: &[IntralineSpan]) -> Vec<String> {
        spans
            .iter()
            .map(|span| {
                text.chars()
                    .skip(span.start)
                    .take(span.end - span.start)
                    .collect()
            })
            .collect()
    }

    fn analyze(old: &str, new: &str) -> (Vec<String>, Vec<String>) {
        let spans = string_intraline(old, new);
        (changed(old, &spans.old), changed(new, &spans.new))
    }

    #[test]
    fn single_number() {
        // DIFF.md section 38: the whole number tokens are the change.
        assert_eq!(
            analyze("foo(10)", "foo(20)"),
            (vec!["10".to_owned()], vec!["20".to_owned()])
        );
    }

    #[test]
    fn single_character() {
        assert_eq!(
            analyze("v1.2", "v1.3"),
            (vec!["2".to_owned()], vec!["3".to_owned()])
        );
    }

    #[test]
    fn identifier() {
        assert_eq!(
            analyze("load_cached_status()", "load_current_status()"),
            (vec!["cached".to_owned()], vec!["current".to_owned()])
        );
    }

    #[test]
    fn addition_only() {
        assert_eq!(
            analyze("run()", "run_async()"),
            (Vec::<String>::new(), vec!["_async".to_owned()])
        );
    }

    #[test]
    fn removal_only() {
        assert_eq!(
            analyze("run_async()", "run()"),
            (vec!["_async".to_owned()], Vec::<String>::new())
        );
    }

    #[test]
    fn multiple_changes() {
        assert_eq!(
            analyze("foo(false, 10)", "foo(true, 100)"),
            (
                vec!["false".to_owned(), "10".to_owned()],
                vec!["true".to_owned(), "100".to_owned()]
            )
        );
    }

    #[test]
    fn single_character_inside_a_word() {
        // DIFF.md section 17: only the removed "s" is the change.
        assert_eq!(
            analyze("get_current_users()", "get_current_user()"),
            (vec!["s".to_owned()], Vec::<String>::new())
        );
    }

    #[test]
    fn inserted_operator_character() {
        // DIFF.md section 22: only the inserted "=" is the change.
        assert_eq!(
            analyze("if retries > 3 {", "if retries >= 3 {"),
            (Vec::<String>::new(), vec!["=".to_owned()])
        );
    }

    #[test]
    fn unicode_accents() {
        // "à" is one grapheme: it must not be split or shift the highlight.
        assert_eq!(
            analyze("città", "citta"),
            (vec!["à".to_owned()], vec!["a".to_owned()])
        );
    }

    #[test]
    fn unicode_emoji_offsets() {
        // The emoji is one character but four bytes: byte offsets would break.
        assert_eq!(
            analyze("😀a", "😀b"),
            (vec!["a".to_owned()], vec!["b".to_owned()])
        );
    }

    #[test]
    fn unicode_combining_characters_are_never_split() {
        let old = "e\u{301}"; // "é" as base + combining mark, one grapheme
        let (old_changed, new_changed) = analyze(old, "e");
        assert_eq!(old_changed, vec![old.to_owned()]);
        assert_eq!(new_changed, vec!["e".to_owned()]);
    }

    #[test]
    fn unicode_word_with_accent() {
        // Only a common affix of at least three quarters is refined.
        assert_eq!(
            analyze("café", "caffe"),
            (vec!["café".to_owned()], vec!["caffe".to_owned()])
        );
    }

    #[test]
    fn changed_whitespace() {
        // DIFF.md section 41: the added space is the change.
        assert_eq!(
            analyze("foo = bar", "foo  = bar"),
            (Vec::<String>::new(), vec![" ".to_owned()])
        );
    }

    #[test]
    fn changed_indentation() {
        assert_eq!(
            analyze("    foo()", "        foo()"),
            (Vec::<String>::new(), vec!["    ".to_owned()])
        );
    }

    #[test]
    fn identical_lines_have_no_spans() {
        assert_eq!(
            analyze("same text", "same text"),
            (Vec::<String>::new(), Vec::<String>::new())
        );
    }

    #[test]
    fn huge_lines_fall_back_to_the_line_diff() {
        let old = "x".repeat(MAX_INTRALINE_LINE_LENGTH + 1);
        let new = "y".repeat(MAX_INTRALINE_LINE_LENGTH + 1);
        assert!(string_intraline(&old, &new).old.is_empty());
    }

    #[test]
    fn spans_are_ordered_and_merged() {
        let spans = string_intraline("run()", "run_async()");
        let new = &spans.new;
        assert_eq!(new.len(), 1); // "_async" merges its tokens
        assert_eq!(new[0], IntralineSpan { start: 3, end: 9 });
    }
}
