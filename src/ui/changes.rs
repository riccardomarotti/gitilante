//! Sidebar with the Staged, Unstaged and Untracked file lists (SPEC section 5).
//!
//! Each row shows the file name with its change letter and the whole-file
//! actions of its section (SPEC section 15): Stage, Unstage, Discard.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, ListBoxRow, Orientation, SelectionMode};

use crate::model::status::{Status, StatusEntry};
use crate::ui::clipboard::{self, CopyAction};
use crate::ui::{Selection, change_letter, display_name};

/// User actions available from the sidebar.
pub struct Callbacks {
    pub copy: Box<dyn Fn(CopyAction)>,
    pub open_editor: Box<dyn Fn(PathBuf)>,
    /// A row was selected.
    pub select: Box<dyn Fn(Selection)>,
    /// Stage every change of the given path.
    pub stage_file: Box<dyn Fn(PathBuf)>,
    /// Unstage every change of the given path.
    pub unstage_file: Box<dyn Fn(PathBuf)>,
    /// Discard the working tree changes of the given path (the caller asks for
    /// confirmation first).
    pub discard_file: Box<dyn Fn(PathBuf)>,
}

/// Whole-file action of a row button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Stage,
    Unstage,
    Discard,
}

/// State shared with the list's signal handlers.
struct Shared {
    /// Selection of each row, aligned with `gtk::ListBoxRow::index`.
    items: RefCell<Vec<Option<Selection>>>,
    /// True while the list is being rebuilt, to ignore selection events.
    rebuilding: Cell<bool>,
    callbacks: Callbacks,
}

/// The Changes sidebar.
pub struct ChangesView {
    list: ListBox,
    shared: Rc<Shared>,
}

impl ChangesView {
    pub fn new(callbacks: Callbacks) -> Self {
        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Single);
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(true);

        let shared = Rc::new(Shared {
            items: RefCell::new(Vec::new()),
            rebuilding: Cell::new(false),
            callbacks,
        });

        {
            let shared = shared.clone();
            list.connect_selected_rows_changed(move |list| {
                if shared.rebuilding.get() {
                    return;
                }
                let Some(row) = list.selected_row() else {
                    return;
                };
                let selection = shared
                    .items
                    .borrow()
                    .get(row.index() as usize)
                    .cloned()
                    .flatten();
                if let Some(selection) = selection {
                    (shared.callbacks.select)(selection);
                }
            });
        }

        Self { list, shared }
    }

    /// The sidebar widget.
    pub fn widget(&self) -> &gtk4::Widget {
        self.list.upcast_ref()
    }

    /// Rebuilds the list from `status`, keeping `selection` when it still exists.
    pub fn update(&self, status: &Status, selection: Option<&Selection>) {
        self.shared.rebuilding.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            clipboard::detach_context_menu(&row);
            self.list.remove(&row);
        }
        let mut items = self.shared.items.borrow_mut();
        items.clear();

        self.push_header("Staged");
        items.push(None);
        for entry in status.staged_entries() {
            let letter = entry.staged_change().map(change_letter).unwrap_or(' ');
            items.push(Some(Selection::Staged(entry.path.clone())));
            self.push_entry(entry, letter, &[("Unstage", Op::Unstage)]);
        }

        self.push_header("Unstaged");
        items.push(None);
        for entry in status.unstaged_entries() {
            let letter = entry.unstaged_change().map(change_letter).unwrap_or(' ');
            items.push(Some(Selection::Unstaged(entry.path.clone())));
            self.push_entry(
                entry,
                letter,
                &[("Stage", Op::Stage), ("Discard", Op::Discard)],
            );
        }

        self.push_header("Conflicted");
        items.push(None);
        for entry in status.unmerged_entries() {
            items.push(Some(Selection::Conflicted(entry.path.clone())));
            // Conflict resolution is out of scope (SPEC sections 25 and 31):
            // conflicted files are visible but read-only.
            self.push_entry(entry, 'U', &[]);
        }

        self.push_header("Untracked");
        items.push(None);
        for entry in status.untracked_entries() {
            items.push(Some(Selection::Untracked(entry.path.clone())));
            self.push_entry(entry, '?', &[("Stage", Op::Stage)]);
        }

        // Restore the selection on the matching row, if any.
        if let Some(selection) = selection {
            for (index, item) in items.iter().enumerate() {
                if item.as_ref() == Some(selection) {
                    self.list
                        .select_row(self.list.row_at_index(index as i32).as_ref());
                    break;
                }
            }
        } else {
            self.list.unselect_all();
        }

        drop(items);
        self.shared.rebuilding.set(false);
    }

    /// Clears the row selection (used when a History commit is selected).
    pub fn clear_selection(&self) {
        self.shared.rebuilding.set(true);
        self.list.unselect_all();
        self.shared.rebuilding.set(false);
    }

    /// Appends a non-selectable section header row.
    fn push_header(&self, title: &str) {
        let row = ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        let label = Label::new(Some(title));
        label.add_css_class("heading");
        label.set_xalign(0.0);
        label.set_margin_top(12);
        label.set_margin_bottom(4);
        label.set_margin_start(6);
        row.set_child(Some(&label));
        self.list.append(&row);
    }

    /// Appends a file row with its action buttons.
    fn push_entry(&self, entry: &StatusEntry, letter: char, buttons: &[(&str, Op)]) {
        let row = ListBoxRow::new();
        row.set_activatable(true);

        let box_ = GtkBox::new(Orientation::Horizontal, 6);
        box_.set_margin_top(4);
        box_.set_margin_bottom(4);
        box_.set_margin_start(6);
        box_.set_margin_end(6);

        let letter_label = Label::new(Some(&letter.to_string()));
        letter_label.add_css_class("dim-label");
        letter_label.set_width_chars(1);
        letter_label.set_xalign(0.5);
        box_.append(&letter_label);

        let name = Label::new(Some(&display_name(&entry.path, entry.orig_path.as_deref())));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(pango::EllipsizeMode::Middle);
        box_.append(&name);

        for (label, op) in buttons {
            let button = Button::with_label(label);
            button.add_css_class("flat");
            button.set_valign(Align::Center);
            button.set_tooltip_text(Some(&tooltip(*op, &entry.path)));
            let path = entry.path.clone();
            let shared = self.shared.clone();
            let op = *op;
            button.connect_clicked(move |_| match op {
                Op::Stage => (shared.callbacks.stage_file)(path.clone()),
                Op::Unstage => (shared.callbacks.unstage_file)(path.clone()),
                Op::Discard => (shared.callbacks.discard_file)(path.clone()),
            });
            box_.append(&button);
        }

        row.set_child(Some(&box_));
        let path = entry.path.clone();
        let absolute_path = path.clone();
        let shared = self.shared.clone();
        let absolute_shared = self.shared.clone();
        let editor_shared = self.shared.clone();
        let editor_path = entry.path.clone();
        clipboard::context_menu(
            &row,
            vec![
                (
                    "Open in Editor",
                    Box::new(move || (editor_shared.callbacks.open_editor)(editor_path.clone())),
                ),
                (
                    "Copy path",
                    Box::new(move || (shared.callbacks.copy)(CopyAction::Path(path.clone()))),
                ),
                (
                    "Copy absolute path",
                    Box::new(move || {
                        (absolute_shared.callbacks.copy)(CopyAction::AbsolutePath(
                            absolute_path.clone(),
                        ))
                    }),
                ),
            ],
        );
        self.list.append(&row);
    }
}

fn tooltip(op: Op, path: &std::path::Path) -> String {
    let name = display_name(path, None);
    match op {
        Op::Stage => format!("Stage all changes of {name}"),
        Op::Unstage => format!("Unstage all changes of {name}"),
        Op::Discard => format!("Discard the working tree changes of {name}"),
    }
}
