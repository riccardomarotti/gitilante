//! Diff renderer (SPEC section 19).
//!
//! A file diff is rendered as a header row with the whole-file actions, then
//! one block per hunk: the hunk header with its actions next to it, and the
//! hunk body in a monospace text view with line numbers. Additions and
//! deletions keep their `+`/`-` markers and get a light background tint.

use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, TextBuffer, TextView, WrapMode};

use crate::model::diff::{DiffLineKind, FileDiff, Hunk};
use crate::ui::{DiffSide, display_name};

/// User actions available from the diff view.
pub struct Callbacks {
    /// Stage one hunk of a working tree diff.
    pub stage_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Unstage one hunk of a staged diff.
    pub unstage_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Discard one hunk of a working tree diff (the caller asks for
    /// confirmation first).
    pub discard_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Revert one hunk of a commit diff into the working tree (SPEC §18).
    pub revert_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Stage every change of the given path.
    pub stage_file: Box<dyn Fn(PathBuf)>,
    /// Unstage every change of the given path.
    pub unstage_file: Box<dyn Fn(PathBuf)>,
    /// Discard the working tree changes of the given path (the caller asks for
    /// confirmation first).
    pub discard_file: Box<dyn Fn(PathBuf)>,
}

/// Renders the diff of `file` with the per-hunk and per-file actions of `side`.
pub fn render(file: &FileDiff, side: DiffSide, callbacks: &Rc<Callbacks>) -> gtk4::Widget {
    let root = GtkBox::new(Orientation::Vertical, 12);
    set_margins(&root, 12);
    root.append(&file_header(file, side, callbacks));

    if file.binary {
        // SPEC section 23: binary files have no textual hunks.
        root.append(&placeholder("Binary file changed"));
    } else if file.hunks.is_empty() {
        // Rename-only and mode-only diffs have metadata but no hunks.
        if file.metadata.is_empty() {
            root.append(&placeholder("No textual changes"));
        } else {
            root.append(&metadata_view(file));
        }
    } else {
        for hunk in &file.hunks {
            root.append(&hunk_view(file, hunk, side, callbacks));
        }
    }
    root.upcast()
}

/// Renders the placeholder shown for an untracked file (SPEC section 15).
pub fn render_untracked(path: &std::path::Path, callbacks: &Rc<Callbacks>) -> gtk4::Widget {
    let root = GtkBox::new(Orientation::Vertical, 12);
    set_margins(&root, 12);

    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name = Label::new(Some(&display_name(path, None)));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_ellipsize(pango::EllipsizeMode::Middle);
    name.set_hexpand(true);
    header.append(&name);
    let stage = file_button("Stage file", {
        let path = path.to_path_buf();
        let callbacks = callbacks.clone();
        move || (callbacks.stage_file)(path.clone())
    });
    header.append(&stage);
    root.append(&header);
    root.append(&placeholder(
        "Untracked file: stage it to work on its changes",
    ));
    root.upcast()
}

/// Renders a centered-left informational label.
pub fn placeholder(text: &str) -> gtk4::Widget {
    let label = Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_xalign(0.0);
    label.set_margin_top(4);
    label.upcast()
}

/// Header row of a file: name and whole-file actions.
fn file_header(file: &FileDiff, side: DiffSide, callbacks: &Rc<Callbacks>) -> gtk4::Widget {
    let header = GtkBox::new(Orientation::Horizontal, 8);

    let name = Label::new(Some(&display_name(
        file.path().unwrap_or_else(|| std::path::Path::new("")),
        file.old_path
            .as_deref()
            .filter(|_| file.status == crate::model::diff::FileStatus::Renamed),
    )));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_ellipsize(pango::EllipsizeMode::Middle);
    name.set_hexpand(true);
    header.append(&name);

    match side {
        DiffSide::Staged => {
            header.append(&file_button("Unstage file", {
                let path = file_path(file);
                let callbacks = callbacks.clone();
                move || (callbacks.unstage_file)(path.clone())
            }));
        }
        DiffSide::Unstaged => {
            header.append(&file_button("Stage file", {
                let path = file_path(file);
                let callbacks = callbacks.clone();
                move || (callbacks.stage_file)(path.clone())
            }));
            header.append(&file_button("Discard file", {
                let path = file_path(file);
                let callbacks = callbacks.clone();
                move || (callbacks.discard_file)(path.clone())
            }));
        }
        // Historical commits get their actions in Fase 6 (Revert hunk).
        DiffSide::History => {}
    }
    header.upcast()
}

/// One hunk block: header with actions, then the hunk body.
fn hunk_view(
    file: &FileDiff,
    hunk: &Hunk,
    side: DiffSide,
    callbacks: &Rc<Callbacks>,
) -> gtk4::Widget {
    let block = GtkBox::new(Orientation::Vertical, 4);

    let header = GtkBox::new(Orientation::Horizontal, 8);
    let header_label = Label::new(Some(&hunk.header_text()));
    header_label.add_css_class("monospace");
    header_label.add_css_class("heading");
    header_label.set_xalign(0.0);
    header_label.set_ellipsize(pango::EllipsizeMode::End);
    header_label.set_hexpand(true);
    header_label.set_selectable(true);
    header.append(&header_label);

    match side {
        DiffSide::Staged => {
            header.append(&file_button("Unstage", {
                let file = file.clone();
                let hunk = hunk.clone();
                let callbacks = callbacks.clone();
                move || (callbacks.unstage_hunk)(file.clone(), hunk.clone())
            }));
        }
        DiffSide::Unstaged => {
            header.append(&file_button("Stage", {
                let file = file.clone();
                let hunk = hunk.clone();
                let callbacks = callbacks.clone();
                move || (callbacks.stage_hunk)(file.clone(), hunk.clone())
            }));
            header.append(&file_button("Discard", {
                let file = file.clone();
                let hunk = hunk.clone();
                let callbacks = callbacks.clone();
                move || (callbacks.discard_hunk)(file.clone(), hunk.clone())
            }));
        }
        // Historical commits get their actions in Fase 6 (Revert hunk).
        DiffSide::History => {
            header.append(&file_button("Revert hunk", {
                let file = file.clone();
                let hunk = hunk.clone();
                let callbacks = callbacks.clone();
                move || (callbacks.revert_hunk)(file.clone(), hunk.clone())
            }));
        }
    }
    block.append(&header);
    block.append(&hunk_body(hunk));
    block.upcast()
}

/// Monospace text view with the hunk lines and their line numbers.
fn hunk_body(hunk: &Hunk) -> gtk4::Widget {
    let view = TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(WrapMode::None);
    view.set_left_margin(8);
    view.set_right_margin(8);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.add_css_class("frame");

    let buffer: TextBuffer = view.buffer();
    let add_tag = new_tag(&buffer, "add", addition_background());
    let del_tag = new_tag(&buffer, "del", deletion_background());
    let dim_tag = new_tag_dim(&buffer);

    let width = line_number_width(hunk);
    let mut old_num = hunk.old_start;
    let mut new_num = hunk.new_start;
    let mut iter = buffer.start_iter();

    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::NoNewlineMarker => {
                let text = format!("{} │ \\{}", " ".repeat(width), line.text().trim_start());
                buffer.insert_with_tags(&mut iter, &text, &[&dim_tag]);
            }
            kind => {
                let (marker, number) = match kind {
                    DiffLineKind::Addition => {
                        let number = new_num;
                        new_num += 1;
                        ('+', number)
                    }
                    DiffLineKind::Deletion => {
                        let number = old_num;
                        old_num += 1;
                        ('-', number)
                    }
                    _ => {
                        let number = new_num;
                        old_num += 1;
                        new_num += 1;
                        (' ', number)
                    }
                };
                let text = format!("{marker} {number:>width$} │ {}", line.text());
                match kind {
                    DiffLineKind::Addition => {
                        buffer.insert_with_tags(&mut iter, &text, &[&add_tag])
                    }
                    DiffLineKind::Deletion => {
                        buffer.insert_with_tags(&mut iter, &text, &[&del_tag])
                    }
                    _ => buffer.insert(&mut iter, &text),
                }
            }
        }
        buffer.insert(&mut iter, "\n");
    }

    view.upcast()
}

/// Raw extended header lines, shown for diffs without hunks.
fn metadata_view(file: &FileDiff) -> gtk4::Widget {
    let view = TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(WrapMode::None);
    view.set_left_margin(8);
    view.set_right_margin(8);
    view.add_css_class("frame");

    let buffer: TextBuffer = view.buffer();
    let mut iter = buffer.start_iter();
    for line in &file.metadata {
        let text = String::from_utf8_lossy(line);
        buffer.insert(&mut iter, &text);
        buffer.insert(&mut iter, "\n");
    }
    view.upcast()
}

fn file_button(label: &str, on_click: impl Fn() + 'static) -> Button {
    let button = Button::with_label(label);
    button.set_valign(Align::Center);
    button.connect_clicked(move |_| on_click());
    button
}

fn set_margins(widget: &impl IsA<gtk4::Widget>, margin: i32) {
    widget.set_margin_top(margin);
    widget.set_margin_bottom(margin);
    widget.set_margin_start(margin);
    widget.set_margin_end(margin);
}

fn file_path(file: &FileDiff) -> PathBuf {
    file.path()
        .map(|path| path.to_path_buf())
        .unwrap_or_default()
}

/// Width of the line number column for one hunk.
fn line_number_width(hunk: &Hunk) -> usize {
    let last_line = (hunk.old_start + hunk.old_count).max(hunk.new_start + hunk.new_count);
    last_line.to_string().len().max(2)
}

fn addition_background() -> gdk::RGBA {
    gdk::RGBA::new(0.20, 0.60, 0.30, 0.18)
}

fn deletion_background() -> gdk::RGBA {
    gdk::RGBA::new(0.85, 0.30, 0.30, 0.18)
}

fn new_tag(buffer: &TextBuffer, name: &str, background: gdk::RGBA) -> gtk4::TextTag {
    buffer
        .create_tag(Some(name), &[("paragraph-background-rgba", &background)])
        .expect("create text tag")
}

fn new_tag_dim(buffer: &TextBuffer) -> gtk4::TextTag {
    let gray = gdk::RGBA::new(0.5, 0.5, 0.5, 0.8);
    buffer
        .create_tag(Some("dim"), &[("foreground-rgba", &gray)])
        .expect("create text tag")
}
