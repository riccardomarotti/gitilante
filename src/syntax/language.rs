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

#[cfg(test)]
mod tests {
    use super::*;

    /// GtkSourceView requires an initialized GTK and works on its main thread
    /// (the thread that initialized GTK), so every detection check lives in a
    /// single test. In headless environments run the suite under `xvfb-run`
    /// (COLORS.md section 24).
    fn ensure_gtk() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            gtk4::init().expect("GTK initialization (run headless tests under xvfb-run)");
        });
    }

    fn detected(path: &str) -> Option<String> {
        detect_language(Path::new(path)).map(|language| language.id().to_string())
    }

    #[test]
    fn language_detection() {
        ensure_gtk();

        // Common languages (COLORS.md section 24).
        assert_eq!(detected("src/main.rs").as_deref(), Some("rust"), "main.rs");
        assert_eq!(detected("foo.py").as_deref(), Some("python3"), "foo.py");
        assert_eq!(
            detected("Cargo.toml").as_deref(),
            Some("toml"),
            "Cargo.toml"
        );
        assert_eq!(detected("foo.json").as_deref(), Some("json"), "foo.json");
        assert_eq!(
            detected("README.md").as_deref(),
            Some("markdown"),
            "README.md"
        );
        assert_eq!(detected("script.sh").as_deref(), Some("sh"), "script.sh");
        assert_eq!(detected("style.css").as_deref(), Some("css"), "style.css");
        assert_eq!(
            detected("index.html").as_deref(),
            Some("html"),
            "index.html"
        );
        assert_eq!(
            detected(".gitlab-ci.yml").as_deref(),
            Some("yaml"),
            ".gitlab-ci.yml"
        );

        // Special filenames are detected only when GtkSourceView supports
        // them (COLORS.md sections 6 and 24): any result is valid, the call
        // must simply never fail.
        let _ = detected("Makefile");
        let _ = detected("Dockerfile");

        // Unknown files: plain text, no error (COLORS.md section 24).
        assert_eq!(detected("foo.unknownextension123"), None);

        // The path decides, not the file system (COLORS.md section 14).
        assert_eq!(detected("does/not/exist/main.rs").as_deref(), Some("rust"));
    }
}
