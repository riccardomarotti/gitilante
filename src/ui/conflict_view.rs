//! Visual conflict solver (docs/GITILANTE_CONFLICT_SOLVER_SPEC.md).
//!
//! The UI answers one question only: which content must end up in the result
//! file? It never says `ours`/`theirs`: every label comes from the
//! operation-aware [`ConflictContext`]. The only editable area is the RESULT
//! of a single conflict block (§25).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, TextBuffer, TextView, WrapMode};

use crate::conflict::context::ConflictContext;
use crate::conflict::model::ConflictBlock;
use crate::conflict::presentation::{ConflictPresentation, ConflictSide, intraline_marks};
use crate::conflict::resolution::ConflictResolution;
use crate::git::conflict::ConflictLoad;
use crate::syntax::Highlighter;
use crate::ui::clipboard;
use crate::ui::diff_view::{self, RenderedDiff, SearchTarget};

/// File-level choice state for non-textual conflicts (§48-51).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileChoice {
    /// No choice made yet.
    #[default]
    None,
    /// Keep the surviving version (§48-49).
    KeepFile,
    /// Delete the file (§48-50).
    DeleteFile,
    /// Use the complete stage 2 version (§51).
    UseVersionA,
    /// Use the complete stage 3 version (§51).
    UseVersionB,
    /// Resolve the conflict as a deletion (§50).
    ResolveAsDeleted,
}

/// One solver session: choices live in memory only (§60, §69).
pub struct ConflictSession {
    /// Conflicted path, relative to the repository root.
    pub path: PathBuf,
    /// Loaded conflict state (context, stages, snapshot, presentation).
    pub load: ConflictLoad,
    /// One resolution per conflict block (§22).
    pub resolutions: Vec<ConflictResolution>,
    /// Visual folding per block (§35).
    pub collapsed: Vec<bool>,
    /// Base disclosure state per block (§29).
    pub base_visible: Vec<bool>,
    /// Choice for file-level presentations (§48-51).
    pub file_choice: FileChoice,
}

impl ConflictSession {
    /// Builds a session from a freshly loaded conflict.
    pub fn new(path: PathBuf, load: ConflictLoad) -> Self {
        let blocks = match &load.presentation {
            ConflictPresentation::TextBlocks(file) => file.conflict_count(),
            _ => 0,
        };
        Self {
            path,
            load,
            resolutions: vec![ConflictResolution::Unresolved; blocks],
            collapsed: vec![false; blocks],
            base_visible: vec![false; blocks],
            file_choice: FileChoice::None,
        }
    }

    /// Number of conflict blocks, or 1 for file-level presentations.
    pub fn total_units(&self) -> usize {
        match &self.load.presentation {
            ConflictPresentation::TextBlocks(file) => file.conflict_count(),
            _ => 1,
        }
    }

    /// Number of resolved units (§27, §33).
    pub fn resolved_units(&self) -> usize {
        match &self.load.presentation {
            ConflictPresentation::TextBlocks(_) => {
                self.resolutions.iter().filter(|r| r.is_resolved()).count()
            }
            ConflictPresentation::Unsupported(_) => 0,
            _ => usize::from(self.file_choice != FileChoice::None),
        }
    }

    /// True when the final apply can run (§38).
    pub fn is_complete(&self) -> bool {
        self.total_units() > 0 && self.resolved_units() == self.total_units()
    }
}

/// User actions of the solver, handled by the main window.
pub struct Callbacks {
    /// Open the current conflicted working-tree file externally.
    pub open_editor: Box<dyn Fn(PathBuf)>,
    /// A block received its resolution (also from Result editing, §25).
    pub choose: Box<dyn Fn(usize, ConflictResolution)>,
    /// A block is reset to unresolved (§28).
    pub reset: Box<dyn Fn(usize)>,
    /// A block was folded or expanded (§35).
    pub toggle_collapse: Box<dyn Fn(usize)>,
    /// The base disclosure of a block was toggled (§29).
    pub toggle_base: Box<dyn Fn(usize)>,
    /// A file-level choice was made (§48-51).
    pub choose_file: Box<dyn Fn(FileChoice)>,
    /// Apply the resolution and mark the file resolved (§43).
    pub apply: Box<dyn Fn()>,
    /// Mark the current file resolved without touching it (§41).
    pub mark_resolved: Box<dyn Fn()>,
    /// Reload the conflict (§42, §61).
    pub refresh: Box<dyn Fn()>,
}

/// Renders the solver for the session.
///
/// `window_width` drives the responsive layout: wide windows show the two
/// sources side by side, narrow ones stack them (§32).
pub fn render(
    session: &ConflictSession,
    highlighter: &Highlighter,
    callbacks: &Rc<Callbacks>,
    window_width: i32,
) -> RenderedDiff {
    let wide = window_width >= WIDE_LAYOUT_MIN_WIDTH;
    let root = GtkBox::new(Orientation::Vertical, 12);
    diff_view::set_margins(&root, 12);
    root.append(&header(session, callbacks));

    let targets = match &session.load.presentation {
        ConflictPresentation::TextBlocks(file) => {
            let (widget, targets) = blocks_view(session, file, highlighter, callbacks, wide);
            root.append(&widget);
            targets
        }
        presentation => {
            let (widget, targets) = file_level_view(session, presentation, highlighter, callbacks);
            root.append(&widget);
            targets
        }
    };
    RenderedDiff {
        widget: root.upcast(),
        targets,
    }
}

/// Header: path and operation context (§33).
fn header(session: &ConflictSession, callbacks: &Rc<Callbacks>) -> gtk4::Widget {
    let root = GtkBox::new(Orientation::Vertical, 2);
    let path = Label::new(Some(&session.path.display().to_string()));
    path.add_css_class("heading");
    path.set_xalign(0.0);
    path.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    root.append(&path);
    if let Some(description) = session.load.context.description() {
        let operation = Label::new(Some(&description));
        operation.add_css_class("dim-label");
        operation.add_css_class("caption");
        operation.set_xalign(0.0);
        root.append(&operation);
    }
    root.append(&hint_label(
        "Nothing is written until the final apply. Manual edits outside Gitilante are never overwritten.",
    ));
    let relative_path = session.path.clone();
    let callbacks = callbacks.clone();
    clipboard::context_menu(
        &root,
        vec![(
            "Open in Editor",
            Box::new(move || {
                (callbacks.open_editor)(relative_path.clone());
            }),
        )],
    );
    root.upcast()
}

/// Small dim informational label.
fn hint_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

/// `3 conflicts · 1 resolved` (§33).
fn counter_text(total: usize, resolved: usize) -> String {
    format!("{total} conflicts · {resolved} resolved")
}

/// Minimum window width for the side-by-side layout (§32).
const WIDE_LAYOUT_MIN_WIDTH: i32 = 1100;

/// All conflict blocks with the counter and the final apply (§33-38).
fn blocks_view(
    session: &ConflictSession,
    file: &crate::conflict::ConflictFile,
    highlighter: &Highlighter,
    callbacks: &Rc<Callbacks>,
    wide: bool,
) -> (gtk4::Widget, Vec<SearchTarget>) {
    let root = GtkBox::new(Orientation::Vertical, 12);
    let mut targets = Vec::new();
    let total = file.conflict_count();

    let resolved_flags: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(vec![false; total]));
    let counter = Label::new(Some(&counter_text(total, session.resolved_units())));
    counter.add_css_class("dim-label");
    counter.add_css_class("caption");
    counter.set_xalign(0.0);
    root.append(&counter);

    let apply_button = Button::with_label("Apply resolution and mark resolved");
    apply_button.add_css_class("suggested-action");
    apply_button.set_sensitive(session.is_complete());

    for (index, block) in file.conflicts().enumerate() {
        let (widget, new_targets) = block_view(
            index,
            block,
            session,
            highlighter,
            callbacks,
            &resolved_flags,
            total,
            &counter,
            &apply_button,
            wide,
        );
        root.append(&widget);
        targets.extend(new_targets);
    }

    let apply_callbacks = callbacks.clone();
    apply_button.connect_clicked(move |_| (apply_callbacks.apply)());
    root.append(&apply_button);
    root.append(&hint_label(
        "Apply resolution writes the file and stages it. Continue the Git operation from the command line.",
    ));
    (root.upcast(), targets)
}

/// One conflict block: sources, actions, base and the editable Result (§25-36).
#[allow(clippy::too_many_arguments)]
fn block_view(
    index: usize,
    block: &ConflictBlock,
    session: &ConflictSession,
    highlighter: &Highlighter,
    callbacks: &Rc<Callbacks>,
    resolved_flags: &Rc<RefCell<Vec<bool>>>,
    total: usize,
    counter: &Label,
    apply_button: &Button,
    wide: bool,
) -> (gtk4::Widget, Vec<SearchTarget>) {
    let root = GtkBox::new(Orientation::Vertical, 6);
    let mut targets = Vec::new();
    let labels = context_labels(&session.load.context);

    // Guard so programmatic Result updates do not count as user edits (§27).
    let updating: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Disclosure header with the per-block state (§27, §35).
    let status_button = Button::with_label(&format!("▶ {}", status_text(index, false)));
    status_button.add_css_class("flat");
    status_button.set_halign(gtk4::Align::Start);
    let body = GtkBox::new(Orientation::Vertical, 8);
    {
        let body = body.clone();
        let status_button_in = status_button.clone();
        let flags = resolved_flags.clone();
        let callbacks = callbacks.clone();
        let collapsed = session.collapsed[index];
        body.set_visible(!collapsed);
        set_status_label(&status_button, index, collapsed, flags.borrow()[index]);
        status_button.connect_clicked(move |_| {
            let visible = body.is_visible();
            body.set_visible(!visible);
            set_status_label(&status_button_in, index, visible, flags.borrow()[index]);
            (callbacks.toggle_collapse)(index);
        });
    }
    root.append(&status_button);

    // Source panels with the operation-aware labels (§5-9); near-identical
    // lines get intraline marks (§31) and wide windows a two-column layout (§32).
    let (marks_a, marks_b) = intraline_marks(&block.source_a, &block.source_b);
    let panels = GtkBox::new(
        if wide {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        },
        8,
    );
    let (panel_a, target_a) = source_panel(
        &labels.a,
        &block.source_a,
        &session.path,
        highlighter,
        &marks_a,
        Some(diff_view::deletion_strong_background()),
    );
    let (panel_b, target_b) = source_panel(
        &labels.b,
        &block.source_b,
        &session.path,
        highlighter,
        &marks_b,
        Some(diff_view::addition_strong_background()),
    );
    panels.append(&panel_a);
    panels.append(&panel_b);
    body.append(&panels);
    targets.push(target_a);
    targets.push(target_b);

    // Actions with the operation-aware wording (§23-24).
    let actions = GtkBox::new(Orientation::Horizontal, 8);
    // GtkSourceView renders the SourceBuffer's live syntax styles; a plain
    // GtkTextView does not reliably display them while editing.
    let result_view: TextView = sourceview5::View::new().upcast();
    let set_resolved_state = progress_updater(
        index,
        resolved_flags.clone(),
        total,
        counter.clone(),
        apply_button.clone(),
        status_button.clone(),
    );

    for (label, resolution) in [
        (
            session.load.context.use_a_action(),
            ConflictResolution::SourceA,
        ),
        (
            session.load.context.use_b_action(),
            ConflictResolution::SourceB,
        ),
        (
            session.load.context.a_then_b_action(),
            ConflictResolution::SourceAThenB,
        ),
        (
            session.load.context.b_then_a_action(),
            ConflictResolution::SourceBThenA,
        ),
    ] {
        let button = Button::with_label(label);
        let callbacks = callbacks.clone();
        let set_resolved_state = set_resolved_state.clone();
        let updating = updating.clone();
        let result_view = result_view.clone();
        let block = block.clone();
        button.connect_clicked(move |_| {
            if let Some(bytes) = block.resolved_bytes(&resolution) {
                set_result_text(&result_view, &bytes, &updating);
            }
            set_resolved_state(true);
            (callbacks.choose)(index, resolution.clone());
        });
        actions.append(&button);
    }
    let reset = Button::with_label("Reset");
    {
        let callbacks = callbacks.clone();
        let set_resolved_state = set_resolved_state.clone();
        let updating = updating.clone();
        let result_view = result_view.clone();
        reset.connect_clicked(move |_| {
            set_result_text(&result_view, b"", &updating);
            set_resolved_state(false);
            (callbacks.reset)(index);
        });
    }
    actions.append(&reset);
    body.append(&actions);

    // Base block behind a disclosure (§29-30).
    if let Some(base) = &block.base {
        let base_button = Button::with_label(if session.base_visible[index] {
            "▼ Hide base"
        } else {
            "▶ Show base"
        });
        base_button.add_css_class("flat");
        base_button.set_halign(gtk4::Align::Start);
        let (base_panel, base_target) =
            source_panel("BASE", base, &session.path, highlighter, &[], None);
        base_panel.set_visible(session.base_visible[index]);
        {
            let base_panel = base_panel.clone();
            let base_button_in = base_button.clone();
            let callbacks = callbacks.clone();
            base_button.connect_clicked(move |_| {
                let visible = base_panel.is_visible();
                base_panel.set_visible(!visible);
                base_button_in.set_label(if visible {
                    "▶ Show base"
                } else {
                    "▼ Hide base"
                });
                (callbacks.toggle_base)(index);
            });
        }
        body.append(&base_button);
        body.append(&base_panel);
        targets.push(base_target);
    }

    // Editable RESULT (§25-27): the only editable area of the feature.
    let result_label = Label::new(Some("RESULT"));
    result_label.add_css_class("caption");
    result_label.set_xalign(0.0);
    body.append(&result_label);
    let initial = block.resolved_bytes(&session.resolutions[index]);
    let (result_widget, result_target) = result_panel(
        index,
        &session.path,
        initial.as_deref(),
        &result_view,
        callbacks,
        &set_resolved_state,
        &updating,
    );
    body.append(&result_widget);
    targets.push(result_target);

    root.append(&body);
    (root.upcast(), targets)
}

/// Builds the closure updating progress, apply sensitivity and the block state.
fn progress_updater(
    index: usize,
    resolved_flags: Rc<RefCell<Vec<bool>>>,
    total: usize,
    counter: Label,
    apply_button: Button,
    status_button: Button,
) -> Rc<dyn Fn(bool)> {
    Rc::new(move |resolved: bool| {
        let done = {
            let mut flags = resolved_flags.borrow_mut();
            flags[index] = resolved;
            flags.iter().filter(|flag| **flag).count()
        };
        counter.set_text(&counter_text(total, done));
        apply_button.set_sensitive(done == total);
        let label = status_button.label().unwrap_or_default();
        set_status_label(&status_button, index, !label.starts_with('▼'), resolved);
    })
}

/// Updates the `✓/○/▶/▼` disclosure label of a block (§27, §35).
fn set_status_label(button: &Button, index: usize, collapsed: bool, resolved: bool) {
    button.set_label(&format!(
        "{} {}",
        if collapsed { "▶" } else { "▼" },
        status_text(index, resolved)
    ));
}

/// `✓ Conflict 2 — resolved` (§27).
fn status_text(index: usize, resolved: bool) -> String {
    format!(
        "{} Conflict {} — {}",
        if resolved { "✓" } else { "○" },
        index + 1,
        if resolved { "resolved" } else { "unresolved" }
    )
}

/// Labels of both sources, resolved for the operation (§5-9).
struct ContextLabels {
    a: String,
    b: String,
}

fn context_labels(context: &ConflictContext) -> ContextLabels {
    ContextLabels {
        a: context.source_a.label.clone(),
        b: context.source_b.label.clone(),
    }
}

/// Replaces the Result buffer content without marking the block edited.
fn set_result_text(view: &TextView, bytes: &[u8], updating: &Rc<Cell<bool>>) {
    updating.set(true);
    view.buffer().set_text(&String::from_utf8_lossy(bytes));
    updating.set(false);
}

/// Read-only source panel with syntax highlighting (§66).
fn source_panel(
    title: &str,
    content: &[u8],
    path: &Path,
    highlighter: &Highlighter,
    marks: &[Vec<(usize, usize)>],
    strong: Option<gtk4::gdk::RGBA>,
) -> (gtk4::Widget, SearchTarget) {
    let root = GtkBox::new(Orientation::Vertical, 2);
    root.set_hexpand(true);
    let caption = Label::new(Some(title));
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");
    caption.set_xalign(0.0);
    root.append(&caption);

    let text = String::from_utf8_lossy(content);
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let buffer = highlighted_buffer(path, &lines, highlighter);
    if let Some(color) = strong {
        // Strong intraline background on top of the syntax styles (§31).
        let tag = diff_view::new_tag_strong(&buffer, color);
        for (row, spans) in marks.iter().enumerate() {
            for (start, end) in spans {
                diff_view::apply_range(
                    &buffer,
                    row as i32,
                    *start,
                    *end,
                    std::slice::from_ref(&tag),
                );
            }
        }
    }
    let view = TextView::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(WrapMode::None);
    view.set_left_margin(4);
    view.set_right_margin(4);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.add_css_class("frame");
    root.append(&view.clone());
    let target = SearchTarget {
        buffer,
        view,
        hunk: None,
    };
    (root.upcast(), target)
}

/// Editable RESULT panel (§25-26).
#[allow(clippy::too_many_arguments)]
fn result_panel(
    index: usize,
    path: &Path,
    initial: Option<&[u8]>,
    view: &TextView,
    callbacks: &Rc<Callbacks>,
    set_resolved_state: &Rc<dyn Fn(bool)>,
    updating: &Rc<Cell<bool>>,
) -> (gtk4::Widget, SearchTarget) {
    let text = String::from_utf8_lossy(initial.unwrap_or(b""));
    let buffer = crate::syntax::editable_buffer(path, &text);
    view.set_buffer(Some(&buffer));
    view.set_editable(true);
    view.set_monospace(true);
    view.set_wrap_mode(WrapMode::None);
    view.set_left_margin(4);
    view.set_right_margin(4);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.add_css_class("frame");
    view.set_hexpand(true);

    // Typing a custom result resolves the block (§22, §27).
    {
        let callbacks = callbacks.clone();
        let set_resolved_state = set_resolved_state.clone();
        let updating = updating.clone();
        buffer.connect_changed(move |buffer| {
            if updating.get() {
                return;
            }
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
            (callbacks.choose)(index, ConflictResolution::Custom(text.as_bytes().to_vec()));
            set_resolved_state(true);
        });
    }
    (
        view.clone().upcast(),
        SearchTarget {
            buffer: buffer.upcast(),
            view: view.clone(),
            hunk: None,
        },
    )
}

/// A highlighted source buffer (one language stream per panel, COLORS.md §11).
fn highlighted_buffer(path: &Path, lines: &[String], highlighter: &Highlighter) -> TextBuffer {
    let language = crate::syntax::detect_language(path);
    let scheme = crate::syntax::style_scheme();
    let styles = highlighter.highlight(language.as_ref(), scheme.as_ref(), lines);
    let buffer = TextBuffer::new(Some(highlighter.table()));
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    buffer.insert(&mut buffer.start_iter(), &text);
    for (row, segments) in styles.iter().enumerate() {
        for segment in segments {
            diff_view::apply_range(
                &buffer,
                row as i32,
                segment.start,
                segment.end,
                &segment.tags,
            );
        }
    }
    buffer
}

/// Non-textual presentations (§41-55).
fn file_level_view(
    session: &ConflictSession,
    presentation: &ConflictPresentation,
    highlighter: &Highlighter,
    callbacks: &Rc<Callbacks>,
) -> (gtk4::Widget, Vec<SearchTarget>) {
    let root = GtkBox::new(Orientation::Vertical, 8);
    let mut targets = Vec::new();
    let labels = context_labels(&session.load.context);

    match presentation {
        ConflictPresentation::KeepOrDelete {
            content,
            content_side,
        } => {
            let owner = match content_side {
                ConflictSide::SourceA => &labels.a,
                ConflictSide::SourceB => &labels.b,
            };
            root.append(&hint_label(&format!(
                "One side deleted this file. The version below comes from {owner}."
            )));
            let (panel, target) =
                source_panel(owner, content, &session.path, highlighter, &[], None);
            root.append(&panel);
            targets.push(target);
            append_file_actions(
                &root,
                session,
                callbacks,
                &[
                    ("Keep file", FileChoice::KeepFile),
                    ("Delete file", FileChoice::DeleteFile),
                ],
            );
        }
        ConflictPresentation::ResolveAsDeleted => {
            root.append(&hint_label("Both sides deleted this file."));
            append_file_actions(
                &root,
                session,
                callbacks,
                &[("Resolve as deleted", FileChoice::ResolveAsDeleted)],
            );
        }
        ConflictPresentation::WholeFileChoice {
            version_a,
            version_b,
        } => {
            root.append(&hint_label(
                "This conflict cannot be merged as text. Choose one complete version.",
            ));
            for (title, version) in [(&labels.a, version_a), (&labels.b, version_b)] {
                let caption = format!("{} · {} bytes", title, version.bytes.len());
                let (panel, target) = source_panel(
                    &caption,
                    &version.bytes,
                    &session.path,
                    highlighter,
                    &[],
                    None,
                );
                root.append(&panel);
                targets.push(target);
            }
            append_file_actions(
                &root,
                session,
                callbacks,
                &[
                    (session.load.context.use_a_action(), FileChoice::UseVersionA),
                    (session.load.context.use_b_action(), FileChoice::UseVersionB),
                ],
            );
        }
        ConflictPresentation::AlreadyManuallyResolved => {
            root.append(&hint_label(
                "No conflict markers remain in the working tree.\nReview the current file and mark it as resolved when ready.",
            ));
            let mark = Button::with_label("Mark current file as resolved");
            mark.add_css_class("suggested-action");
            let callbacks = callbacks.clone();
            mark.connect_clicked(move |_| (callbacks.mark_resolved)());
            root.append(&mark);
        }
        ConflictPresentation::Unsupported(reason) => {
            root.append(&hint_label(&format!(
                "{reason}\nThis conflict is not modified automatically; resolve it externally and refresh."
            )));
            if let Some(content) = &session.load.working_tree {
                let text = String::from_utf8_lossy(content);
                let (panel, target) = source_panel(
                    "Working tree · read-only",
                    text.as_bytes(),
                    &session.path,
                    highlighter,
                    &[],
                    None,
                );
                root.append(&panel);
                targets.push(target);
            }
            let refresh = Button::with_label("Refresh");
            let callbacks = callbacks.clone();
            refresh.connect_clicked(move |_| (callbacks.refresh)());
            root.append(&refresh);
        }
        ConflictPresentation::TextBlocks(_) => {}
    }
    (root.upcast(), targets)
}

/// Choice buttons plus the final apply for file-level presentations (§48-51).
fn append_file_actions(
    root: &GtkBox,
    session: &ConflictSession,
    callbacks: &Rc<Callbacks>,
    choices: &[(&str, FileChoice)],
) {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let apply_button = Button::with_label("Apply resolution and mark resolved");
    apply_button.add_css_class("suggested-action");
    apply_button.set_sensitive(session.file_choice != FileChoice::None);
    for &(label, choice) in choices {
        let button = Button::with_label(label);
        let callbacks = callbacks.clone();
        let apply_button = apply_button.clone();
        button.connect_clicked(move |_| {
            (callbacks.choose_file)(choice);
            apply_button.set_sensitive(true);
        });
        row.append(&button);
    }
    root.append(&row);
    let apply_callbacks = callbacks.clone();
    apply_button.connect_clicked(move |_| (apply_callbacks.apply)());
    root.append(&apply_button);
}
