//! Syntax highlighting support (COLORS.md).
//!
//! Purely visual: it never alters the text, the diffs or the Git operations.
//! Every failure degrades silently to plain text (COLORS.md sections 2 and 7).

pub mod highlight;
pub mod language;

pub use highlight::{Highlighter, Segment};
pub use language::detect_language;

use sourceview5::{StyleScheme, StyleSchemeManager};

/// The style scheme matching the current light/dark appearance (COLORS.md
/// sections 17 and 32).
pub fn style_scheme() -> Option<StyleScheme> {
    let dark = libadwaita::StyleManager::default().is_dark();
    StyleSchemeManager::default().scheme(if dark { "Adwaita-dark" } else { "Adwaita" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::prelude::*;
    use std::path::Path;

    /// GtkSourceView is bound to the thread that initialized GTK and
    /// `cargo test` runs tests in parallel on different threads, so every
    /// GtkSourceView check lives in this single test. In headless
    /// environments run the suite under `xvfb-run` (COLORS.md section 24).
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
    fn syntax_support() {
        ensure_gtk();

        // --- language detection (COLORS.md section 24) ---
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
        // Special filenames only when GtkSourceView supports them (section 6).
        let _ = detected("Makefile");
        let _ = detected("Dockerfile");
        // Unknown files: plain text, no error (section 24).
        assert_eq!(detected("foo.unknownextension123"), None);
        // The path decides, not the file system (section 14).
        assert_eq!(detected("does/not/exist/main.rs").as_deref(), Some("rust"));

        // --- syntax style extraction (COLORS.md sections 11, 12 and 28) ---
        let highlighter = Highlighter::new();
        let language = detect_language(Path::new("f.rs"));
        let scheme = style_scheme();
        let lines: Vec<String> = ["let x = 1;", "/* comment", "still comment */"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        let styled = highlighter.highlight(language.as_ref(), scheme.as_ref(), &lines);

        // The keyword is styled differently from the plain text around it.
        let first = &styled[0];
        assert!(!first[0].tags.is_empty(), "{first:?}");
        assert_ne!(
            first[0].tags, first[1].tags,
            "keyword must differ from plain text"
        );

        // Multi-line constructs keep their state across lines: every style of
        // the comment line continues on the closing line (section 12).
        let plain_tags = &first[1].tags;
        assert!(
            styled[1][0]
                .tags
                .iter()
                .any(|tag| !plain_tags.contains(tag)),
            "the comment line must be styled"
        );
        for tag in &styled[1][0].tags {
            if !plain_tags.contains(tag) {
                assert!(
                    styled[2].iter().any(|segment| segment.tags.contains(tag)),
                    "comment style must span lines"
                );
            }
        }

        // The extracted tags belong to the shared table and can style a
        // visible buffer: this is the renderer composition (section 28).
        let visible = gtk4::TextBuffer::new(Some(highlighter.table()));
        visible.insert(&mut visible.start_iter(), "let x = 1;");
        visible.apply_tag(
            &first[0].tags[0],
            &visible.start_iter(),
            &visible.end_iter(),
        );

        // Without a language everything is plain (section 7).
        let plain = highlighter.highlight(None, scheme.as_ref(), &lines);
        assert!(plain.iter().all(|segments| segments.is_empty()));
    }
}
