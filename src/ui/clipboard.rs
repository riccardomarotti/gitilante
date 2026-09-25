//! Text-only clipboard actions and lightweight context menus.
//! Invalid UTF-8 is rejected before touching the clipboard.

use std::cell::Cell;
use std::path::{Path, PathBuf};

thread_local! {
    static OPEN_MENUS: Cell<usize> = const { Cell::new(0) };
}

/// Popup focus changes must not rebuild the rows hosting an open menu.
pub fn menu_is_open() -> bool {
    OPEN_MENUS.with(|count| count.get() > 0)
}

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation, Popover};

use crate::model::commit::Commit;
use crate::model::diff::{FileDiff, Hunk};

pub enum CopyAction {
    Path(PathBuf),
    AbsolutePath(PathBuf),
    FileDiff(FileDiff),
    HunkDiff(FileDiff, Hunk),
    Text(String),
    CommitSha(Commit),
    CommitInfo(Commit),
}

/// Constructs the exact text before attempting any clipboard write.
pub fn payload(action: CopyAction, root: &Path) -> Result<(String, &'static str), &'static str> {
    match action {
        CopyAction::Path(path) => Ok((path_text(&path)?.to_owned(), "Path copied")),
        CopyAction::AbsolutePath(path) => {
            if path.is_absolute() {
                return Err("Path cannot be copied as text");
            }
            Ok((
                path_text(&root.join(path))?.to_owned(),
                "Absolute path copied",
            ))
        }
        CopyAction::FileDiff(file) => Ok((
            diff_text(crate::git::patch::file_patch(&file))?,
            "Diff copied",
        )),
        CopyAction::HunkDiff(file, hunk) => Ok((
            diff_text(crate::git::patch::single_hunk_patch(&file, &hunk))?,
            "Hunk diff copied",
        )),
        CopyAction::Text(text) => Ok((text, "Text copied")),
        CopyAction::CommitSha(commit) => Ok((commit.oid, "Commit SHA copied")),
        CopyAction::CommitInfo(commit) => Ok((commit_info(&commit), "Commit info copied")),
    }
}

pub fn copy(action: CopyAction, root: &Path) -> Result<&'static str, &'static str> {
    let (text, feedback) = payload(action, root)?;
    set_text(&text)?;
    Ok(feedback)
}

pub fn path_text(path: &Path) -> Result<&str, &'static str> {
    path.to_str().ok_or("Path cannot be copied as text")
}

pub fn diff_text(bytes: Vec<u8>) -> Result<String, &'static str> {
    String::from_utf8(bytes).map_err(|_| "Diff cannot be copied as text")
}

pub fn commit_info(commit: &Commit) -> String {
    let author = if commit.author_email.is_empty() {
        commit.author_name.clone()
    } else {
        format!("{} <{}>", commit.author_name, commit.author_email)
    };
    format!(
        "Commit: {}\nSubject: {}\nAuthor: {}",
        commit.oid, commit.subject, author
    )
}

pub fn set_text(text: &str) -> Result<(), &'static str> {
    let display = gdk::Display::default().ok_or("Clipboard unavailable")?;
    display.clipboard().set_text(text);
    Ok(())
}

/// Removes direct popover children before a row is removed from a ListBox.
pub fn detach_context_menu(widget: &impl IsA<gtk4::Widget>) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        if current.is::<Popover>() {
            current.unparent();
        }
    }
}

/// Attaches a popup to an individual row/header, without selecting it.
/// Each action owns the data of that row; the current selection is irrelevant.
pub type MenuAction = (&'static str, Box<dyn Fn()>);

/// Adds a menu action that copies the current text selection, if one exists.
pub fn copy_selection_action(
    buffer: &gtk4::TextBuffer,
    copy: impl Fn(String) + 'static,
) -> MenuAction {
    let buffer = buffer.clone();
    (
        "Copy selected text",
        Box::new(move || {
            if let Some((start, end)) = buffer.selection_bounds() {
                copy(buffer.text(&start, &end, true).to_string());
            }
        }),
    )
}

/// GDK event coordinates are relative to the window surface; the popover's
/// pointing rectangle must instead be relative to its anchor widget.
fn point_at_event(popover: &Popover, anchor: &gtk4::Widget, event: &gdk::Event) {
    let Some((x, y)) = event.position() else {
        return;
    };
    let Some(root) = anchor.root().and_then(|root| {
        root.upcast::<gtk4::glib::Object>()
            .downcast::<gtk4::Widget>()
            .ok()
    }) else {
        return;
    };
    let point = gtk4::graphene::Point::new(x as f32, y as f32);
    let Some(local) = root.compute_point(anchor, &point) else {
        return;
    };
    popover.set_pointing_to(Some(&gdk::Rectangle::new(
        local.x() as i32,
        local.y() as i32,
        1,
        1,
    )));
}

pub fn context_menu(widget: &impl IsA<gtk4::Widget>, items: Vec<MenuAction>) {
    let popover = Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_parent(widget);
    popover.connect_map(|_| {
        OPEN_MENUS.with(|count| count.set(count.get() + 1));
        log::debug!("Copy menu mapped");
    });
    popover.connect_unmap(|_| {
        OPEN_MENUS.with(|count| count.set(count.get().saturating_sub(1)));
        log::debug!("Copy menu unmapped");
    });
    let buttons = GtkBox::new(Orientation::Vertical, 2);
    buttons.set_margin_top(4);
    buttons.set_margin_bottom(4);
    buttons.set_margin_start(4);
    buttons.set_margin_end(4);
    for (label, action) in items {
        let button = Button::with_label(label);
        button.add_css_class("flat");
        let weak = popover.downgrade();
        button.connect_clicked(move |_| {
            if let Some(popover) = weak.upgrade() {
                popover.popdown();
            }
            action();
        });
        buttons.append(&button);
    }
    popover.set_child(Some(&buttons));

    let mouse = gtk4::EventControllerLegacy::new();
    mouse.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let popup = popover.downgrade();
    mouse.connect_event(move |controller, event| {
        let Some(button) = event.downcast_ref::<gdk::ButtonEvent>() else {
            return gtk4::glib::Propagation::Proceed;
        };
        if button.button() != gdk::BUTTON_SECONDARY {
            return gtk4::glib::Propagation::Proceed;
        }
        // Stop ListBox selection/navigation on press; open after release so
        // the same release cannot dismiss the new popup.
        if event.event_type() == gdk::EventType::ButtonRelease {
            if let Some(popup) = popup.upgrade() {
                if let Some(anchor) = controller.widget() {
                    point_at_event(&popup, &anchor, event);
                }
                popup.popup();
            }
        }
        gtk4::glib::Propagation::Stop
    });
    widget.add_controller(mouse);

    let popup = popover.downgrade();
    widget.connect_destroy(move |_| {
        if let Some(popup) = popup.upgrade() {
            popup.unparent();
        }
    });

    let keys = gtk4::EventControllerKey::new();
    let popup = popover.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && modifiers.contains(gdk::ModifierType::SHIFT_MASK))
        {
            if let Some(popup) = popup.upgrade() {
                popup.popup();
            }
            gtk4::glib::Propagation::Stop
        } else {
            gtk4::glib::Propagation::Proceed
        }
    });
    widget.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn path_requires_exact_utf8() {
        for path in ["with space.rs", "città.rs", "-leading.rs", "tab\tname.rs"] {
            assert_eq!(path_text(Path::new(path)), Ok(path));
        }
        let invalid = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"bad-\xff".to_vec()));
        assert!(path_text(&invalid).is_err());
        assert!(payload(CopyAction::AbsolutePath(invalid), Path::new("/repo")).is_err());
    }

    #[test]
    fn diff_requires_exact_utf8() {
        assert_eq!(diff_text(b"+valid\n".to_vec()), Ok("+valid\n".to_owned()));
        assert!(diff_text(b"+bad\xff\n".to_vec()).is_err());
        assert_eq!(
            payload(CopyAction::Text("selected text".into()), Path::new("/repo")),
            Ok(("selected text".into(), "Text copied"))
        );
    }

    #[test]
    fn payload_uses_current_path_and_rejects_invalid_diff() {
        for (raw, expected) in [
            (b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n".as_slice(), "a"),
            (b"diff --git a/a b/b\nrename from a\nrename to b\n", "b"),
            (b"diff --git a/a b/a\ndeleted file mode 100644\n--- a/a\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\n", "a"),
            (b"diff --git a/a b/b\nnew file mode 100644\n--- /dev/null\n+++ b/b\n@@ -0,0 +1 @@\n+b\n", "b"),
        ] {
            let file = crate::git::diff::parse(raw).unwrap().files.remove(0);
            let path = file.path().unwrap().to_path_buf();
            assert_eq!(payload(CopyAction::Path(path.clone()), Path::new("/repo")).unwrap().0, expected);
            assert_eq!(payload(CopyAction::AbsolutePath(path), Path::new("/repo")).unwrap(), (format!("/repo/{expected}"), "Absolute path copied"));
            assert_eq!(payload(CopyAction::FileDiff(file), Path::new("/repo")).unwrap().0.as_bytes(), raw);
        }
        let file = crate::git::diff::parse(
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+bad\xff\n",
        )
        .unwrap()
        .files
        .remove(0);
        assert!(matches!(
            payload(CopyAction::FileDiff(file), Path::new("/repo")),
            Err("Diff cannot be copied as text")
        ));
    }

    #[test]
    fn formats_full_commit_info() {
        let mut commit = Commit {
            oid: "abcdef0123456789".into(),
            parents: Vec::new(),
            author_name: "Alice".into(),
            author_email: "alice@example.com".into(),
            author_time: 0,
            subject: "Fix search".into(),
        };
        assert_eq!(
            commit_info(&commit),
            "Commit: abcdef0123456789\nSubject: Fix search\nAuthor: Alice <alice@example.com>"
        );
        assert_eq!(
            payload(CopyAction::CommitSha(commit.clone()), Path::new("/repo"))
                .unwrap()
                .0,
            commit.oid
        );
        commit.author_email.clear();
        commit.subject.clear();
        assert_eq!(
            commit_info(&commit),
            "Commit: abcdef0123456789\nSubject: \nAuthor: Alice"
        );
    }
}
