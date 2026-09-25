//! Global search UI: the Ctrl+Shift+F dialog
//! (GITILANTE_SEARCH_SPEC.md sections 5, 31-38 and 51).
//!
//! The dialog is a dumb view: it debounces the query, then hands it to the
//! caller, which runs the providers on the worker thread and delivers the
//! results back with `show_results`. A stale search can never overwrite newer
//! results because the caller owns the generation counter (section 35).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk::Key;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow, SearchEntry,
    ToggleButton,
};
use libadwaita as adw;
use libadwaita::prelude::AdwDialogExt;

use crate::search::result::{MatchRange, SearchResult};
use crate::search::{CaseMode, ChangeFilter, SearchQuery, SearchScope};

/// Delay between the last keystroke and the search (section 33).
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// A generation belongs to the user's search intent, not the worker start.
#[derive(Default)]
pub(crate) struct SearchGeneration(Cell<u64>);

impl SearchGeneration {
    pub fn invalidate(&self) {
        self.0.set(self.0.get().wrapping_add(1));
    }

    pub fn current(&self) -> u64 {
        self.0.get()
    }

    pub fn accepts(&self, generation: u64) -> bool {
        self.current() == generation
    }
}

/// User actions available from the dialog.
pub struct Callbacks {
    /// Run the query (on the worker thread, section 52).
    pub search: Box<dyn Fn(SearchQuery)>,
    /// Invalidates in-flight results as soon as the search intent changes.
    pub invalidate: Box<dyn Fn()>,
    /// A result was activated with its query (navigation, sections 12 and 30).
    pub activate: Box<dyn Fn(SearchResult, SearchQuery)>,
    /// Told when the dialog opens/closes, to suspend the single-key shortcuts
    /// (section 51).
    pub on_open: Box<dyn Fn(bool)>,
}

/// State shared with the dialog's signal handlers.
struct Shared {
    entry: SearchEntry,
    status: Label,
    list: ListBox,
    filter_box: GtkBox,
    query: RefCell<SearchQuery>,
    /// One entry per list row; `None` for group headers.
    rows: RefCell<Vec<Option<SearchResult>>>,
    debounce: Cell<u64>,
    callbacks: Callbacks,
}

/// The global search dialog.
pub struct SearchDialog {
    dialog: adw::Dialog,
    shared: Rc<Shared>,
}

impl SearchDialog {
    /// Creates the dialog (not shown until `present`).
    pub fn new(callbacks: Callbacks) -> Self {
        let dialog = adw::Dialog::new();
        <adw::Dialog as AdwDialogExt>::set_title(&dialog, "Search repository");
        <adw::Dialog as AdwDialogExt>::set_content_width(&dialog, 640);
        <adw::Dialog as AdwDialogExt>::set_content_height(&dialog, 480);

        let content = GtkBox::new(Orientation::Vertical, 8);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let entry = SearchEntry::new();
        entry.set_placeholder_text(Some("Search repository…"));
        content.append(&entry);

        // Matching options (GITILANTE_SEARCH_SPEC.md sections 8 and 49).
        let options = GtkBox::new(Orientation::Horizontal, 6);
        let regex_toggle = ToggleButton::with_label(".*");
        regex_toggle.set_tooltip_text(Some("Regular expression"));
        options.append(&regex_toggle);
        let case_button = Button::with_label("Aa");
        case_button.set_tooltip_text(Some("Smart case — click to change"));
        options.append(&case_button);
        content.append(&options);

        // Scope row (section 2). History arrives with its providers
        // (GITILANTE_SEARCH_SPEC.md sections 79).
        let scopes = GtkBox::new(Orientation::Horizontal, 0);
        scopes.add_css_class("linked");
        let scope_all = ToggleButton::with_label("All");
        let scope_changes = ToggleButton::with_label("Changes");
        let scope_files = ToggleButton::with_label("Files");
        let scope_contents = ToggleButton::with_label("Contents");
        let scope_history = ToggleButton::with_label("History");
        let scope_history_changes = ToggleButton::with_label("History Changes");
        scope_changes.set_group(Some(&scope_all));
        scope_files.set_group(Some(&scope_all));
        scope_contents.set_group(Some(&scope_all));
        scope_history.set_group(Some(&scope_all));
        scope_history_changes.set_group(Some(&scope_all));
        scope_all.set_active(true);
        scopes.append(&scope_all);
        scopes.append(&scope_changes);
        scopes.append(&scope_files);
        scopes.append(&scope_contents);
        scopes.append(&scope_history);
        scopes.append(&scope_history_changes);
        content.append(&scopes);

        // Added/Removed filter, shown only for the Changes scope (section 10).
        let filter_box = GtkBox::new(Orientation::Horizontal, 0);
        filter_box.add_css_class("linked");
        let filter_both = ToggleButton::with_label("Added + Removed");
        let filter_added = ToggleButton::with_label("Added only");
        let filter_removed = ToggleButton::with_label("Removed only");
        filter_added.set_group(Some(&filter_both));
        filter_removed.set_group(Some(&filter_both));
        filter_both.set_active(true);
        filter_box.append(&filter_both);
        filter_box.append(&filter_added);
        filter_box.append(&filter_removed);
        content.append(&filter_box);

        let list = ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.set_activate_on_single_click(false);
        list.add_css_class("navigation-sidebar");
        let scroll = ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        content.append(&scroll);

        let status = Label::new(None);
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_xalign(0.0);
        content.append(&status);

        let shared = Rc::new(Shared {
            entry: entry.clone(),
            status,
            list: list.clone(),
            filter_box: filter_box.clone(),
            query: RefCell::new(SearchQuery::default()),
            rows: RefCell::new(Vec::new()),
            debounce: Cell::new(0),
            callbacks,
        });

        // Query typing is debounced (section 33).
        {
            let shared = shared.clone();
            entry.connect_search_changed(move |entry| {
                shared.query.borrow_mut().text = entry.text().to_string();
                search_changed(&shared, true);
            });
        }
        {
            // Enter is consumed by the entry: open the selected result
            // (GITILANTE_SEARCH_SPEC.md section 51).
            let shared = shared.clone();
            entry.connect_activate(move |_| activate_selected(&shared));
        }
        {
            // Esc is consumed by the entry too: it emits stop-search.
            let dialog_weak = dialog.downgrade();
            entry.connect_stop_search(move |_| {
                if let Some(dialog) = dialog_weak.upgrade() {
                    <adw::Dialog as AdwDialogExt>::close(&dialog);
                }
            });
        }

        // Scope and filter changes run immediately.
        {
            let shared = shared.clone();
            regex_toggle.connect_toggled(move |button| {
                shared.query.borrow_mut().regex = button.is_active();
                search_changed(&shared, false);
            });
        }
        {
            let shared = shared.clone();
            case_button.connect_clicked(move |button| {
                let mode = {
                    let mut query = shared.query.borrow_mut();
                    query.case_mode = match query.case_mode {
                        CaseMode::Smart => CaseMode::Sensitive,
                        CaseMode::Sensitive => CaseMode::Insensitive,
                        CaseMode::Insensitive => CaseMode::Smart,
                    };
                    query.case_mode
                };
                button.set_tooltip_text(Some(match mode {
                    CaseMode::Smart => "Smart case — click to change",
                    CaseMode::Sensitive => "Match case — click to change",
                    CaseMode::Insensitive => "Ignore case — click to change",
                }));
                search_changed(&shared, false);
            });
        }
        {
            let shared = shared.clone();
            scope_all.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::All;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        {
            let shared = shared.clone();
            scope_changes.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::Changes;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        {
            let shared = shared.clone();
            scope_files.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::Files;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        {
            let shared = shared.clone();
            scope_contents.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::Contents;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        {
            let shared = shared.clone();
            scope_history.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::History;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        {
            // History Changes is explicit-only: it never runs in All
            // (GITILANTE_SEARCH_SPEC.md section 28).
            let shared = shared.clone();
            scope_history_changes.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().scope = SearchScope::HistoryChanges;
                    update_filter_visibility(&shared);
                    search_changed(&shared, false);
                }
            });
        }
        for (button, filter) in [
            (&filter_both, ChangeFilter::AddedAndRemoved),
            (&filter_added, ChangeFilter::AddedOnly),
            (&filter_removed, ChangeFilter::RemovedOnly),
        ] {
            let shared = shared.clone();
            button.connect_toggled(move |button| {
                if button.is_active() {
                    shared.query.borrow_mut().change_filter = filter;
                    search_changed(&shared, false);
                }
            });
        }

        // Keyboard UX (section 51): Up/Down through the results, Enter opens
        // the selected one (or the first), Esc closes.
        {
            let shared = shared.clone();
            let dialog_weak = dialog.downgrade();
            let controller = gtk4::EventControllerKey::new();
            controller.connect_key_pressed(move |_, keyval, _, _| match keyval {
                Key::Down => {
                    move_selection(&shared, 1);
                    gtk4::glib::Propagation::Stop
                }
                Key::Up => {
                    move_selection(&shared, -1);
                    gtk4::glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    activate_selected(&shared);
                    gtk4::glib::Propagation::Stop
                }
                Key::Escape => {
                    if let Some(dialog) = dialog_weak.upgrade() {
                        <adw::Dialog as AdwDialogExt>::close(&dialog);
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            });
            content.add_controller(controller);
        }

        {
            let shared = shared.clone();
            list.connect_row_activated(move |_, row| {
                activate_row(&shared, row.index());
            });
        }

        <adw::Dialog as AdwDialogExt>::set_child(&dialog, Some(&content));
        update_filter_visibility(&shared);

        {
            // Restore the single-key shortcuts when the dialog goes away
            // (GITILANTE_SEARCH_SPEC.md section 51).
            let shared = shared.clone();
            dialog.connect_closed(move |_| {
                cancel_search(&shared);
                (shared.callbacks.on_open)(false);
            });
        }

        Self { dialog, shared }
    }

    /// Shows the dialog over `parent`.
    pub fn present(&self, parent: &impl IsA<gtk4::Widget>) {
        (self.shared.callbacks.on_open)(true);
        cancel_search(&self.shared);
        <adw::Dialog as AdwDialogExt>::present(&self.dialog, Some(parent));
        run_search(&self.shared);
        self.shared.entry.grab_focus();
        self.shared.entry.select_region(0, -1);
    }

    /// Closes the dialog.
    pub fn close(&self) {
        <adw::Dialog as AdwDialogExt>::close(&self.dialog);
    }

    /// True while the dialog is visible.
    pub fn is_open(&self) -> bool {
        self.dialog.is_visible()
    }

    /// Invalidates results before a repository refresh.
    pub fn repository_changed(&self) {
        cancel_search(&self.shared);
    }

    /// Reruns the query after fresh repository data arrives.
    pub fn rerun_if_open(&self) {
        if self.is_open() {
            run_search(&self.shared);
        }
    }

    /// Replaces the result list (GITILANTE_SEARCH_SPEC.md sections 31-32).
    pub fn show_results(&self, results: Vec<SearchResult>) {
        while let Some(row) = self.shared.list.row_at_index(0) {
            self.shared.list.remove(&row);
        }
        let mut rows = self.shared.rows.borrow_mut();
        rows.clear();

        for group in Group::ALL {
            let items: Vec<&SearchResult> = results
                .iter()
                .filter(|result| group.contains(result))
                .collect();
            if items.is_empty() {
                continue;
            }
            self.shared.list.append(&group_header(group.title()));
            rows.push(None);
            for result in items {
                self.shared.list.append(&result_row(result));
                rows.push(Some(result.clone()));
            }
        }

        self.shared.status.set_text(&if results.is_empty() {
            "No results".to_owned()
        } else {
            format!("{} results", results.len())
        });
    }

    /// Reports a provider error near the query field (section 38).
    pub fn show_error(&self, message: &str) {
        self.shared.status.set_text(message);
    }

    /// Clears the results and the status.
    pub fn reset(&self) {
        clear_results(&self.shared);
    }
}

/// Provider groups, in display order (GITILANTE_SEARCH_SPEC.md section 31).
enum Group {
    File,
    Change,
    Content,
    History,
    HistoryChange,
}

impl Group {
    const ALL: [Group; 5] = [
        Group::File,
        Group::Change,
        Group::Content,
        Group::History,
        Group::HistoryChange,
    ];

    fn title(&self) -> &'static str {
        match self {
            Group::File => "FILES",
            Group::Change => "CHANGES",
            Group::Content => "CONTENTS",
            Group::History => "HISTORY",
            Group::HistoryChange => "HISTORY CHANGES",
        }
    }

    fn contains(&self, result: &SearchResult) -> bool {
        matches!(
            (self, result),
            (Group::File, SearchResult::File(_))
                | (Group::Change, SearchResult::Change(_))
                | (Group::Content, SearchResult::Content(_))
                | (Group::History, SearchResult::Commit(_))
                | (Group::HistoryChange, SearchResult::HistoryChange(_))
        )
    }
}

/// Shows the Added/Removed filter only for the Changes scope (section 10).
fn update_filter_visibility(shared: &Rc<Shared>) {
    let visible = shared.query.borrow().scope == SearchScope::Changes;
    shared.filter_box.set_visible(visible);
}

/// Invalidates workers and debounce immediately, then clears stale rows.
fn cancel_search(shared: &Rc<Shared>) {
    shared.debounce.set(shared.debounce.get().wrapping_add(1));
    (shared.callbacks.invalidate)();
    clear_results(shared);
}

fn clear_results(shared: &Shared) {
    while let Some(row) = shared.list.row_at_index(0) {
        shared.list.remove(&row);
    }
    shared.rows.borrow_mut().clear();
    shared.status.set_text("");
}

/// Starts a new intent now; only worker execution is debounced.
fn search_changed(shared: &Rc<Shared>, debounce: bool) {
    cancel_search(shared);
    if shared.query.borrow().is_empty() {
        return;
    }
    shared.status.set_text("Searching…");
    if !debounce {
        run_search(shared);
        return;
    }
    let token = shared.debounce.get();
    let shared = shared.clone();
    gtk4::glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
        if shared.debounce.get() == token {
            run_search(&shared);
        }
    });
}

/// Hands the current query to the caller and shows the searching state.
fn run_search(shared: &Rc<Shared>) {
    let query = shared.query.borrow().clone();
    if query.is_empty() {
        clear_results(shared);
        return;
    }
    shared.status.set_text("Searching…");
    (shared.callbacks.search)(query);
}

/// Moves the selection to the next/previous result row.
fn move_selection(shared: &Rc<Shared>, delta: i32) {
    let rows = shared.rows.borrow();
    let count = rows.len() as i32;
    if count == 0 {
        return;
    }
    let current = shared
        .list
        .selected_row()
        .map(|row| row.index())
        .unwrap_or(-1);
    let mut index = current;
    for _ in 0..count {
        index = (index + delta).rem_euclid(count);
        if rows[index as usize].is_some() {
            shared
                .list
                .select_row(shared.list.row_at_index(index).as_ref());
            return;
        }
    }
}

/// Activates the selected result, or the first one when nothing is selected
/// (GITILANTE_SEARCH_SPEC.md section 51).
fn activate_selected(shared: &Rc<Shared>) {
    if let Some(row) = shared.list.selected_row() {
        activate_row(shared, row.index());
        return;
    }
    let first = first_result_index(&shared.rows);
    if let Some(first) = first {
        activate_row(shared, first as i32);
    }
}

fn first_result_index<T>(rows: &RefCell<Vec<Option<T>>>) -> Option<usize> {
    rows.borrow().iter().position(Option::is_some)
}

fn activate_row(shared: &Rc<Shared>, index: i32) {
    let result = shared.rows.borrow().get(index as usize).cloned().flatten();
    let Some(result) = result else { return };
    let query = shared.query.borrow().clone();
    (shared.callbacks.activate)(result, query);
}

/// A non-selectable group header row (GITILANTE_SEARCH_SPEC.md section 31).
fn group_header(title: &str) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    let label = Label::new(Some(title));
    label.add_css_class("heading");
    label.add_css_class("caption");
    label.set_xalign(0.0);
    label.set_margin_top(8);
    label.set_margin_bottom(2);
    row.set_child(Some(&label));
    row
}

/// A two-line result row with textual badges (section 76).
fn result_row(result: &SearchResult) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_activatable(true);

    let box_ = GtkBox::new(Orientation::Vertical, 2);
    box_.set_margin_top(4);
    box_.set_margin_bottom(4);

    let title = GtkBox::new(Orientation::Horizontal, 6);
    let mut badges: Vec<&str> = Vec::new();
    let (head, detail) = match result {
        SearchResult::Change(change) => {
            badges.push(match change.kind {
                crate::model::diff::DiffLineKind::Addition => "+ added",
                crate::model::diff::DiffLineKind::Deletion => "− removed",
                _ => "",
            });
            badges.push(match change.side {
                crate::ui::DiffSide::Staged => "staged",
                _ => "unstaged",
            });
            (
                change.path.display().to_string(),
                marked_snippet(&change.snippet, &change.match_ranges),
            )
        }
        SearchResult::File(file) => (
            format!("{}  ·  {}", file.name, file.path.display()),
            String::new(),
        ),
        SearchResult::Content(content) => (
            format!("{}:{}", content.path.display(), content.line_number),
            marked_snippet(&content.snippet, &content.match_ranges),
        ),
        SearchResult::Commit(commit) => {
            if !commit.refs.is_empty() {
                badges.push("refs");
            }
            (
                format!("{}  ·  {}", short_oid(&commit.oid), commit.subject),
                commit.author.clone(),
            )
        }
        SearchResult::HistoryChange(change) => (
            format!("{}  ·  {}", short_oid(&change.oid), change.subject),
            change.author.clone(),
        ),
    };

    let head_label = Label::new(Some(&head));
    head_label.set_xalign(0.0);
    head_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    head_label.set_hexpand(true);
    title.append(&head_label);
    for badge in badges {
        if badge.is_empty() {
            continue;
        }
        let badge_label = Label::new(Some(badge));
        badge_label.add_css_class("ref-badge");
        badge_label.add_css_class("caption");
        title.append(&badge_label);
    }
    box_.append(&title);

    if !detail.is_empty() {
        let detail_label = Label::new(None);
        detail_label.set_markup(&detail);
        detail_label.set_xalign(0.0);
        detail_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        detail_label.add_css_class("monospace");
        detail_label.add_css_class("dim-label");
        box_.append(&detail_label);
    }

    row.set_child(Some(&box_));
    row
}

/// Wraps the matched character ranges in a highlight span, escaping the text
/// (GITILANTE_SEARCH_SPEC.md sections 21, 40 and 55).
fn marked_snippet(snippet: &str, ranges: &[MatchRange]) -> String {
    let chars: Vec<char> = snippet.trim_end_matches('\n').chars().take(160).collect();
    let inside = |index: usize| {
        ranges
            .iter()
            .any(|range| index >= range.start && index < range.end)
    };
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let highlight = inside(index);
        let start = index;
        while index < chars.len() && inside(index) == highlight {
            index += 1;
        }
        let fragment: String = chars[start..index].iter().collect();
        let escaped = gtk4::glib::markup_escape_text(&fragment);
        if highlight {
            out.push_str("<span background=\"#fce94f\" background_alpha=\"45%\">");
            out.push_str(&escaped);
            out.push_str("</span>");
        } else {
            out.push_str(&escaped);
        }
    }
    if snippet.trim_end_matches('\n').chars().count() > 160 {
        out.push('…');
    }
    out
}

/// The short object name of a commit.
fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

#[cfg(test)]
mod lifecycle_tests {
    use std::cell::RefCell;

    #[test]
    fn enter_releases_rows_borrow_before_activation_closes_dialog() {
        let rows = RefCell::new(vec![None, Some(42)]);
        if let Some(first) = super::first_result_index(&rows) {
            assert_eq!(first, 1);
            // Closing the dialog clears rows synchronously in the callback.
            rows.borrow_mut().clear();
        }
        assert!(rows.borrow().is_empty());
    }

    use super::SearchGeneration;

    #[test]
    fn older_worker_is_stale_before_new_debounce_expires() {
        let intent = SearchGeneration::default();
        let worker_a = intent.current();
        intent.invalidate(); // Text/scope/options changed; B has not started yet.
        assert!(!intent.accepts(worker_a));
        let worker_b = intent.current();
        assert!(intent.accepts(worker_b));
    }

    #[test]
    fn clearing_query_or_entering_invalid_regex_rejects_old_results() {
        let intent = SearchGeneration::default();
        let worker = intent.current();
        intent.invalidate(); // Empty query, or invalid regex before validation.
        assert!(!intent.accepts(worker));
    }

    #[test]
    fn close_and_reopen_reject_previous_session() {
        let intent = SearchGeneration::default();
        let worker = intent.current();
        intent.invalidate(); // Close.
        intent.invalidate(); // Reopen and rerun.
        assert!(!intent.accepts(worker));
        assert!(intent.accepts(intent.current()));
    }
}
