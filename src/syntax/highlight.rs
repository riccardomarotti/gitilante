//! Syntax style extraction (COLORS.md sections 2, 11 and 12).
//!
//! GtkSourceView highlights a coherent source block (an old side or a new side
//! of a hunk, never the two mixed): [`Highlighter`] runs its engine
//! synchronously and reports which tags style which part of which line. The
//! caller renders the real diff text with the very same tag objects, so the
//! syntax foreground composes with the diff backgrounds (COLORS.md sections 3
//! and 28).
//!
//! The [`Highlighter`] and its tags live as long as the renderer needs them:
//! GtkSourceView unregisters a buffer's tags when the buffer is dropped, so
//! the source buffer is created once and reused for every block.

use gtk4::prelude::*;
use gtk4::{TextTag, TextTagTable};
use sourceview5::{Buffer as SourceBuffer, Language, StyleScheme, prelude::*};

/// Maximum number of characters analyzed before falling back to plain text
/// (COLORS.md section 21).
pub const MAX_HIGHLIGHT_CHARS: usize = 64_000;

/// A run of characters inside a source line sharing the same syntax tags.
#[derive(Debug)]
pub struct Segment {
    /// First character of the run within its line (Unicode scalar values).
    pub start: usize,
    /// Character after the last one of the run.
    pub end: usize,
    /// Syntax tags styling the run.
    pub tags: Vec<TextTag>,
}

/// Syntax highlighter bound to the GTK main thread.
///
/// Reuses one hidden source buffer and one tag table for the whole
/// application (COLORS.md section 49).
pub struct Highlighter {
    table: TextTagTable,
    source: SourceBuffer,
}

impl Highlighter {
    /// Creates the shared highlighting resources (main thread only).
    pub fn new() -> Self {
        let table = TextTagTable::new();
        let source = SourceBuffer::new(Some(&table));
        source.set_highlight_syntax(true);
        Self { table, source }
    }

    /// The shared tag table: create the visible buffers from it so the syntax
    /// tags can be applied to their text.
    pub fn table(&self) -> &TextTagTable {
        &self.table
    }

    /// Highlights `lines` as one coherent source block and returns the styled
    /// segments of every line, in order.
    ///
    /// Input without a language, or over [`MAX_HIGHLIGHT_CHARS`], comes back
    /// untagged: the plain text fallback (COLORS.md sections 7 and 21).
    pub fn highlight(
        &self,
        language: Option<&Language>,
        scheme: Option<&StyleScheme>,
        lines: &[String],
    ) -> Vec<Vec<Segment>> {
        let mut styled: Vec<Vec<Segment>> = lines.iter().map(|_| Vec::new()).collect();
        if language.is_none()
            || lines.iter().map(|line| line.chars().count()).sum::<usize>() > MAX_HIGHLIGHT_CHARS
        {
            return styled;
        }

        self.source.set_style_scheme(scheme);
        self.source.set_language(language);
        let mut text = String::new();
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        self.source.set_text(&text);

        // Synchronous: the segments below are complete when this returns.
        let (start, end) = self.source.bounds();
        self.source.ensure_highlight(&start, &end);

        for (index, line) in lines.iter().enumerate() {
            let segments = &mut styled[index];
            let Some(line_start) = self.source.iter_at_line(index as i32) else {
                continue;
            };
            let chars = line.chars().count();
            let mut offset = 0;
            while offset < chars {
                let tags = tags_at(&line_start, offset);
                let mut end = offset;
                while end + 1 < chars && tags_at(&line_start, end + 1) == tags {
                    end += 1;
                }
                segments.push(Segment {
                    start: offset,
                    end: end + 1,
                    tags,
                });
                offset = end + 1;
            }
        }
        styled
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Tags active at `offset` characters into the line starting at `line_start`.
fn tags_at(line_start: &gtk4::TextIter, offset: usize) -> Vec<TextTag> {
    let mut iter = *line_start;
    iter.forward_chars(offset as i32);
    iter.tags()
}
