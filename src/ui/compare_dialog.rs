//! Revision comparison dialog. Git work is delegated to the main window.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Key;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Entry, Label, ListBox, ListBoxRow, Orientation, PolicyType,
    ScrolledWindow,
};
use libadwaita as adw;
use libadwaita::prelude::AdwDialogExt;

use crate::model::revisions::RevisionCandidate;

/// Actions delegated to the main window.
pub struct Callbacks {
    /// Starts a comparison request.
    pub compare: Box<dyn Fn(String, String)>,
    /// Invalidates a request when the dialog closes.
    pub invalidate: Box<dyn Fn()>,
    /// Loads cached or fresh revision candidates.
    pub load_candidates: Box<dyn Fn()>,
    /// Suspends/restores the application's single-key hunk shortcuts.
    pub on_open: Box<dyn Fn(bool)>,
}

struct Shared {
    dialog: adw::Dialog,
    from: Entry,
    to: Entry,
    from_list: ListBox,
    to_list: ListBox,
    from_matches: RefCell<Vec<RevisionCandidate>>,
    to_matches: RefCell<Vec<RevisionCandidate>>,
    candidates: RefCell<Vec<RevisionCandidate>>,
    status: Label,
    callbacks: Callbacks,
}

/// The revision comparison dialog.
pub struct CompareDialog {
    shared: Rc<Shared>,
}

impl CompareDialog {
    /// Creates the dialog, initially hidden, with `To = HEAD`.
    pub fn new(callbacks: Callbacks) -> Self {
        let dialog = adw::Dialog::new();
        <adw::Dialog as AdwDialogExt>::set_title(&dialog, "Compare revisions");
        <adw::Dialog as AdwDialogExt>::set_content_width(&dialog, 520);
        <adw::Dialog as AdwDialogExt>::set_content_height(&dialog, 480);

        let content = GtkBox::new(Orientation::Vertical, 10);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);

        let from = Entry::new();
        from.set_placeholder_text(Some("Branch, tag, SHA, HEAD~1…"));
        from.set_hexpand(true);
        let from_list = ListBox::new();
        from_list.set_selection_mode(gtk4::SelectionMode::Single);
        from_list.set_activate_on_single_click(true);
        from_list.add_css_class("boxed-list");
        from_list.set_visible(false);
        content.append(&revision_field("From", &from, &from_list));

        let to = Entry::new();
        to.set_text("HEAD");
        to.set_placeholder_text(Some("Branch, tag, SHA, HEAD~1…"));
        to.set_hexpand(true);
        let to_list = ListBox::new();
        to_list.set_selection_mode(gtk4::SelectionMode::Single);
        to_list.set_activate_on_single_click(true);
        to_list.add_css_class("boxed-list");
        to_list.set_visible(false);
        content.append(&revision_field("To", &to, &to_list));

        let swap_row = GtkBox::new(Orientation::Horizontal, 0);
        let swap = Button::with_label("Swap");
        swap.add_css_class("flat");
        swap_row.append(&swap);
        content.append(&swap_row);

        let spacer = GtkBox::new(Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        content.append(&spacer);

        let status = Label::new(None);
        status.set_xalign(0.0);
        status.add_css_class("caption");
        status.add_css_class("dim-label");
        content.append(&status);

        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_halign(gtk4::Align::End);
        let cancel = Button::with_label("Cancel");
        let compare = Button::with_label("Compare");
        compare.add_css_class("suggested-action");
        footer.append(&cancel);
        footer.append(&compare);
        content.append(&footer);

        <adw::Dialog as AdwDialogExt>::set_child(&dialog, Some(&content));
        let shared = Rc::new(Shared {
            dialog: dialog.clone(),
            from: from.clone(),
            to: to.clone(),
            from_list: from_list.clone(),
            to_list: to_list.clone(),
            from_matches: RefCell::new(Vec::new()),
            to_matches: RefCell::new(Vec::new()),
            candidates: RefCell::new(Vec::new()),
            status,
            callbacks,
        });

        for (entry, is_from) in [(&from, true), (&to, false)] {
            let weak = Rc::downgrade(&shared);
            entry.connect_changed(move |_| {
                if let Some(shared) = weak.upgrade() {
                    clear_status(&shared);
                    update_suggestions(&shared, is_from);
                }
            });

            let focus = gtk4::EventControllerFocus::new();
            let weak = Rc::downgrade(&shared);
            focus.connect_enter(move |_| {
                if let Some(shared) = weak.upgrade() {
                    update_suggestions(&shared, is_from);
                }
            });
            entry.add_controller(focus);

            let keys = gtk4::EventControllerKey::new();
            keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let weak = Rc::downgrade(&shared);
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(shared) = weak.upgrade() else {
                    return gtk4::glib::Propagation::Proceed;
                };
                match key {
                    Key::Down => {
                        move_candidate_selection(&shared, is_from, 1);
                        gtk4::glib::Propagation::Stop
                    }
                    Key::Up => {
                        move_candidate_selection(&shared, is_from, -1);
                        gtk4::glib::Propagation::Stop
                    }
                    _ => gtk4::glib::Propagation::Proceed,
                }
            });
            entry.add_controller(keys);
        }

        for (list, is_from) in [(&from_list, true), (&to_list, false)] {
            let weak = Rc::downgrade(&shared);
            list.connect_row_activated(move |_, row| {
                if let Some(shared) = weak.upgrade() {
                    choose_candidate(&shared, is_from, row.index());
                }
            });
        }

        {
            let weak = Rc::downgrade(&shared);
            swap.connect_clicked(move |_| {
                if let Some(shared) = weak.upgrade() {
                    let from_text = shared.from.text().to_string();
                    let to_text = shared.to.text().to_string();
                    shared.from.set_text(&to_text);
                    shared.to.set_text(&from_text);
                    clear_status(&shared);
                    update_suggestions(&shared, true);
                    update_suggestions(&shared, false);
                }
            });
        }
        {
            let weak = Rc::downgrade(&shared);
            compare.connect_clicked(move |_| {
                if let Some(shared) = weak.upgrade() {
                    submit(&shared);
                }
            });
        }
        {
            let weak = Rc::downgrade(&shared);
            cancel.connect_clicked(move |_| {
                if let Some(shared) = weak.upgrade() {
                    close_dialog(&shared);
                }
            });
        }

        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(&shared);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(shared) = weak.upgrade() else {
                return gtk4::glib::Propagation::Proceed;
            };
            match key {
                Key::Escape => {
                    let from_focused = shared.from.has_focus() || shared.from_list.has_focus();
                    let list = suggestions(&shared, from_focused);
                    if list.is_visible() {
                        list.set_visible(false);
                        list.unselect_all();
                    } else {
                        close_dialog(&shared);
                    }
                    gtk4::glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    let from = shared.from.has_focus() || shared.from_list.has_focus();
                    let selected = suggestions(&shared, from).selected_row();
                    if let Some(row) = selected {
                        choose_candidate(&shared, from, row.index());
                    } else {
                        submit(&shared);
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            }
        });
        content.add_controller(keys);

        {
            let shared = shared.clone();
            dialog.connect_closed(move |_| {
                (shared.callbacks.invalidate)();
                (shared.callbacks.on_open)(false);
            });
        }

        Self { shared }
    }

    /// Presents the dialog; optional values replace the current fields.
    pub fn present(&self, parent: &impl IsA<gtk4::Widget>, values: Option<(String, String)>) {
        if let Some((from, to)) = values {
            self.shared.from.set_text(&from);
            self.shared.to.set_text(&to);
        }
        clear_status(&self.shared);
        (self.shared.callbacks.on_open)(true);
        (self.shared.callbacks.load_candidates)();
        <adw::Dialog as AdwDialogExt>::present(&self.shared.dialog, Some(parent));
        self.shared.from.grab_focus();
        self.shared.from.select_region(0, -1);
        update_suggestions(&self.shared, true);
    }

    /// Closes the dialog after a successful comparison.
    pub fn close(&self) {
        close_dialog(&self.shared);
    }

    /// Whether the dialog is currently visible.
    pub fn is_open(&self) -> bool {
        self.shared.dialog.is_visible()
    }

    /// Replaces suggestions without changing the arbitrary text fields.
    pub fn set_candidates(&self, candidates: Vec<RevisionCandidate>) {
        *self.shared.candidates.borrow_mut() = candidates;
        update_suggestions(&self.shared, true);
        update_suggestions(&self.shared, false);
    }

    /// Shows request progress in the dialog.
    pub fn show_comparing(&self) {
        show_comparing(&self.shared);
    }

    /// Shows a short inline validation or Git error.
    pub fn show_error(&self, message: &str) {
        show_error(&self.shared, message);
    }
}

fn revision_field(label: &str, entry: &Entry, suggestions: &ListBox) -> GtkBox {
    let field = GtkBox::new(Orientation::Vertical, 4);
    let title = Label::new(Some(label));
    title.set_xalign(0.0);
    title.add_css_class("heading");
    field.append(&title);
    field.append(entry);
    let scroll = ScrolledWindow::new();
    scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(180);
    scroll.set_child(Some(suggestions));
    field.append(&scroll);
    field
}

fn close_dialog(shared: &Shared) {
    (shared.callbacks.invalidate)();
    <adw::Dialog as AdwDialogExt>::close(&shared.dialog);
}

fn clear_status(shared: &Shared) {
    shared.status.remove_css_class("error");
    shared.status.add_css_class("dim-label");
    shared.status.set_text("");
}

fn show_comparing(shared: &Shared) {
    shared.status.remove_css_class("error");
    shared.status.add_css_class("dim-label");
    shared.status.set_text("Comparing…");
}

fn show_error(shared: &Shared, message: &str) {
    shared.status.remove_css_class("dim-label");
    shared.status.add_css_class("error");
    shared.status.set_text(message);
}

fn suggestions(shared: &Shared, is_from: bool) -> &ListBox {
    if is_from {
        &shared.from_list
    } else {
        &shared.to_list
    }
}

fn entry(shared: &Shared, is_from: bool) -> &Entry {
    if is_from { &shared.from } else { &shared.to }
}

fn matches(shared: &Shared, is_from: bool) -> &RefCell<Vec<RevisionCandidate>> {
    if is_from {
        &shared.from_matches
    } else {
        &shared.to_matches
    }
}

fn update_suggestions(shared: &Rc<Shared>, is_from: bool) {
    let list = suggestions(shared, is_from);
    if entry(shared, is_from).has_focus() {
        suggestions(shared, !is_from).set_visible(false);
    }
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let query = entry(shared, is_from).text().to_string().to_lowercase();
    let filtered: Vec<RevisionCandidate> = shared
        .candidates
        .borrow()
        .iter()
        .filter(|candidate| {
            query.is_empty()
                || candidate.label.to_lowercase().contains(&query)
                || candidate.spec.to_lowercase().contains(&query)
        })
        .take(10)
        .cloned()
        .collect();
    for candidate in &filtered {
        let row = ListBoxRow::new();
        let line = GtkBox::new(Orientation::Horizontal, 8);
        line.set_margin_top(4);
        line.set_margin_bottom(4);
        line.set_margin_start(8);
        line.set_margin_end(8);
        let label = Label::new(Some(&candidate.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        line.append(&label);
        if candidate.kind != crate::model::revisions::RevisionKind::Head {
            let kind = Label::new(Some(candidate.kind.label()));
            kind.add_css_class("dim-label");
            kind.add_css_class("caption");
            line.append(&kind);
        }
        row.set_child(Some(&line));
        list.append(&row);
    }
    *matches(shared, is_from).borrow_mut() = filtered;
    list.set_visible(entry(shared, is_from).has_focus() && list.row_at_index(0).is_some());
}

fn choose_candidate(shared: &Rc<Shared>, is_from: bool, index: i32) {
    let candidate = matches(shared, is_from)
        .borrow()
        .get(index as usize)
        .cloned();
    let Some(candidate) = candidate else { return };
    entry(shared, is_from).set_text(&candidate.spec);
    entry(shared, is_from).grab_focus();
    suggestions(shared, is_from).set_visible(false);
    clear_status(shared);
}

fn move_candidate_selection(shared: &Rc<Shared>, is_from: bool, direction: i32) {
    update_suggestions(shared, is_from);
    let list = suggestions(shared, is_from);
    if list.row_at_index(0).is_none() {
        return;
    }
    list.set_visible(true);
    let count = matches(shared, is_from).borrow().len() as i32;
    let selected = list
        .selected_row()
        .map(|row| row.index())
        .unwrap_or(if direction > 0 { -1 } else { count });
    let next = (selected + direction).clamp(0, count.saturating_sub(1));
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
    }
}

fn submit(shared: &Rc<Shared>) {
    // A new submit intent invalidates any older worker, even when validation
    // rejects this input before a new Git task is started.
    (shared.callbacks.invalidate)();
    let from = shared.from.text().trim().to_owned();
    shared.from_list.set_visible(false);
    shared.to_list.set_visible(false);
    if from.is_empty() {
        show_error(shared, "Enter a From revision");
        shared.from.grab_focus();
        return;
    }
    let to = shared.to.text().trim().to_owned();
    if to.is_empty() {
        show_error(shared, "Enter a To revision");
        shared.to.grab_focus();
        return;
    }
    show_comparing(shared);
    (shared.callbacks.compare)(from, to);
}
