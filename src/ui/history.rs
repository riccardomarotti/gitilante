//! History list in the sidebar (SPEC sections 16 and 17).
//!
//! Commits are appended in blocks as the user scrolls; each row shows the
//! short object name, the subject, the author and a relative date.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, ListBox, ListBoxRow, Orientation, SelectionMode};

use crate::model::commit::Commit;
use crate::ui::Selection;

/// User actions available from the History list.
pub struct Callbacks {
    /// A commit was selected.
    pub select: Box<dyn Fn(Selection)>,
    /// The user asked for the next block of commits.
    pub load_more: Box<dyn Fn()>,
}

/// State shared with the list's signal handlers.
struct Shared {
    /// Selection of each row, aligned with `gtk::ListBoxRow::index`.
    items: RefCell<Vec<Option<Selection>>>,
    /// True while the list is being rebuilt, to ignore selection events.
    rebuilding: Cell<bool>,
    callbacks: Callbacks,
}

/// The History view: section header and commit list.
pub struct HistoryView {
    root: GtkBox,
    list: ListBox,
    shared: Rc<Shared>,
}

impl HistoryView {
    pub fn new(callbacks: Callbacks) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        let header = Label::new(Some("History"));
        header.add_css_class("heading");
        header.set_xalign(0.0);
        header.set_margin_top(12);
        header.set_margin_bottom(4);
        header.set_margin_start(6);
        root.append(&header);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Single);
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(true);
        root.append(&list);

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

        Self { root, list, shared }
    }

    /// The view widget (header included).
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Clears the row selection (used when a Changes entry is selected).
    pub fn clear_selection(&self) {
        self.shared.rebuilding.set(true);
        self.list.unselect_all();
        self.shared.rebuilding.set(false);
    }

    /// Rebuilds the list from the commits loaded so far.
    ///
    /// `loading` and `exhausted` control the trailing row that offers the next
    /// block of commits.
    pub fn update(
        &self,
        commits: &[Commit],
        selection: Option<&Selection>,
        loading: bool,
        exhausted: bool,
    ) {
        self.shared.rebuilding.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut items = self.shared.items.borrow_mut();
        items.clear();

        for commit in commits {
            self.push_commit(commit);
            items.push(Some(Selection::Commit(commit.oid.clone())));
        }

        if !exhausted {
            self.push_footer(loading);
            items.push(None);
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
        }

        drop(items);
        self.shared.rebuilding.set(false);
    }

    /// Appends one commit row.
    fn push_commit(&self, commit: &Commit) {
        let row = ListBoxRow::new();
        row.set_activatable(true);

        let box_ = GtkBox::new(Orientation::Vertical, 2);
        box_.set_margin_top(6);
        box_.set_margin_bottom(6);
        box_.set_margin_start(6);
        box_.set_margin_end(6);

        let title = GtkBox::new(Orientation::Horizontal, 8);
        let oid = Label::new(Some(commit.short_oid()));
        oid.add_css_class("monospace");
        oid.add_css_class("dim-label");
        title.append(&oid);
        let subject = Label::new(Some(&commit.subject));
        subject.set_xalign(0.0);
        subject.set_hexpand(true);
        subject.set_ellipsize(pango::EllipsizeMode::End);
        title.append(&subject);
        box_.append(&title);

        let detail = Label::new(Some(&format!(
            "{} · {}",
            commit.author_name,
            relative_time(commit.author_time)
        )));
        detail.add_css_class("dim-label");
        detail.add_css_class("caption");
        detail.set_xalign(0.0);
        detail.set_ellipsize(pango::EllipsizeMode::End);
        box_.append(&detail);

        row.set_child(Some(&box_));
        self.list.append(&row);
    }

    /// Appends the trailing row offering the next block of commits.
    fn push_footer(&self, loading: bool) {
        let row = ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);

        let box_ = GtkBox::new(Orientation::Horizontal, 6);
        box_.set_margin_top(6);
        box_.set_margin_bottom(6);
        box_.set_margin_start(6);
        box_.set_margin_end(6);

        if loading {
            let label = Label::new(Some("Loading…"));
            label.add_css_class("dim-label");
            box_.append(&label);
        } else {
            let button = Button::with_label("Load more commits");
            button.add_css_class("flat");
            let shared = self.shared.clone();
            button.connect_clicked(move |_| (shared.callbacks.load_more)());
            box_.append(&button);
        }

        row.set_child(Some(&box_));
        self.list.append(&row);
    }
}

/// Formats a Unix timestamp as a relative date ("3 days ago").
fn relative_time(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(timestamp);
    let seconds = (now - timestamp).max(0);
    let (value, unit) = match seconds {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86_399 => (seconds / 3600, "hour"),
        86_400..=2_591_999 => (seconds / 86_400, "day"),
        2_592_000..=31_557_599 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_557_600, "year"),
    };
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}
