//! Diff renderer (SPEC section 19).
//!
//! A file diff is rendered as a header row with the whole-file actions, then
//! one block per hunk: the hunk header with its actions next to it, and the
//! hunk body in a monospace text view with line numbers. Additions and
//! deletions keep their `+`/`-` markers and get a light background tint.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, TextBuffer, TextTag, TextView, WrapMode,
};
use sourceview5::Language;

use crate::syntax::Highlighter;

use crate::model::diff::{DiffLineKind, DiffStats, FileDiff, Hunk};
use crate::ui::clipboard::{self, CopyAction};
use crate::ui::folding::{DiffFoldState, FileFoldKey, FoldFocus, HunkFoldKey};
use crate::ui::{DiffSide, HunkTarget, display_name};

/// User actions available from the diff view.
pub struct Callbacks {
    /// Copy an existing model payload to the clipboard.
    pub copy: Box<dyn Fn(CopyAction)>,
    /// Stage one hunk of a working tree diff.
    pub stage_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Unstage one hunk of a staged diff.
    pub unstage_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Discard one hunk of a working tree diff (the caller asks for
    /// confirmation first).
    pub discard_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// Operations on selected changed lines of one modified-file hunk.
    pub stage_lines: Box<dyn Fn(FileDiff, Hunk, Vec<usize>)>,
    pub unstage_lines: Box<dyn Fn(FileDiff, Hunk, Vec<usize>)>,
    pub discard_lines: Box<dyn Fn(FileDiff, Hunk, Vec<usize>)>,
    /// Revert one hunk of a commit diff into the working tree (SPEC §18).
    pub revert_hunk: Box<dyn Fn(FileDiff, Hunk)>,
    /// A hunk received keyboard focus (for the contextual shortcuts).
    pub focus_hunk: Box<dyn Fn(HunkTarget)>,
    /// Stage every change of the given path.
    pub stage_file: Box<dyn Fn(PathBuf)>,
    /// Unstage every change of the given path.
    pub unstage_file: Box<dyn Fn(PathBuf)>,
    /// Discard the working tree changes of the given path (the caller asks for
    /// confirmation first).
    pub discard_file: Box<dyn Fn(PathBuf)>,
    /// Enter the selected file's HEAD history.
    pub file_history: Box<dyn Fn(FileDiff)>,
    /// Open the HEAD history of a tracked file preview.
    pub preview_history: Box<dyn Fn(PathBuf)>,
    /// Toggle the folding of a file (GITILANTE_DIFF_FOLDING_SPEC.md §23).
    pub toggle_file: Box<dyn Fn(FileFoldKey)>,
    /// Toggle the folding of a hunk.
    pub toggle_hunk: Box<dyn Fn(HunkFoldKey)>,
}

/// One piece of searchable rendered text: the buffer of a hunk body holds
/// clean source text only (no headers, numbers or gutter) and the view hosts it
/// (GITILANTE_SEARCH_SPEC.md section 3).
#[derive(Clone)]
pub struct SearchTarget {
    pub buffer: TextBuffer,
    pub view: TextView,
    /// The hunk this target belongs to, for the search navigation
    /// (GITILANTE_DIFF_FOLDING_SPEC.md section 30).
    pub hunk: Option<HunkFoldKey>,
}

/// A rendered diff: its widget and its searchable text (one per hunk body).
pub struct RenderedDiff {
    pub widget: gtk4::Widget,
    pub targets: Vec<SearchTarget>,
}

impl RenderedDiff {
    /// A rendered widget without searchable text (placeholders, metadata).
    pub fn plain(widget: gtk4::Widget) -> Self {
        Self {
            widget,
            targets: Vec::new(),
        }
    }
}

/// Renders the diff of `file` with the per-hunk and per-file actions of `side`.
pub fn render(
    file: &FileDiff,
    side: DiffSide,
    highlighter: &Highlighter,
    folds: &DiffFoldState,
    focus: Option<&FoldFocus>,
    callbacks: &Rc<Callbacks>,
) -> RenderedDiff {
    let root = GtkBox::new(Orientation::Vertical, 12);
    let mut targets = Vec::new();
    set_margins(&root, 12);

    let file_key = FileFoldKey::from_file(file);
    let collapsed = folds.is_file_collapsed(&file_key);
    root.append(&file_header(
        file, side, collapsed, &file_key, focus, callbacks,
    ));

    // A collapsed file renders its header only (GITILANTE_DIFF_FOLDING_SPEC.md
    // sections 3, 40 and 41).
    if collapsed {
        return RenderedDiff {
            widget: root.upcast(),
            targets,
        };
    }

    // One language per file: prefer the new path of a rename (COLORS.md §8).
    let language = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .and_then(crate::syntax::detect_language);

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
            let hunk_key = HunkFoldKey::from_hunk(file, hunk);
            let (widget, target) = hunk_view(
                file,
                hunk,
                side,
                language.as_ref(),
                highlighter,
                folds.is_hunk_collapsed(&hunk_key),
                focus,
                callbacks,
            );
            root.append(&widget);
            if let Some(target) = target {
                targets.push(target);
            }
        }
    }
    RenderedDiff {
        widget: root.upcast(),
        targets,
    }
}

/// Renders the placeholder shown for an untracked file (SPEC section 15).
pub fn render_untracked(path: &std::path::Path, callbacks: &Rc<Callbacks>) -> RenderedDiff {
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
    RenderedDiff::plain(root.upcast())
}

/// Renders the read-only file preview for the search results
/// (GITILANTE_SEARCH_SPEC.md sections 16 and 42).
///
/// The preview shows the working tree content with line numbers and syntax
/// highlighting and nothing else: no diff backgrounds, no intraline spans and
/// no hunk actions.
pub fn render_file_preview(
    path: &std::path::Path,
    content: &str,
    highlighter: &Highlighter,
    tracked: bool,
    callbacks: &Rc<Callbacks>,
) -> RenderedDiff {
    /// Large files are truncated: a preview is never a full editor.
    const MAX_PREVIEW_LINES: usize = 5000;

    let root = GtkBox::new(Orientation::Vertical, 12);
    set_margins(&root, 12);

    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name = Label::new(Some(&display_name(path, None)));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_ellipsize(pango::EllipsizeMode::Middle);
    name.set_hexpand(true);
    header.append(&name);
    let hint = Label::new(Some("working tree · read-only"));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    header.append(&hint);
    let path_for_copy = path.to_path_buf();
    let absolute_path = path.to_path_buf();
    let copy_callbacks = callbacks.clone();
    let absolute_callbacks = callbacks.clone();
    clipboard::context_menu(
        &header,
        vec![
            (
                "Copy path",
                Box::new(move || (copy_callbacks.copy)(CopyAction::Path(path_for_copy.clone()))),
            ),
            (
                "Copy absolute path",
                Box::new(move || {
                    (absolute_callbacks.copy)(CopyAction::AbsolutePath(absolute_path.clone()))
                }),
            ),
        ],
    );
    if tracked {
        let history = file_button("History", {
            let path = path.to_path_buf();
            let callbacks = callbacks.clone();
            move || (callbacks.preview_history)(path.clone())
        });
        history.set_tooltip_text(Some("Show history of this file"));
        header.append(&history);
    }
    root.append(&header);

    let lines: Vec<String> = content
        .lines()
        .take(MAX_PREVIEW_LINES)
        .map(str::to_owned)
        .collect();
    let truncated = content.lines().count() > MAX_PREVIEW_LINES;

    // One coherent source stream per file (COLORS.md section 11).
    let language = crate::syntax::detect_language(path);
    let scheme = crate::syntax::style_scheme();
    let styles = highlighter.highlight(language.as_ref(), scheme.as_ref(), &lines);

    let buffer = TextBuffer::new(Some(highlighter.table()));
    let mut text = String::new();
    for line in &lines {
        text.push_str(line);
        text.push('\n');
    }
    buffer.insert(&mut buffer.start_iter(), &text);
    for (row, segments) in styles.iter().enumerate() {
        for segment in segments {
            apply_range(
                &buffer,
                row as i32,
                segment.start,
                segment.end,
                &segment.tags,
            );
        }
    }

    // Line numbers live in the gutter, never in the source text
    // (COLORS.md sections 5 and 19).
    let width = lines.len().to_string().len();
    let mut gutter = String::new();
    for number in 1..=lines.len() {
        gutter.push_str(&format!("{number:>width$} │\n"));
    }
    let gutter_view = Label::new(Some(&gutter));
    gutter_view.add_css_class("monospace");
    gutter_view.add_css_class("dim-label");
    gutter_view.set_xalign(1.0);
    gutter_view.set_margin_start(8);
    gutter_view.set_margin_top(4);
    gutter_view.set_margin_bottom(4);

    let body = TextView::with_buffer(&buffer);
    body.set_editable(false);
    body.set_cursor_visible(false);
    body.set_monospace(true);
    body.set_wrap_mode(WrapMode::None);
    body.set_left_margin(4);
    body.set_right_margin(8);
    body.set_top_margin(4);
    body.set_bottom_margin(4);
    body.set_hexpand(true);

    let box_ = GtkBox::new(Orientation::Horizontal, 0);
    box_.add_css_class("frame");
    box_.append(&gutter_view);
    box_.append(&body);
    root.append(&box_);

    if truncated {
        root.append(&placeholder(&format!(
            "Preview truncated to the first {MAX_PREVIEW_LINES} lines"
        )));
    }

    let target = SearchTarget {
        buffer,
        view: body,
        hunk: None,
    };
    RenderedDiff {
        widget: root.upcast(),
        targets: vec![target],
    }
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
/// The ▼/▶ disclosure control of a collapsible section. The button carries the
/// expanded state for assistive technologies and works from the keyboard
/// (GITILANTE_DIFF_FOLDING_SPEC.md sections 6, 38 and 39).
fn disclosure_button(collapsed: bool, what: &str, on_toggle: impl Fn() + 'static) -> gtk4::Button {
    let button = gtk4::Button::from_icon_name(if collapsed {
        "pan-end-symbolic"
    } else {
        "pan-down-symbolic"
    });
    button.set_has_frame(false);
    button.set_tooltip_text(Some(&format!(
        "{} this {what}",
        if collapsed { "Expand" } else { "Collapse" }
    )));
    button.update_state(&[gtk4::accessible::State::Expanded(Some(!collapsed))]);
    button.connect_clicked(move |_| on_toggle());
    button
}

/// Makes the header text a click target for the folding toggle
/// (GITILANTE_DIFF_FOLDING_SPEC.md section 7). A drag still selects the text.
fn attach_toggle_gesture(label: &Label, on_toggle: impl Fn() + 'static) {
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    gesture.connect_released(move |_, _, _, _| on_toggle());
    label.add_controller(gesture);
}

/// The `+N −M` summary of a file or hunk
/// (GITILANTE_DIFF_FOLDING_SPEC.md sections 8-9).
fn stats_label(stats: DiffStats) -> Label {
    let label = Label::new(Some(&format!("+{} −{}", stats.additions, stats.deletions)));
    label.add_css_class("dim-label");
    label.add_css_class("monospace");
    label
}

fn file_header(
    file: &FileDiff,
    side: DiffSide,
    collapsed: bool,
    key: &FileFoldKey,
    focus: Option<&FoldFocus>,
    callbacks: &Rc<Callbacks>,
) -> gtk4::Widget {
    let header = GtkBox::new(Orientation::Horizontal, 8);

    let toggle = disclosure_button(collapsed, "file", {
        let callbacks = callbacks.clone();
        let key = key.clone();
        move || (callbacks.toggle_file)(key.clone())
    });
    header.append(&toggle);
    if matches!(focus, Some(FoldFocus::File(focused)) if focused == key) {
        toggle.grab_focus();
    }

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
    attach_toggle_gesture(&name, {
        let callbacks = callbacks.clone();
        let key = key.clone();
        move || (callbacks.toggle_file)(key.clone())
    });
    header.append(&name);

    header.append(&stats_label(file.stats()));

    if matches!(side, DiffSide::Staged | DiffSide::Unstaged) {
        let history = file_button("History", {
            let file = file.clone();
            let callbacks = callbacks.clone();
            move || (callbacks.file_history)(file.clone())
        });
        history.set_tooltip_text(Some("Show history of this file"));
        header.append(&history);
    }
    if !matches!(side, DiffSide::Conflicted) {
        let path = file.path().map(Path::to_path_buf);
        let absolute_path = path.clone();
        let file_for_diff = file.clone();
        let copy_path = callbacks.clone();
        let copy_absolute = callbacks.clone();
        let copy_diff = callbacks.clone();
        clipboard::context_menu(
            &header,
            vec![
                (
                    "Copy path",
                    Box::new(move || {
                        if let Some(path) = &path {
                            (copy_path.copy)(CopyAction::Path(path.clone()));
                        }
                    }),
                ),
                (
                    "Copy absolute path",
                    Box::new(move || {
                        if let Some(path) = &absolute_path {
                            (copy_absolute.copy)(CopyAction::AbsolutePath(path.clone()));
                        }
                    }),
                ),
                (
                    "Copy diff",
                    Box::new(move || (copy_diff.copy)(CopyAction::FileDiff(file_for_diff.clone()))),
                ),
            ],
        );
    }
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
        // Historical commits and conflicted files have no file actions: hunks
        // get [Revert hunk], conflicted files are read-only (SPEC §§ 19, 31).
        DiffSide::History | DiffSide::Conflicted => {}
    }
    header.upcast()
}

/// One hunk block: header with actions, then the hunk body.
/// One hunk: its collapsible header and, when expanded, its body.
#[allow(clippy::too_many_arguments)]
fn hunk_view(
    file: &FileDiff,
    hunk: &Hunk,
    side: DiffSide,
    language: Option<&Language>,
    highlighter: &Highlighter,
    collapsed: bool,
    focus: Option<&FoldFocus>,
    callbacks: &Rc<Callbacks>,
) -> (gtk4::Widget, Option<SearchTarget>) {
    let block = GtkBox::new(Orientation::Vertical, 4);
    let target = HunkTarget {
        file: file.clone(),
        hunk: hunk.clone(),
        side,
    };

    let header = GtkBox::new(Orientation::Horizontal, 8);
    let key = HunkFoldKey::from_hunk(file, hunk);
    let toggle = disclosure_button(collapsed, "hunk", {
        let callbacks = callbacks.clone();
        let key = key.clone();
        move || (callbacks.toggle_hunk)(key.clone())
    });
    header.append(&toggle);
    if matches!(focus, Some(FoldFocus::Hunk(focused)) if focused == &key) {
        toggle.grab_focus();
    }

    let header_label = Label::new(Some(&hunk.header_text()));
    header_label.add_css_class("monospace");
    header_label.add_css_class("heading");
    header_label.set_xalign(0.0);
    header_label.set_ellipsize(pango::EllipsizeMode::End);
    header_label.set_hexpand(true);
    header_label.set_selectable(true);
    attach_toggle_gesture(&header_label, {
        let callbacks = callbacks.clone();
        let key = key.clone();
        move || (callbacks.toggle_hunk)(key.clone())
    });
    track_focus(&header_label, &target, callbacks);
    header.append(&header_label);

    header.append(&stats_label(hunk.stats()));

    match side {
        DiffSide::Staged => {
            push_hunk_button(
                &header,
                "Unstage",
                "Unstage this hunk (U)",
                &target,
                callbacks,
                {
                    let callbacks = callbacks.clone();
                    let file = file.clone();
                    let hunk = hunk.clone();
                    move || (callbacks.unstage_hunk)(file.clone(), hunk.clone())
                },
            );
        }
        DiffSide::Unstaged => {
            push_hunk_button(
                &header,
                "Stage",
                "Stage this hunk (S)",
                &target,
                callbacks,
                {
                    let callbacks = callbacks.clone();
                    let file = file.clone();
                    let hunk = hunk.clone();
                    move || (callbacks.stage_hunk)(file.clone(), hunk.clone())
                },
            );
            push_hunk_button(
                &header,
                "Discard",
                "Discard this hunk (D)",
                &target,
                callbacks,
                {
                    let callbacks = callbacks.clone();
                    let file = file.clone();
                    let hunk = hunk.clone();
                    move || (callbacks.discard_hunk)(file.clone(), hunk.clone())
                },
            );
        }
        DiffSide::History => {
            push_hunk_button(
                &header,
                "Revert hunk",
                "Revert this hunk (R)",
                &target,
                callbacks,
                {
                    let callbacks = callbacks.clone();
                    let file = file.clone();
                    let hunk = hunk.clone();
                    move || (callbacks.revert_hunk)(file.clone(), hunk.clone())
                },
            );
        }
        // Conflicted files are read-only (SPEC sections 25 and 31).
        DiffSide::Conflicted => {}
    }
    if !matches!(side, DiffSide::Conflicted) {
        let file = file.clone();
        let hunk = hunk.clone();
        let callbacks = callbacks.clone();
        clipboard::context_menu(
            &header,
            vec![(
                "Copy hunk diff",
                Box::new(move || {
                    (callbacks.copy)(CopyAction::HunkDiff(file.clone(), hunk.clone()));
                }),
            )],
        );
    }
    block.append(&header);

    // Collapsed hunks render the header only: the line widgets are never
    // created (GITILANTE_DIFF_FOLDING_SPEC.md sections 3, 40 and 41).
    if collapsed {
        return (block.upcast(), None);
    }

    let (body, mut search_target) = hunk_body(hunk, language, highlighter);
    search_target.hunk = Some(key);
    track_focus(&body, &target, callbacks);
    if matches!(side, DiffSide::Staged | DiffSide::Unstaged)
        && crate::git::patch::supports_selected_lines(file, hunk)
    {
        block.append(&selected_line_actions(
            file,
            hunk,
            side,
            &search_target.buffer,
            callbacks,
        ));
    }
    block.append(&body);
    (block.upcast(), Some(search_target))
}

/// Maps a half-open GTK text selection to modified hunk-line indexes.
fn selected_line_indexes(hunk: &Hunk, start: &gtk4::TextIter, end: &gtk4::TextIter) -> Vec<usize> {
    selected_line_range(
        hunk,
        start.line().max(0) as usize,
        end.line().max(0) as usize,
        end.line_offset() as usize,
    )
}

fn selected_line_range(
    hunk: &Hunk,
    first: usize,
    end_line: usize,
    end_column: usize,
) -> Vec<usize> {
    let last = end_line.saturating_add(usize::from(end_column > 0));
    (first..last.min(hunk.lines.len()))
        .filter(|&index| {
            matches!(
                hunk.lines[index].kind,
                DiffLineKind::Addition | DiffLineKind::Deletion
            )
        })
        .collect()
}

/// Contextual actions below the hunk header, visible only for changed lines.
fn selected_line_actions(
    file: &FileDiff,
    hunk: &Hunk,
    side: DiffSide,
    buffer: &TextBuffer,
    callbacks: &Rc<Callbacks>,
) -> gtk4::Widget {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_visible(false);
    let count = Label::new(None);
    count.set_hexpand(true);
    count.set_xalign(0.0);
    row.append(&count);
    let selected = Rc::new(RefCell::new(Vec::new()));
    let add_action = |label: &str| {
        let file = file.clone();
        let hunk = hunk.clone();
        let selected = selected.clone();
        let button = Button::with_label(label);
        row.append(&button);
        (button, file, hunk, selected)
    };
    match side {
        DiffSide::Staged => {
            let (button, file, hunk, selected) = add_action("Unstage selected");
            let callbacks = callbacks.clone();
            button.connect_clicked(move |_| {
                (callbacks.unstage_lines)(file.clone(), hunk.clone(), selected.borrow().clone())
            });
        }
        DiffSide::Unstaged => {
            let (button, file, hunk, selected) = add_action("Stage selected");
            let stage_callbacks = callbacks.clone();
            button.connect_clicked(move |_| {
                (stage_callbacks.stage_lines)(file.clone(), hunk.clone(), selected.borrow().clone())
            });
            let (button, file, hunk, selected) = add_action("Discard selected");
            let callbacks = callbacks.clone();
            button.connect_clicked(move |_| {
                (callbacks.discard_lines)(file.clone(), hunk.clone(), selected.borrow().clone())
            });
        }
        _ => unreachable!(),
    }
    let hunk = hunk.clone();
    let row_for_selection = row.clone();
    buffer.connect_mark_set(move |buffer, _, _| {
        let indexes = buffer
            .selection_bounds()
            .map_or_else(Vec::new, |(start, end)| {
                selected_line_indexes(&hunk, &start, &end)
            });
        count.set_text(&format!("{} changed lines selected", indexes.len()));
        row_for_selection.set_visible(!indexes.is_empty());
        *selected.borrow_mut() = indexes;
    });
    row.upcast()
}

/// Appends an action button that reports its hunk when focused or clicked.
fn push_hunk_button(
    header: &GtkBox,
    label: &str,
    tooltip: &str,
    target: &HunkTarget,
    callbacks: &Rc<Callbacks>,
    on_click: impl Fn() + 'static,
) {
    let button = file_button(label, on_click);
    button.set_tooltip_text(Some(tooltip));
    track_focus(&button, target, callbacks);
    header.append(&button);
}

/// Reports `target` to the caller when `widget` gains keyboard focus, so the
/// contextual shortcuts know which hunk to act on (SPEC section 30).
fn track_focus(widget: &impl IsA<gtk4::Widget>, target: &HunkTarget, callbacks: &Rc<Callbacks>) {
    let controller = gtk4::EventControllerFocus::new();
    let callbacks = callbacks.clone();
    let target = target.clone();
    controller.connect_enter(move |_| (callbacks.focus_hunk)(target.clone()));
    widget.add_controller(controller);
}

/// Which logical source stream styles a visible line (COLORS.md §11).
#[derive(Debug, Clone, Copy)]
enum Side {
    /// The old (removed) side of a change block.
    Old,
    /// The new (added) side of a change block.
    New,
}

/// Hunk body: the diff gutter beside the clean source text.
///
/// The visible buffer only contains the source text (COLORS.md sections 5 and
/// 19), so copying always yields the file content: line numbers and diff
/// markers live in the unselectable gutter. The three visual levels compose
/// (COLORS.md section 28 and DIFF.md section 25): syntax tags carry the
/// foreground, the line tags the base background and the intraline tags the
/// strong background on top. Styles come from the two logical streams of the
/// hunk, never from the mixed text (COLORS.md sections 11 and 12).
fn hunk_body(
    hunk: &Hunk,
    language: Option<&Language>,
    highlighter: &Highlighter,
) -> (gtk4::Widget, SearchTarget) {
    // Build the old and new source streams and remember, for every visible
    // line, which stream styles it (context lines are shared, section 20).
    let (mut old_lines, mut new_lines) = (Vec::<String>::new(), Vec::<String>::new());
    let mut sources: Vec<Option<(Side, usize)>> = Vec::with_capacity(hunk.lines.len());
    for line in &hunk.lines {
        let text = line.text().into_owned();
        match line.kind {
            DiffLineKind::Deletion => {
                sources.push(Some((Side::Old, old_lines.len())));
                old_lines.push(text);
            }
            DiffLineKind::Addition => {
                sources.push(Some((Side::New, new_lines.len())));
                new_lines.push(text);
            }
            DiffLineKind::Context => {
                sources.push(Some((Side::New, new_lines.len())));
                old_lines.push(text.clone());
                new_lines.push(text);
            }
            DiffLineKind::NoNewlineMarker => sources.push(None),
        }
    }

    let scheme = crate::syntax::style_scheme();
    let old_styles = highlighter.highlight(language, scheme.as_ref(), &old_lines);
    let new_styles = highlighter.highlight(language, scheme.as_ref(), &new_lines);

    let buffer = TextBuffer::new(Some(highlighter.table()));
    let add_tag = new_tag(&buffer, addition_background());
    let del_tag = new_tag(&buffer, deletion_background());
    let add_strong = new_tag_strong(&buffer, addition_strong_background());
    let del_strong = new_tag_strong(&buffer, deletion_strong_background());
    let dim_tag = new_tag_dim(&buffer);

    let mut text = String::new();
    for line in &hunk.lines {
        text.push_str(&line.text());
        text.push('\n');
    }
    buffer.insert(&mut buffer.start_iter(), &text);

    for (index, line) in hunk.lines.iter().enumerate() {
        let row = index as i32;
        let chars = line.text().chars().count();
        // Syntax foreground (COLORS.md section 3).
        if let Some((side, source)) = sources[index] {
            let styles = match side {
                Side::Old => &old_styles,
                Side::New => &new_styles,
            };
            for segment in &styles[source] {
                apply_range(&buffer, row, segment.start, segment.end, &segment.tags);
            }
        }
        // Line diff background (COLORS.md section 3).
        let line_tag = match line.kind {
            DiffLineKind::Addition => Some(&add_tag),
            DiffLineKind::Deletion => Some(&del_tag),
            _ => None,
        };
        if let Some(tag) = line_tag {
            apply_range(&buffer, row, 0, chars, std::slice::from_ref(tag));
        }
        // Strong intraline background (DIFF.md section 25).
        let strong = match line.kind {
            DiffLineKind::Addition => Some(&add_strong),
            DiffLineKind::Deletion => Some(&del_strong),
            _ => None,
        };
        if let Some(tag) = strong {
            for span in &line.intraline {
                apply_range(
                    &buffer,
                    row,
                    span.start,
                    span.end,
                    std::slice::from_ref(tag),
                );
            }
        }
        if line.kind == DiffLineKind::NoNewlineMarker {
            apply_range(&buffer, row, 0, chars, std::slice::from_ref(&dim_tag));
        }
    }

    let body = TextView::with_buffer(&buffer);
    body.set_editable(false);
    body.set_cursor_visible(false);
    body.set_monospace(true);
    body.set_wrap_mode(WrapMode::None);
    body.set_left_margin(4);
    body.set_right_margin(8);
    body.set_top_margin(4);
    body.set_bottom_margin(4);
    body.set_hexpand(true);

    // Diff gutter: numbers and markers, never part of the source text and
    // never selectable (COLORS.md sections 5, 6 and 19).
    let mut gutter = String::new();
    let width = line_number_width(hunk);
    let mut old_num = hunk.old_start;
    let mut new_num = hunk.new_start;
    for line in &hunk.lines {
        let cell = match line.kind {
            DiffLineKind::NoNewlineMarker => format!("  {:>width$} │", ""),
            DiffLineKind::Addition => {
                let number = new_num;
                new_num += 1;
                format!("+ {number:>width$} │")
            }
            DiffLineKind::Deletion => {
                let number = old_num;
                old_num += 1;
                format!("- {number:>width$} │")
            }
            _ => {
                let number = new_num;
                old_num += 1;
                new_num += 1;
                format!("  {number:>width$} │")
            }
        };
        gutter.push_str(&cell);
        gutter.push('\n');
    }

    let gutter_view = Label::new(Some(&gutter));
    gutter_view.add_css_class("monospace");
    gutter_view.add_css_class("dim-label");
    gutter_view.set_xalign(0.0);
    gutter_view.set_margin_start(8);
    gutter_view.set_margin_top(4);
    gutter_view.set_margin_bottom(4);

    let box_ = GtkBox::new(Orientation::Horizontal, 0);
    box_.add_css_class("frame");
    box_.append(&gutter_view);
    box_.append(&body);
    let search_target = SearchTarget {
        buffer,
        view: body,
        hunk: None,
    };
    (box_.upcast(), search_target)
}

/// Applies `tags` to a character range of one buffer line.
pub(crate) fn apply_range(
    buffer: &TextBuffer,
    line: i32,
    start: usize,
    end: usize,
    tags: &[TextTag],
) {
    if start >= end {
        return;
    }
    let Some(start) = buffer.iter_at_line_offset(line, start as i32) else {
        return;
    };
    let Some(end) = buffer.iter_at_line_offset(line, end as i32) else {
        return;
    };
    for tag in tags {
        buffer.apply_tag(tag, &start, &end);
    }
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

pub(crate) fn set_margins(widget: &impl IsA<gtk4::Widget>, margin: i32) {
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

pub(crate) fn addition_strong_background() -> gdk::RGBA {
    gdk::RGBA::new(0.20, 0.60, 0.30, 0.38)
}

fn deletion_background() -> gdk::RGBA {
    gdk::RGBA::new(0.85, 0.30, 0.30, 0.18)
}

pub(crate) fn deletion_strong_background() -> gdk::RGBA {
    gdk::RGBA::new(0.85, 0.30, 0.30, 0.38)
}

/// Diff and dim tags are anonymous: the tag table is shared with the syntax
/// tags (COLORS.md section 28) and named tags would collide between hunks.
fn new_tag(buffer: &TextBuffer, background: gdk::RGBA) -> gtk4::TextTag {
    buffer
        .create_tag(None::<&str>, &[("paragraph-background-rgba", &background)])
        .expect("create text tag")
}

fn new_tag_dim(buffer: &TextBuffer) -> gtk4::TextTag {
    let gray = gdk::RGBA::new(0.5, 0.5, 0.5, 0.8);
    buffer
        .create_tag(None::<&str>, &[("foreground-rgba", &gray)])
        .expect("create text tag")
}

/// Overlay tag for the strong intraline spans: the background covers the text
/// extent only, on top of the line background (COLORS.md section 28).
pub(crate) fn new_tag_strong(buffer: &TextBuffer, background: gdk::RGBA) -> gtk4::TextTag {
    buffer
        .create_tag(None::<&str>, &[("background-rgba", &background)])
        .expect("create text tag")
}

#[cfg(test)]
mod selected_line_tests {
    use super::*;
    use crate::model::diff::DiffLine;

    #[test]
    fn selection_end_at_next_line_start_excludes_that_line() {
        let hunk = Hunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 2,
            header: Vec::new(),
            lines: [
                DiffLineKind::Context,
                DiffLineKind::Deletion,
                DiffLineKind::Addition,
                DiffLineKind::Context,
            ]
            .into_iter()
            .map(|kind| DiffLine {
                kind,
                content: b"text".to_vec(),
                intraline: Vec::new(),
            })
            .collect(),
        };
        assert_eq!(selected_line_range(&hunk, 0, 2, 0), vec![1]);
        assert_eq!(selected_line_range(&hunk, 1, 2, 1), vec![1, 2]);
        assert!(selected_line_range(&hunk, 0, 1, 0).is_empty());
    }
}
