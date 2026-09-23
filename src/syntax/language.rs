//! Language detection from file paths (COLORS.md sections 6 and 47).
//!
//! GtkSourceView does the work through its own language definitions
//! (extensions, globs, special filenames): there is no hand-written mapping
//! here. Detection is display-only and its failure is not an error.

use std::path::Path;

use sourceview5::{Language, LanguageManager};

/// The global language manager: GtkSourceView loads its language definitions
/// once and shares them for the whole application (COLORS.md section 49).
fn manager() -> LanguageManager {
    LanguageManager::default()
}

/// Detects the syntax highlighting language of `path`, or `None` when
/// GtkSourceView does not know it (plain text, COLORS.md sections 6 and 7).
///
/// The path only decides the language: its existence or content are never
/// consulted, so the same call works for working tree, staged and historical
/// diffs (COLORS.md sections 8 and 14).
pub fn detect_language(path: &Path) -> Option<Language> {
    let language = manager().guess_language(Some(path), None::<&str>);
    match &language {
        Some(language) => log::debug!(
            "syntax language detected: {} for {}",
            language.id(),
            path.display()
        ),
        None => log::debug!("syntax language not detected for: {}", path.display()),
    }
    language
}
