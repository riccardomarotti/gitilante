//! Search in the current view: the Ctrl+F search bar
//! (GITILANTE_SEARCH_SPEC.md sections 2-4).
//!
//! The bar searches the source text currently rendered in the diff panel,
//! highlights every match, marks the active one and scrolls to it. The
//! highlight is one more visual layer on top of syntax colors, line diff and
//! intraline spans (GITILANTE_SEARCH_SPEC.md section 4): search tags use the
//! text background, which composes with the paragraph background of the line
//! diff and wins over the intraline overlay (the tag created last has the
//! highest priority).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gdk::{self, Key};
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, Orientation, ScrolledWindow, SearchEntry, TextTag, ToggleButton,
};

use crate::search::matcher::{find_matches, find_matches_regex, is_case_sensitive};
use crate::ui::diff_view::SearchTarget;

/// Background of a non-active search match.
fn match_background() -> gdk::RGBA {
    gdk::RGBA::new(1.0, 0.84, 0.0, 0.35)
}

/// Background of the active search match (GITILANTE_SEARCH_SPEC.md section 4).
fn active_background() -> gdk::RGBA {
    gdk::RGBA::new(1.0, 0.62, 0.0, 0.60)
}

/// One highlighted occurrence: character offsets into its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatchLoc {
    target: usize,
    start: i32,
    end: i32,
}

/// One rendered piece of text and its search highlight tags.
struct TargetState {
    target: SearchTarget,
    match_tag: TextTag,
    active_tag: TextTag,
}

/// State shared with the bar's signal handlers.
struct Shared {
    scrolled: ScrolledWindow,
    entry: SearchEntry,
    counter: Label,
    targets: RefCell<Vec<TargetState>>,
    matches: RefCell<Vec<MatchLoc>>,
    active: Cell<usize>,
    query: RefCell<String>,
    /// Regular expression mode (GITILANTE_SEARCH_SPEC.md section 49).
    regex: Cell<bool>,
    /// Told when the bar gains/loses the keyboard focus.
    on_focus: Box<dyn Fn(bool)>,
}

/// The search bar shown above the diff panel.
pub struct SearchBar {
    root: GtkBox,
    shared: Rc<Shared>,
}

impl SearchBar {
    /// Creates the search bar for the content of `scrolled` (initially hidden).
    ///
    /// `on_focus` is called with `true` when the bar takes the keyboard focus
    /// and with `false` when it closes: single-key shortcuts must not intercept
    /// typing (GITILANTE_SEARCH_SPEC.md section 51).
    pub fn new(scrolled: ScrolledWindow, on_focus: Box<dyn Fn(bool)>) -> Self {
        let root = GtkBox::new(Orientation::Horizontal, 6);
        root.set_margin_top(6);
        root.set_margin_bottom(6);
        root.set_margin_start(8);
        root.set_margin_end(8);
        root.set_visible(false);

        let entry = SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Search in current view…"));
        root.append(&entry);

        let regex_toggle = ToggleButton::with_label(".*");
        regex_toggle.set_tooltip_text(Some("Regular expression"));
        root.append(&regex_toggle);

        let counter = Label::new(None);
        counter.add_css_class("dim-label");
        counter.add_css_class("monospace");
        root.append(&counter);

        let previous = Button::from_icon_name("go-up-symbolic");
        previous.set_tooltip_text(Some("Previous match (Shift+Enter)"));
        root.append(&previous);
        let next = Button::from_icon_name("go-down-symbolic");
        next.set_tooltip_text(Some("Next match (Enter)"));
        root.append(&next);
        let close_button = Button::from_icon_name("window-close-symbolic");
        close_button.set_tooltip_text(Some("Close the search bar (Esc)"));
        root.append(&close_button);

        let shared = Rc::new(Shared {
            scrolled,
            entry: entry.clone(),
            counter,
            targets: RefCell::new(Vec::new()),
            matches: RefCell::new(Vec::new()),
            active: Cell::new(0),
            query: RefCell::new(String::new()),
            regex: Cell::new(false),
            on_focus,
        });

        {
            let shared = shared.clone();
            regex_toggle.connect_toggled(move |button| {
                shared.regex.set(button.is_active());
                refresh(&shared);
            });
        }

        {
            let shared = shared.clone();
            entry.connect_search_changed(move |entry| {
                *shared.query.borrow_mut() = entry.text().to_string();
                refresh(&shared);
            });
        }
        {
            let shared = shared.clone();
            let controller = gtk4::EventControllerKey::new();
            controller.connect_key_pressed(move |_, keyval, _, state| match keyval {
                Key::Return | Key::KP_Enter => {
                    if state.contains(gdk::ModifierType::SHIFT_MASK) {
                        previous_match(&shared);
                    } else {
                        next_match(&shared);
                    }
                    gtk4::glib::Propagation::Stop
                }
                Key::Escape => {
                    close(&shared);
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            });
            entry.add_controller(controller);
        }
        {
            let shared = shared.clone();
            next.connect_clicked(move |_| next_match(&shared));
        }
        {
            let shared = shared.clone();
            previous.connect_clicked(move |_| previous_match(&shared));
        }
        {
            let shared = shared.clone();
            close_button.connect_clicked(move |_| close(&shared));
        }

        Self { root, shared }
    }

    /// The bar widget.
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Shows the bar, focusing the query field (GITILANTE_SEARCH_SPEC.md §2).
    pub fn open(&self) {
        self.root.set_visible(true);
        (self.shared.on_focus)(true);
        self.shared.entry.grab_focus();
        self.shared.entry.select_region(0, -1);
        refresh(&self.shared);
    }

    /// Hides the bar and clears the highlight.
    pub fn close(&self) {
        close(&self.shared);
    }

    /// True while the bar is visible.
    pub fn is_open(&self) -> bool {
        self.root.is_visible()
    }

    /// Sets the searchable text of the current view (one entry per rendered
    /// hunk body) and re-runs the current query over it.
    pub fn set_targets(&self, targets: Vec<SearchTarget>) {
        clear_highlight(&self.shared);
        let states = targets
            .into_iter()
            .map(|target| {
                // Created after the rendering tags: the search highlight wins
                // over the intraline overlay (section 4).
                let match_tag = target
                    .buffer
                    .create_tag(None::<&str>, &[("background-rgba", &match_background())])
                    .expect("create text tag");
                let active_tag = target
                    .buffer
                    .create_tag(
                        None::<&str>,
                        &[
                            ("background-rgba", &active_background()),
                            ("underline", &pango::Underline::Single),
                        ],
                    )
                    .expect("create text tag");
                TargetState {
                    target,
                    match_tag,
                    active_tag,
                }
            })
            .collect();
        *self.shared.targets.borrow_mut() = states;
        refresh(&self.shared);
    }

    /// Moves to the next match (Enter).
    pub fn next_match(&self) {
        next_match(&self.shared);
    }

    /// Moves to the previous match (Shift+Enter).
    pub fn previous_match(&self) {
        previous_match(&self.shared);
    }
}

/// Runs the current query over every target and highlights the matches.
fn refresh(shared: &Rc<Shared>) {
    clear_highlight(shared);
    let query = shared.query.borrow().clone();
    let case_sensitive = is_case_sensitive(&query);

    let mut matches = Vec::new();
    let mut error = None;
    if !query.is_empty() {
        let targets = shared.targets.borrow();
        for (index, state) in targets.iter().enumerate() {
            let text = buffer_text(&state.target);
            let found = if shared.regex.get() {
                find_matches_regex(&text, &query, case_sensitive)
            } else {
                Ok(find_matches(&text, &query, case_sensitive))
            };
            match found {
                Ok(ranges) => {
                    for (start, end) in ranges {
                        matches.push(MatchLoc {
                            target: index,
                            start: start as i32,
                            end: end as i32,
                        });
                    }
                }
                Err(message) => {
                    error = Some(message);
                    break;
                }
            }
        }
    }

    shared.active.set(0);
    let total = matches.len();
    *shared.matches.borrow_mut() = matches;
    reapply(shared);
    if let Some(message) = error {
        // A contextual error near the field (section 38).
        shared.counter.set_text("invalid regex");
        shared.counter.set_tooltip_text(Some(&message));
    } else {
        shared.counter.set_tooltip_text(None);
        set_counter(shared, total);
    }
    reveal(shared);
}

/// Reapplies the highlight tags after the active match changed.
fn reapply(shared: &Rc<Shared>) {
    clear_highlight(shared);
    let matches = shared.matches.borrow();
    for (position, match_loc) in matches.iter().enumerate() {
        let targets = shared.targets.borrow();
        let state = &targets[match_loc.target];
        let tag = if position == shared.active.get() {
            &state.active_tag
        } else {
            &state.match_tag
        };
        let start = state.target.buffer.iter_at_offset(match_loc.start);
        let end = state.target.buffer.iter_at_offset(match_loc.end);
        state.target.buffer.apply_tag(tag, &start, &end);
    }
}

/// Removes every search highlight.
fn clear_highlight(shared: &Rc<Shared>) {
    let targets = shared.targets.borrow();
    for state in targets.iter() {
        let start = state.target.buffer.start_iter();
        let end = state.target.buffer.end_iter();
        state
            .target
            .buffer
            .remove_tag(&state.match_tag, &start, &end);
        state
            .target
            .buffer
            .remove_tag(&state.active_tag, &start, &end);
    }
}

/// Updates the "3 / 12" match counter (GITILANTE_SEARCH_SPEC.md section 3).
fn set_counter(shared: &Rc<Shared>, total: usize) {
    if shared.query.borrow().is_empty() {
        shared.counter.set_text("");
        return;
    }
    if total == 0 {
        shared.counter.set_text("0 / 0");
        return;
    }
    shared
        .counter
        .set_text(&format!("{} / {}", shared.active.get() + 1, total));
}

/// Moves to the next match, wrapping around.
fn next_match(shared: &Rc<Shared>) {
    let total = shared.matches.borrow().len();
    if total == 0 {
        return;
    }
    shared.active.set((shared.active.get() + 1) % total);
    reapply(shared);
    set_counter(shared, total);
    reveal(shared);
}

/// Moves to the previous match, wrapping around.
fn previous_match(shared: &Rc<Shared>) {
    let total = shared.matches.borrow().len();
    if total == 0 {
        return;
    }
    shared.active.set((shared.active.get() + total - 1) % total);
    reapply(shared);
    set_counter(shared, total);
    reveal(shared);
}

/// Hides the bar and clears the highlight.
fn close(shared: &Rc<Shared>) {
    clear_highlight(shared);
    if let Some(bar) = shared.entry.parent() {
        bar.set_visible(false);
    }
    (shared.on_focus)(false);
}

/// Scrolls the active match into view.
fn reveal(shared: &Rc<Shared>) {
    let matches = shared.matches.borrow();
    let Some(match_loc) = matches.get(shared.active.get()) else {
        return;
    };
    let targets = shared.targets.borrow();
    reveal_in(
        &shared.scrolled,
        &targets[match_loc.target].target,
        match_loc.start,
    );
}

/// Scrolls `char_offset` of `target` into view, both in its text view and in
/// the outer scroll of the diff panel (GITILANTE_SEARCH_SPEC.md sections 3
/// and 12).
pub fn reveal_in(scrolled: &ScrolledWindow, target: &SearchTarget, char_offset: i32) {
    let iter = target.buffer.iter_at_offset(char_offset);
    reveal_iter(scrolled, target, &iter);
}

/// Scrolls `iter` of `target` into view (GITILANTE_SEARCH_SPEC.md section 12).
pub fn reveal_iter(scrolled: &ScrolledWindow, target: &SearchTarget, iter: &gtk4::TextIter) {
    let mut iter = *iter;
    target.view.scroll_to_iter(&mut iter, 0.0, false, 0.0, 0.3);

    let Some(content) = scrolled.child() else {
        return;
    };
    let location = target.view.iter_location(&iter);
    if let Some((_, y)) = target.view.translate_coordinates(
        &content,
        f64::from(location.x()),
        f64::from(location.y()),
    ) {
        let adjustment = scrolled.vadjustment();
        let top = y - adjustment.page_size() / 3.0;
        let maximum = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(top.clamp(adjustment.lower(), maximum));
    }
}

/// Creates the temporary highlight tag for a landed search result
/// (GITILANTE_SEARCH_SPEC.md section 12).
pub fn flash_tag(buffer: &gtk4::TextBuffer) -> TextTag {
    buffer
        .create_tag(None::<&str>, &[("background-rgba", &active_background())])
        .expect("create text tag")
}

/// The full text of a target's buffer.
fn buffer_text(target: &SearchTarget) -> String {
    let start = target.buffer.start_iter();
    let end = target.buffer.end_iter();
    target.buffer.text(&start, &end, false).to_string()
}
