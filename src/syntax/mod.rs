//! Syntax highlighting support (COLORS.md).
//!
//! Purely visual: it never alters the text, the diffs or the Git operations.
//! Every failure degrades silently to plain text (COLORS.md sections 2 and 7).

pub mod language;

pub use language::detect_language;
