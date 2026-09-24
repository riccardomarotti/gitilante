//! Main window: sidebar (Changes + History) and diff view.
//!
//! Responsibilities (SPEC sections 14, 20, 21, 22, 29):
//! - runs every Git operation on the worker thread and refreshes afterwards;
//! - drops stale results with generation counters;
//! - refreshes when the window regains focus;
//! - loads history in blocks and commit diffs only when selected;
//! - reports errors with toasts, reserving dialogs for decisions (discard).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{Adjustment, Box as GtkBox, Button, Orientation, Paned, PolicyType, ScrolledWindow};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::conflict::presentation::ConflictPresentation;
use crate::conflict::resolution::ConflictResolution;
use crate::git::Error;
use crate::git::conflict::{ApplyAction, ApplyOutcome, ConflictLoad};
use crate::git::history::FileHistoryEntry;
use crate::git::repository::Repository;
use crate::model::commit::Commit;
use crate::model::diff::{Diff, FileDiff, Hunk};
use crate::model::refs::{CommitRef, HeadRef};
use crate::model::status::{Status, StatusEntry, UnmergedInfo};
use crate::search::SearchQuery;
use crate::search::SearchScope;
use crate::search::result::{
    ChangeSearchResult, HistoryChangeSearchResult, MatchRange, SearchResult,
};
use crate::ui::worker::Worker;
use crate::ui::{
    DiffSide, HunkTarget, Selection, changes, clipboard, conflict_view, diff_view, external_editor,
    folding, history, search, search_dialog, short_path,
};

/// Commits per history block (SPEC section 16).
const HISTORY_PAGE: usize = 200;

/// Distance from the sidebar bottom that triggers loading more history.
const HISTORY_SCROLL_THRESHOLD: f64 = 300.0;

/// Everything read from Git in one history block: the commits of the page plus
/// the refs and HEAD needed by the badges (BRANCH.md sections 4-11).
type HistoryData = (
    Vec<Commit>,
    Vec<FileHistoryEntry>,
    HashMap<String, Vec<CommitRef>>,
    Option<HeadRef>,
);

/// Everything read from Git in one refresh.
struct RefreshData {
    status: Status,
    worktree: Diff,
    staged: Diff,
}

/// History sidebar mode; file history follows the lineage from HEAD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum HistoryMode {
    #[default]
    Repository,
    File {
        display_path: PathBuf,
        seed_path: PathBuf,
    },
}

/// Current UI state.
#[derive(Default)]
struct State {
    data: Option<RefreshData>,
    selection: Option<Selection>,
    /// Incremented on every refresh; results of older refreshes are dropped.
    generation: u64,
    /// Commits loaded so far.
    commits: Vec<Commit>,
    file_entries: Vec<FileHistoryEntry>,
    history_mode: HistoryMode,
    history_generation: u64,
    /// Refs pointing at the loaded commits (BRANCH.md sections 9-10).
    refs: HashMap<String, Vec<CommitRef>>,
    /// Checked out HEAD (BRANCH.md section 11).
    head: Option<HeadRef>,
    /// True while a history request is in flight (requests are single flight).
    history_loading: bool,
    /// True when Git returned fewer commits than requested.
    history_exhausted: bool,
    /// Diff of the selected commit, loaded lazily (SPEC section 22).
    commit_diff: Option<(String, Diff)>,
    /// Content of the selected file preview, loaded lazily
    /// (GITILANTE_SEARCH_SPEC.md section 16).
    file_preview: Option<(PathBuf, Result<Vec<u8>, String>)>,
    /// Match ranges to flash once the preview is rendered (section 22).
    preview_flash: Option<Vec<MatchRange>>,
    /// Query to flash in the commit diff once loaded (section 30).
    commit_flash: Option<SearchQuery>,
    /// Bumped on every selection change; stale commit diffs are dropped.
    commit_diff_generation: u64,
    /// Hunk that has keyboard focus, target of the contextual shortcuts.
    current_hunk: Option<HunkTarget>,
    /// Searchable text of the currently rendered diff (section 3).
    render_targets: Vec<crate::ui::diff_view::SearchTarget>,
    /// Conflict solver sessions, kept while each conflict stays valid (§60-61).
    conflict_sessions: HashMap<PathBuf, Rc<RefCell<conflict_view::ConflictSession>>>,
    /// Load failures of the conflict solver, keyed by path.
    conflict_errors: HashMap<PathBuf, String>,
}

impl State {
    /// Switching modes invalidates every in-flight history page immediately.
    fn change_history_mode(&mut self, mode: HistoryMode) -> bool {
        if self.history_mode == mode {
            return false;
        }
        self.history_mode = mode;
        self.history_generation = self.history_generation.wrapping_add(1);
        self.history_loading = false;
        self.history_exhausted = false;
        self.commits.clear();
        self.file_entries.clear();
        true
    }
}

struct Inner {
    repo: Repository,
    worker: Worker,
    state: RefCell<State>,
    /// In-flight requests, shown by the header bar spinner.
    busy: Cell<u32>,
    /// Shared syntax highlighting resources (COLORS.md section 49).
    highlighter: crate::syntax::Highlighter,
    /// Folding state of the rendered diffs (GITILANTE_DIFF_FOLDING_SPEC.md §15).
    folds: RefCell<folding::FoldStateStore>,
    /// Disclosure to focus after a folding toggle (section 26).
    pending_focus: Cell<Option<folding::FoldFocus>>,
    /// Menu button whose model swaps between diff and conflict folding (§68).
    fold_button: gtk4::MenuButton,
    /// Last menu context; avoid replacing an open menu on every render.
    conflict_fold_menu_active: Cell<bool>,
    /// Query whose hidden matches were revealed last (sections 30-32).
    last_reveal_query: RefCell<Option<String>>,
    self_weak: Weak<Inner>,
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    subtitle: adw::WindowTitle,
    spinner: gtk4::Spinner,
    changes: changes::ChangesView,
    history_view: history::HistoryView,
    /// Ctrl+F search bar over the diff panel (GITILANTE_SEARCH_SPEC.md §3).
    search_bar: search::SearchBar,
    /// Ctrl+Shift+F global search dialog (GITILANTE_SEARCH_SPEC.md §5).
    search_dialog: Rc<search_dialog::SearchDialog>,
    /// Bumped on every search intent: stale results are dropped.
    search_generation: search_dialog::SearchGeneration,
    diff_box: GtkBox,
    diff_scroll: ScrolledWindow,
    /// Sidebar scroll, anchored across History rebuilds.
    sidebar_adjustment: Adjustment,
}

/// The Gitilante main window.
pub struct MainWindow {
    inner: Rc<Inner>,
}

impl MainWindow {
    /// Builds the window for `repo` (without showing it).
    pub fn new(app: &adw::Application, repo: Repository) -> Self {
        let inner: Rc<Inner> =
            Rc::new_cyclic(|weak: &Weak<Inner>| Inner::build(app, repo, weak.clone()));
        inner.refresh();
        Self { inner }
    }

    /// Shows the window.
    pub fn present(&self) {
        self.inner.window.present();
    }
}

impl Inner {
    fn build(app: &adw::Application, repo: Repository, self_weak: Weak<Inner>) -> Self {
        let subtitle = adw::WindowTitle::new("Gitilante", &short_path(repo.root()));
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&subtitle));

        let refresh_button = Button::from_icon_name("view-refresh-symbolic");
        refresh_button.set_tooltip_text(Some("Refresh (Ctrl+R)"));
        refresh_button.set_action_name(Some("win.refresh"));
        header.pack_end(&refresh_button);

        // Diff folding menu (GITILANTE_DIFF_FOLDING_SPEC.md section 21).
        let fold_menu = gio::Menu::new();
        fold_menu.append(Some("Collapse all files"), Some("win.fold-collapse-files"));
        fold_menu.append(Some("Expand all files"), Some("win.fold-expand-files"));
        fold_menu.append(Some("Collapse all hunks"), Some("win.fold-collapse-hunks"));
        fold_menu.append(Some("Expand all hunks"), Some("win.fold-expand-hunks"));
        let fold_button = gtk4::MenuButton::new();
        fold_button.set_icon_name("open-menu-symbolic");
        fold_button.set_menu_model(Some(&fold_menu));
        fold_button.set_tooltip_text(Some("Diff folding"));
        header.pack_end(&fold_button);

        // Loading state: spins while any Git request is in flight (SPEC §35).
        let spinner = gtk4::Spinner::new();
        spinner.set_visible(false);
        spinner.set_tooltip_text(Some("Loading…"));
        header.pack_end(&spinner);

        let changes = changes::ChangesView::new(changes::Callbacks {
            copy: op_callback(&self_weak, Inner::copy_action),
            open_editor: op_callback(&self_weak, Inner::open_in_editor),
            select: {
                let weak = self_weak.clone();
                Box::new(move |selection| {
                    if let Some(inner) = weak.upgrade() {
                        inner.history_view.clear_selection();
                        inner.select(selection);
                    }
                })
            },
            stage_file: op_callback(&self_weak, Inner::stage_file),
            unstage_file: op_callback(&self_weak, Inner::unstage_file),
            discard_file: {
                let weak = self_weak.clone();
                Box::new(move |path| {
                    if let Some(inner) = weak.upgrade() {
                        inner.confirm_discard_file(path);
                    }
                })
            },
        });

        let history_view = history::HistoryView::new(history::Callbacks {
            copy: op_callback(&self_weak, Inner::copy_action),
            select: {
                let weak = self_weak.clone();
                Box::new(move |selection| {
                    if let Some(inner) = weak.upgrade() {
                        inner.changes.clear_selection();
                        inner.select(selection);
                    }
                })
            },
            all_history: {
                let weak = self_weak.clone();
                Box::new(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.set_history_mode(HistoryMode::Repository);
                    }
                })
            },
            load_more: {
                let weak = self_weak.clone();
                Box::new(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.request_history_more();
                    }
                })
            },
        });

        // The whole sidebar column scrolls as one, Changes above History.
        let sidebar = GtkBox::new(Orientation::Vertical, 0);
        sidebar.append(changes.widget());
        sidebar.append(history_view.widget());

        let sidebar_scroll = ScrolledWindow::new();
        sidebar_scroll.set_child(Some(&sidebar));
        sidebar_scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        sidebar_scroll.set_width_request(320);
        hook_history_scroll(&sidebar_scroll.vadjustment(), self_weak.clone());

        let diff_scroll = ScrolledWindow::new();
        diff_scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);
        diff_scroll.set_hexpand(true);
        diff_scroll.set_vexpand(true);
        let diff_box = GtkBox::new(Orientation::Vertical, 0);
        diff_scroll.set_child(Some(&diff_box));

        // The search bar sits above the diff panel
        // (GITILANTE_SEARCH_SPEC.md section 3).
        let search_bar = {
            let app = app.clone();
            let weak = self_weak.clone();
            search::SearchBar::new(
                diff_scroll.clone(),
                Box::new(move |focused| {
                    // Typing must never trigger the hunk shortcuts (section 51).
                    crate::ui::app::suspend_hunk_shortcuts(&app, focused);
                }),
                Box::new(move |query| {
                    if let Some(inner) = weak.upgrade() {
                        inner.reveal_hidden_matches(query);
                    }
                }),
            )
        };

        // Global search dialog (GITILANTE_SEARCH_SPEC.md section 5).
        let search_dialog = {
            let weak = self_weak.clone();
            Rc::new(search_dialog::SearchDialog::new(search_dialog::Callbacks {
                search: Box::new(move |query| {
                    if let Some(inner) = weak.upgrade() {
                        inner.run_search(query);
                    }
                }),
                invalidate: {
                    let weak = self_weak.clone();
                    Box::new(move || {
                        if let Some(inner) = weak.upgrade() {
                            inner.search_generation.invalidate();
                        }
                    })
                },
                activate: {
                    let weak = self_weak.clone();
                    Box::new(move |result, query| {
                        if let Some(inner) = weak.upgrade() {
                            inner.activate_search_result(result, query);
                        }
                    })
                },
                on_open: {
                    let app = app.clone();
                    Box::new(move |open| {
                        // Typing must never trigger the hunk shortcuts (§51).
                        crate::ui::app::suspend_hunk_shortcuts(&app, open);
                    })
                },
            }))
        };
        let diff_panel = GtkBox::new(Orientation::Vertical, 0);
        diff_panel.append(search_bar.widget());
        diff_panel.append(&diff_scroll);

        let paned = Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar_scroll));
        paned.set_end_child(Some(&diff_panel));
        paned.set_position(340);
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_vexpand(true);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&paned));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(1200)
            .default_height(800)
            .title("Gitilante")
            .build();
        window.set_content(Some(&toast_overlay));

        // Ctrl+R (SPEC section 30).
        let refresh_action = gio::SimpleAction::new("refresh", None);
        {
            let weak = self_weak.clone();
            refresh_action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.refresh();
                }
            });
        }
        window.add_action(&refresh_action);

        // Ctrl+F: search in the current view
        // (GITILANTE_SEARCH_SPEC.md sections 2-3).
        let search_action = gio::SimpleAction::new("search", None);
        {
            let weak = self_weak.clone();
            search_action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.search_bar.open();
                }
            });
        }
        window.add_action(&search_action);

        // Ctrl+Shift+F: global search (GITILANTE_SEARCH_SPEC.md section 2).
        let global_search_action = gio::SimpleAction::new("global-search", None);
        {
            let weak = self_weak.clone();
            global_search_action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.search_dialog.present(&inner.window);
                }
            });
        }
        window.add_action(&global_search_action);

        // Diff folding actions (GITILANTE_DIFF_FOLDING_SPEC.md sections 21-22).
        for (name, action) in [
            ("fold-collapse-files", folding::FoldAction::CollapseAllFiles),
            ("fold-expand-files", folding::FoldAction::ExpandAllFiles),
            ("fold-collapse-hunks", folding::FoldAction::CollapseAllHunks),
            ("fold-expand-hunks", folding::FoldAction::ExpandAllHunks),
        ] {
            let fold_action = gio::SimpleAction::new(name, None);
            let weak = self_weak.clone();
            fold_action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.fold_all(action);
                }
            });
            window.add_action(&fold_action);
        }

        // Conflict folding actions (conflict solver §35).
        for (name, collapse) in [
            ("fold-collapse-conflicts", true),
            ("fold-expand-conflicts", false),
        ] {
            let fold_action = gio::SimpleAction::new(name, None);
            let weak = self_weak.clone();
            fold_action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.fold_all_conflicts(collapse);
                }
            });
            window.add_action(&fold_action);
        }

        // Contextual hunk shortcuts, acting on the focused hunk (SPEC §30).
        for (name, activate) in [
            ("stage-hunk", Inner::shortcut_stage_hunk as fn(&Inner)),
            ("unstage-hunk", Inner::shortcut_unstage_hunk),
            ("discard-hunk", Inner::shortcut_discard_hunk),
            ("revert-hunk", Inner::shortcut_revert_hunk),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let weak = self_weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    activate(&inner);
                }
            });
            window.add_action(&action);
        }

        // Refresh when the window regains focus (SPEC section 20).
        {
            let weak = self_weak.clone();
            window.connect_is_active_notify(move |window| {
                if window.is_active() {
                    // Popups can change activation during the same event that
                    // maps them. Wait until that event finishes before deciding
                    // whether this was a real return to the application.
                    let weak = weak.clone();
                    gtk4::glib::idle_add_local_once(move || {
                        if let Some(inner) = weak.upgrade() {
                            if inner.window.is_active() && !clipboard::menu_is_open() {
                                inner.refresh();
                            }
                        }
                    });
                }
            });
        }

        // Re-render the diff when the light/dark appearance changes, so the
        // syntax style scheme follows the theme (COLORS.md sections 17, 32)
        // and the graph palette stays readable (BRANCH.md section 65).
        {
            let weak = self_weak.clone();
            adw::StyleManager::default().connect_dark_notify(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.render_diff();
                    inner.render_history();
                }
            });
        }

        Self {
            repo,
            worker: Worker::new(),
            state: RefCell::new(State::default()),
            busy: Cell::new(0),
            highlighter: crate::syntax::Highlighter::new(),
            folds: RefCell::new(folding::FoldStateStore::default()),
            pending_focus: Cell::new(None),
            fold_button,
            conflict_fold_menu_active: Cell::new(false),
            last_reveal_query: RefCell::new(None),
            self_weak,
            window,
            toast_overlay,
            subtitle,
            spinner,
            changes,
            history_view,
            search_bar,
            search_dialog,
            search_generation: search_dialog::SearchGeneration::default(),
            diff_box,
            diff_scroll,
            sidebar_adjustment: sidebar_scroll.vadjustment(),
        }
    }

    /// Shows the loading state while a request is in flight.
    fn busy_start(&self) {
        self.busy.set(self.busy.get() + 1);
        self.spinner.set_visible(true);
        self.spinner.start();
    }

    /// Hides the loading state when no request is in flight anymore.
    fn busy_finish(&self) {
        let remaining = self.busy.get().saturating_sub(1);
        self.busy.set(remaining);
        if remaining == 0 {
            self.spinner.stop();
            self.spinner.set_visible(false);
        }
    }

    // --- data loading ----------------------------------------------------

    /// Reloads status, diffs and history from Git (SPEC section 20).
    fn refresh(&self) {
        self.search_dialog.repository_changed();
        let generation = {
            let mut state = self.state.borrow_mut();
            state.generation += 1;
            state.generation
        };
        self.busy_start();
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || -> Result<RefreshData, Error> {
                Ok(RefreshData {
                    status: repo.status()?,
                    worktree: repo.working_tree_diff()?,
                    staged: repo.staged_diff()?,
                })
            },
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_refresh(generation, result);
                }
            },
        );
        self.request_history_replace();
    }

    /// Reloads the history blocks loaded so far.
    fn request_history_replace(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.history_generation = state.history_generation.wrapping_add(1);
            state.history_loading = false;
        }
        let count = {
            let state = self.state.borrow();
            state.commits.len().max(HISTORY_PAGE)
        };
        self.request_history(false, 0, count);
    }

    /// Loads the next block of history (SPEC sections 16 and 22).
    fn set_history_mode(&self, mode: HistoryMode) {
        if !self.state.borrow_mut().change_history_mode(mode) {
            return;
        }
        self.request_history_replace();
        self.render_diff();
        let history = self.history_view.widget().clone();
        let adjustment = self.sidebar_adjustment.clone();
        gtk4::glib::idle_add_local_once(move || {
            if let Some(parent) = history.parent() {
                if let Some(bounds) = history.compute_bounds(&parent) {
                    adjustment.set_value(f64::from(bounds.y()));
                }
            }
        });
    }

    fn request_history_more(&self) {
        let skip = self.state.borrow().commits.len();
        self.request_history(true, skip, HISTORY_PAGE);
    }

    /// Single-flight history request: `append` adds to the loaded commits,
    /// otherwise the list is replaced.
    fn request_history(&self, append: bool, skip: usize, count: usize) {
        let (generation, mode) = {
            let mut state = self.state.borrow_mut();
            if state.history_loading || (append && state.history_exhausted) {
                return;
            }
            state.history_loading = true;
            (state.history_generation, state.history_mode.clone())
        };
        self.busy_start();
        self.render_history();

        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || -> Result<HistoryData, Error> {
                let (commits, entries) = match &mode {
                    HistoryMode::Repository => (repo.history(skip, count)?, Vec::new()),
                    HistoryMode::File { seed_path, .. } => {
                        let entries = repo.file_history(seed_path, skip, count)?;
                        let commits = entries.iter().map(|entry| entry.commit.clone()).collect();
                        (commits, entries)
                    }
                };
                let refs = repo.commit_refs()?;
                let head = repo.head()?;
                Ok((commits, entries, refs, head))
            },
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_history(generation, append, count, result);
                }
            },
        );
    }

    fn finish_history(
        &self,
        generation: u64,
        append: bool,
        requested: usize,
        result: Result<HistoryData, Error>,
    ) {
        self.busy_finish();
        if self.state.borrow().history_generation != generation {
            return;
        }
        match result {
            Ok((commits, entries, refs, head)) => {
                let mut state = self.state.borrow_mut();
                state.history_loading = false;
                state.history_exhausted = commits.len() < requested;
                state.refs = refs;
                state.head = head;
                if append {
                    state.commits.extend(commits);
                    state.file_entries.extend(entries);
                } else {
                    state.commits = commits;
                    state.file_entries = entries;
                }
            }
            Err(error) => {
                self.state.borrow_mut().history_loading = false;
                self.show_error(&error);
            }
        }
        self.render_history();
        let update_file_diff = {
            let state = self.state.borrow();
            matches!(state.history_mode, HistoryMode::File { .. })
                && matches!(state.selection, Some(Selection::Commit(_)))
        };
        if update_file_diff {
            self.render_diff();
        }
    }

    /// Applies a refresh result unless a newer refresh superseded it.
    fn finish_refresh(&self, generation: u64, result: Result<RefreshData, Error>) {
        self.busy_finish();
        {
            let mut state = self.state.borrow_mut();
            if state.generation != generation {
                return; // stale result (SPEC section 21)
            }
            match result {
                Ok(data) => {
                    let previous = state.selection.clone();
                    // A solver session survives only while its conflict is
                    // unchanged: same file, same stages (§61).
                    state.conflict_sessions.retain(|path, session| {
                        data.status
                            .entries
                            .iter()
                            .find(|entry| &entry.path == path)
                            .and_then(|entry| entry.unmerged())
                            .is_some_and(|info| *info == session.borrow().load.unmerged)
                    });
                    let selection = previous
                        .clone()
                        .filter(|selection| selection_exists(&data.status, selection));
                    // After a resolution, the next conflict is selected (§63).
                    let next_conflict = if matches!(previous, Some(Selection::Conflicted(_))) {
                        data.status
                            .unmerged_entries()
                            .next()
                            .map(|entry| Selection::Conflicted(entry.path.clone()))
                    } else {
                        None
                    };
                    // Show a diff right away: when nothing is selected (startup
                    // or a selection that vanished), pick the first entry.
                    let first = first_selection(&data.status);
                    state.data = Some(data);
                    state.selection = selection.or(next_conflict).or(first);
                }
                Err(error) => {
                    drop(state);
                    self.show_error(&error);
                    return;
                }
            }
        }
        self.render_changes();
        self.render_diff();
        self.search_dialog.rerun_if_open();
        // Open the solver when a selected conflict has no session yet (§58).
        let pending_conflict = {
            let state = self.state.borrow();
            match &state.selection {
                Some(Selection::Conflicted(path))
                    if !state.conflict_sessions.contains_key(path) =>
                {
                    Some((state.commit_diff_generation, path.clone()))
                }
                _ => None,
            }
        };
        if let Some((generation, path)) = pending_conflict {
            self.load_conflict(generation, path);
        }
        let (root, status) = {
            let state = self.state.borrow();
            let data = state.data.as_ref().expect("refresh just set it");
            (self.repo.root().to_path_buf(), data.status.clone())
        };
        self.subtitle.set_subtitle(&subtitle_text(&root, &status));
    }

    /// Loads the diff of a commit, dropping results that became stale.
    fn load_commit_diff(&self, generation: u64, oid: String) {
        self.busy_start();
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        let requested = oid.clone();
        self.worker.run(
            move || repo.commit_diff(&oid),
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_commit_diff(generation, requested, result);
                }
            },
        );
    }

    fn finish_commit_diff(&self, generation: u64, oid: String, result: Result<Diff, Error>) {
        self.busy_finish();
        match result {
            Ok(diff) => {
                let mut state = self.state.borrow_mut();
                if state.commit_diff_generation != generation
                    || state.selection.as_ref() != Some(&Selection::Commit(oid.clone()))
                {
                    return; // stale result (SPEC section 21)
                }
                state.commit_diff = Some((oid, diff));
            }
            Err(error) => {
                self.show_error(&error);
            }
        }
        self.render_diff();
        // GITILANTE_SEARCH_SPEC.md section 30: land on the first match of a
        // History Changes result.
        self.flash_commit_matches();
    }

    // --- selection and rendering -----------------------------------------

    /// Handles sidebar row selection.
    fn select(&self, selection: Selection) {
        let mut state = self.state.borrow_mut();
        state.commit_diff_generation += 1;
        let generation = state.commit_diff_generation;

        let cached = match &selection {
            Selection::Commit(oid) => state
                .commit_diff
                .as_ref()
                .is_some_and(|(cached, _)| cached == oid),
            Selection::FilePreview { path, .. } => state
                .file_preview
                .as_ref()
                .is_some_and(|(cached, _)| cached == path),
            Selection::Conflicted(path) => state.conflict_sessions.contains_key(path),
            _ => true,
        };
        if !matches!(selection, Selection::FilePreview { .. }) {
            state.preview_flash = None;
        }
        state.selection = Some(selection.clone());
        drop(state);

        self.render_diff();
        if !cached {
            match selection {
                Selection::Commit(oid) => self.load_commit_diff(generation, oid),
                Selection::FilePreview { path, .. } => self.load_file_preview(generation, path),
                Selection::Conflicted(path) => self.load_conflict(generation, path),
                _ => {}
            }
        }
    }

    /// Loads the working tree content of a file preview
    /// (GITILANTE_SEARCH_SPEC.md section 16).
    fn load_file_preview(&self, generation: u64, path: PathBuf) {
        self.busy_start();
        let root = self.repo.root().to_path_buf();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || {
                std::fs::read(root.join(&path))
                    .map(|bytes| (path.clone(), bytes))
                    .map_err(|error| (path, error.to_string()))
            },
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_file_preview(generation, result);
                }
            },
        );
    }

    fn finish_file_preview(
        &self,
        generation: u64,
        result: Result<(PathBuf, Vec<u8>), (PathBuf, String)>,
    ) {
        self.busy_finish();
        let mut state = self.state.borrow_mut();
        if state.commit_diff_generation != generation {
            return; // stale result (SPEC section 21)
        }
        state.file_preview = Some(match result {
            Ok((path, bytes)) => (path, Ok(bytes)),
            Err((path, error)) => (path, Err(error)),
        });
        drop(state);
        self.render_diff();
    }

    // --- conflict solver (docs/GITILANTE_CONFLICT_SOLVER_SPEC.md) -------

    /// Swaps the folding menu between the diff and the conflict solver (§68).
    fn update_fold_menu(&self) {
        let conflict = matches!(
            self.state.borrow().selection,
            Some(Selection::Conflicted(_))
        );
        if self.conflict_fold_menu_active.get() == conflict {
            return;
        }
        self.conflict_fold_menu_active.set(conflict);
        let menu = gio::Menu::new();
        if conflict {
            menu.append(
                Some("Collapse all conflicts"),
                Some("win.fold-collapse-conflicts"),
            );
            menu.append(
                Some("Expand all conflicts"),
                Some("win.fold-expand-conflicts"),
            );
            self.fold_button.set_tooltip_text(Some("Conflict folding"));
        } else {
            menu.append(Some("Collapse all files"), Some("win.fold-collapse-files"));
            menu.append(Some("Expand all files"), Some("win.fold-expand-files"));
            menu.append(Some("Collapse all hunks"), Some("win.fold-collapse-hunks"));
            menu.append(Some("Expand all hunks"), Some("win.fold-expand-hunks"));
            self.fold_button.set_tooltip_text(Some("Diff folding"));
        }
        self.fold_button.set_menu_model(Some(&menu));
    }

    /// Collapses or expands every conflict block (§35).
    fn fold_all_conflicts(&self, collapse: bool) {
        self.with_current_session(|session| session.collapsed.fill(collapse));
        self.render_diff();
    }

    /// Loads one conflict into a solver session (§58).
    fn load_conflict(&self, generation: u64, path: PathBuf) {
        self.busy_start();
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || match repo.conflict(&path) {
                Ok(loaded) => Ok((path.clone(), loaded)),
                Err(error) => Err((path.clone(), error)),
            },
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_conflict(generation, result);
                }
            },
        );
    }

    /// Stores a loaded conflict unless a newer selection superseded it (§59).
    fn finish_conflict(
        &self,
        generation: u64,
        result: Result<(PathBuf, Option<ConflictLoad>), (PathBuf, Error)>,
    ) {
        self.busy_finish();
        let mut state = self.state.borrow_mut();
        if state.commit_diff_generation != generation {
            return; // stale result (§59)
        }
        match result {
            Ok((path, Some(load))) => {
                state.conflict_errors.remove(&path);
                state.conflict_sessions.insert(
                    path.clone(),
                    Rc::new(RefCell::new(conflict_view::ConflictSession::new(
                        path, load,
                    ))),
                );
            }
            Ok((path, None)) => {
                state.conflict_sessions.remove(&path);
            }
            Err((path, error)) => {
                state.conflict_errors.insert(path, error.to_string());
                drop(state);
                self.show_error(&error);
                self.render_diff();
                return;
            }
        }
        drop(state);
        self.render_diff();
    }

    /// Runs `action` on the session of the selected conflict (§60).
    fn with_current_session(&self, action: impl FnOnce(&mut conflict_view::ConflictSession)) {
        let state = self.state.borrow();
        if let Some(Selection::Conflicted(path)) = &state.selection {
            if let Some(session) = state.conflict_sessions.get(path) {
                action(&mut session.borrow_mut());
            }
        }
    }

    /// Callbacks of the conflict solver.
    fn conflict_callbacks(&self) -> Rc<conflict_view::Callbacks> {
        let choose = {
            let weak = self.self_weak.clone();
            Box::new(move |index: usize, resolution: ConflictResolution| {
                if let Some(inner) = weak.upgrade() {
                    inner.with_current_session(|session| {
                        session.resolutions[index] = resolution;
                    });
                }
            })
        };
        let reset = {
            let weak = self.self_weak.clone();
            Box::new(move |index: usize| {
                if let Some(inner) = weak.upgrade() {
                    inner.with_current_session(|session| {
                        session.resolutions[index] = ConflictResolution::Unresolved;
                    });
                }
            })
        };
        let toggle_collapse = {
            let weak = self.self_weak.clone();
            Box::new(move |index: usize| {
                if let Some(inner) = weak.upgrade() {
                    inner.with_current_session(|session| {
                        session.collapsed[index] = !session.collapsed[index];
                    });
                }
            })
        };
        let toggle_base = {
            let weak = self.self_weak.clone();
            Box::new(move |index: usize| {
                if let Some(inner) = weak.upgrade() {
                    inner.with_current_session(|session| {
                        session.base_visible[index] = !session.base_visible[index];
                    });
                }
            })
        };
        let choose_file = {
            let weak = self.self_weak.clone();
            Box::new(move |choice: conflict_view::FileChoice| {
                if let Some(inner) = weak.upgrade() {
                    inner.with_current_session(|session| session.file_choice = choice);
                }
            })
        };
        let apply = {
            let weak = self.self_weak.clone();
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.apply_conflict();
                }
            })
        };
        let mark_resolved = {
            let weak = self.self_weak.clone();
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.mark_conflict_resolved();
                }
            })
        };
        let refresh = {
            let weak = self.self_weak.clone();
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.refresh_conflict();
                }
            })
        };
        Rc::new(conflict_view::Callbacks {
            open_editor: op_callback(&self.self_weak, Inner::open_in_editor),
            choose,
            reset,
            toggle_collapse,
            toggle_base,
            choose_file,
            apply,
            mark_resolved,
            refresh,
        })
    }

    /// Applies the resolution: writes the file and stages it (§43-44).
    fn apply_conflict(&self) {
        let prepared = {
            let state = self.state.borrow();
            let Some(Selection::Conflicted(path)) = state.selection.clone() else {
                return;
            };
            let Some(session) = state.conflict_sessions.get(&path).cloned() else {
                return;
            };
            let session = session.borrow();
            let action = match &session.load.presentation {
                ConflictPresentation::TextBlocks(file) => {
                    match file.build_resolved(&session.resolutions) {
                        Ok(bytes) => ApplyAction::Write(bytes),
                        Err(_) => return, // §38: apply is enabled only when complete
                    }
                }
                ConflictPresentation::KeepOrDelete { .. } => match session.file_choice {
                    conflict_view::FileChoice::KeepFile => ApplyAction::Stage,
                    conflict_view::FileChoice::DeleteFile => ApplyAction::Delete,
                    _ => return,
                },
                ConflictPresentation::ResolveAsDeleted => ApplyAction::Delete,
                ConflictPresentation::WholeFileChoice {
                    version_a,
                    version_b,
                } => match session.file_choice {
                    conflict_view::FileChoice::UseVersionA => {
                        ApplyAction::Write(version_a.bytes.clone())
                    }
                    conflict_view::FileChoice::UseVersionB => {
                        ApplyAction::Write(version_b.bytes.clone())
                    }
                    _ => return,
                },
                ConflictPresentation::AlreadyManuallyResolved => ApplyAction::Stage,
                ConflictPresentation::Unsupported(_) => return,
            };
            (
                path,
                session.load.unmerged.clone(),
                session.load.working_tree.clone(),
                action,
            )
        };
        let (path, expected, snapshot, action) = prepared;
        self.run_conflict_apply(path, expected, snapshot, action);
    }

    /// Stages the current content as the resolution (§41).
    fn mark_conflict_resolved(&self) {
        let prepared = {
            let state = self.state.borrow();
            let Some(Selection::Conflicted(path)) = state.selection.clone() else {
                return;
            };
            let Some(session) = state.conflict_sessions.get(&path).cloned() else {
                return;
            };
            let session = session.borrow();
            (
                path,
                session.load.unmerged.clone(),
                session.load.working_tree.clone(),
            )
        };
        let (path, expected, snapshot) = prepared;
        self.run_conflict_apply(path, expected, snapshot, ApplyAction::Stage);
    }

    /// Runs the guarded apply on the worker thread (§39-44, §89).
    ///
    /// File and conflict stages are re-verified inside the same task that
    /// applies, so nothing can change between the check and the index update.
    fn run_conflict_apply(
        &self,
        path: PathBuf,
        expected: UnmergedInfo,
        snapshot: Option<Vec<u8>>,
        action: ApplyAction,
    ) {
        self.busy_start();
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        let path_for_result = path.clone();
        self.worker.run(
            move || repo.resolve_conflict(&path, &expected, snapshot.as_deref(), action),
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_conflict_apply(path_for_result, result);
                }
            },
        );
    }

    /// Completes a guarded apply (§39-44).
    fn finish_conflict_apply(&self, path: PathBuf, result: Result<ApplyOutcome, Error>) {
        self.busy_finish();
        match result {
            Ok(ApplyOutcome::Applied) => {
                self.state.borrow_mut().conflict_sessions.remove(&path);
                self.refresh();
            }
            Ok(ApplyOutcome::Stale) => {
                self.state.borrow_mut().conflict_sessions.remove(&path);
                let toast = adw::Toast::new(
                    "The file or its conflict stages changed since the conflict solver was opened.\nRefresh the conflict and resolve it again.",
                );
                toast.set_timeout(8);
                self.toast_overlay.add_toast(toast);
                self.refresh();
            }
            Err(Error::Git(git_error)) => {
                // §44: the file may hold the resolution without being staged.
                log::warn!("{}", git_error.details());
                let toast = adw::Toast::new(
                    "The file on disk contains the resolution but is not staged yet.\nCheck the message in the logs, then apply again.",
                );
                toast.set_timeout(8);
                self.toast_overlay.add_toast(toast);
                self.refresh();
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// Reloads the selected conflict (§42, §61).
    fn refresh_conflict(&self) {
        let pending = {
            let mut state = self.state.borrow_mut();
            match state.selection.clone() {
                Some(Selection::Conflicted(path)) => {
                    state.conflict_sessions.remove(&path);
                    state.conflict_errors.remove(&path);
                    Some((state.commit_diff_generation, path))
                }
                _ => None,
            }
        };
        if let Some((generation, path)) = pending {
            self.load_conflict(generation, path);
        }
    }

    /// Runs the global search on the worker thread
    /// (GITILANTE_SEARCH_SPEC.md sections 52-53).
    fn run_search(&self, query: SearchQuery) {
        let generation = self.search_generation.current();
        // An invalid pattern reports before any provider runs (section 38).
        if let Err(message) = crate::search::matcher::validate(&query) {
            self.search_dialog.reset();
            self.search_dialog.show_error(&message);
            return;
        }
        if cfg!(debug_assertions) {
            // Debug telemetry (GITILANTE_SEARCH_SPEC.md section 75).
            eprintln!(
                "[search] scope={:?} query={:?} regex={}",
                query.scope, query.text, query.regex
            );
        }
        let (worktree, staged) = {
            let state = self.state.borrow();
            match state.data.as_ref() {
                Some(data) => (data.worktree.clone(), data.staged.clone()),
                None => (Diff::default(), Diff::default()),
            }
        };
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || {
                // The providers work on data already loaded or on the worker
                // thread (GITILANTE_SEARCH_SPEC.md sections 52-53).
                let mut results = Vec::new();
                let mut error = None;
                if matches!(query.scope, SearchScope::All | SearchScope::Changes) {
                    results.extend(crate::search::changes::search(&query, &worktree, &staged));
                }
                if matches!(query.scope, SearchScope::All | SearchScope::Files) {
                    if let Ok(files) = repo.files() {
                        results.extend(crate::search::files::search(&query, &files));
                    }
                }
                if matches!(query.scope, SearchScope::All | SearchScope::Contents) {
                    let untracked = repo.untracked_files().unwrap_or_default();
                    results.extend(crate::search::contents::search(&query, &repo, &untracked));
                }
                if matches!(query.scope, SearchScope::All | SearchScope::History) {
                    let refs = repo.commit_refs().unwrap_or_default();
                    results.extend(crate::search::history::search(&query, &repo, &refs));
                }
                // History Changes never runs in All (section 28).
                if query.scope == SearchScope::HistoryChanges {
                    match crate::search::history_changes::search(&query, &repo) {
                        Ok(found) => results.extend(found),
                        Err(message) => error = Some(message),
                    }
                }
                (results, error)
            },
            move |(results, error)| {
                if let Some(inner) = weak.upgrade() {
                    // A stale search never overwrites newer ones (section 35).
                    if inner.search_generation.accepts(generation) {
                        inner.search_dialog.show_results(results);
                        if let Some(message) = error {
                            // A failing provider never hides the others
                            // (GITILANTE_SEARCH_SPEC.md sections 37-38).
                            inner.search_dialog.show_error(&message);
                        }
                    }
                }
            },
        );
    }

    /// Navigates to an activated search result (GITILANTE_SEARCH_SPEC.md §12).
    fn activate_search_result(&self, result: SearchResult, query: SearchQuery) {
        match result {
            SearchResult::Change(change) => self.navigate_to_change(&change),
            SearchResult::File(file) => self.navigate_to_file(&file.path, None, &[]),
            SearchResult::Content(content) => self.navigate_to_file(
                &content.path,
                Some(content.line_number),
                &content.match_ranges,
            ),
            SearchResult::Commit(commit) => self.navigate_to_commit(&commit.oid),
            SearchResult::HistoryChange(change) => self.navigate_to_history_change(&change, &query),
        }
    }

    /// Navigates to a History Changes result (GITILANTE_SEARCH_SPEC.md §30):
    /// select the commit, then flash the first match on its changed lines.
    fn navigate_to_history_change(&self, result: &HistoryChangeSearchResult, query: &SearchQuery) {
        self.search_dialog.close();
        {
            let mut state = self.state.borrow_mut();
            state.commit_flash = Some(query.clone());
        }
        self.select(Selection::Commit(result.oid.clone()));
    }

    /// Flashes the first match of the pending query on the added/removed lines
    /// of the freshly rendered commit diff (GITILANTE_SEARCH_SPEC.md §30).
    fn flash_commit_matches(&self) {
        let (query, diff, targets) = {
            let mut state = self.state.borrow_mut();
            let Some(query) = state.commit_flash.take() else {
                return;
            };
            let Some((_, diff)) = state.commit_diff.clone() else {
                return;
            };
            (query, diff, state.render_targets.clone())
        };
        let case_sensitive = query.is_case_sensitive();
        let mut found = None;
        'search: for file in &diff.files {
            for hunk in &file.hunks {
                for (line_index, line) in hunk.lines.iter().enumerate() {
                    if !matches!(
                        line.kind,
                        crate::model::diff::DiffLineKind::Addition
                            | crate::model::diff::DiffLineKind::Deletion
                    ) {
                        continue;
                    }
                    let text = line.text();
                    let ranges =
                        crate::search::matcher::find_matches(&text, &query.text, case_sensitive);
                    if ranges.is_empty() {
                        continue;
                    }
                    found = Some((
                        folding::HunkFoldKey::from_hunk(file, hunk),
                        line_index,
                        ranges,
                    ));
                    break 'search;
                }
            }
        }
        let Some((key, line_index, ranges)) = found else {
            return;
        };
        // Reveal the parents when the match is inside collapsed content
        // (GITILANTE_DIFF_FOLDING_SPEC.md sections 30-31).
        let mut targets = targets;
        if !targets
            .iter()
            .any(|target| target.hunk.as_ref() == Some(&key))
        {
            let document = {
                let state = self.state.borrow();
                state
                    .selection
                    .as_ref()
                    .and_then(folding::DiffDocumentKey::from_selection)
            };
            if let Some(document) = document {
                self.folds
                    .borrow_mut()
                    .for_document(&document)
                    .reveal_key(&key);
            }
            self.rerender_diff();
            targets = self.state.borrow().render_targets.clone();
        }
        let Some(target) = targets
            .iter()
            .find(|target| target.hunk.as_ref() == Some(&key))
        else {
            return;
        };
        // Flash this match and stop: Ctrl+F walks the rest.
        let flash_tag = search::flash_tag(&target.buffer);
        for (start, end) in ranges {
            let (Some(from), Some(to)) = (
                target
                    .buffer
                    .iter_at_line_offset(line_index as i32, start as i32),
                target
                    .buffer
                    .iter_at_line_offset(line_index as i32, end as i32),
            ) else {
                continue;
            };
            target.buffer.apply_tag(&flash_tag, &from, &to);
        }
        if let Some(iter) = target.buffer.iter_at_line_offset(line_index as i32, 0) {
            search::reveal_iter(&self.diff_scroll, target, &iter);
        }
        let buffer = target.buffer.clone();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(1400), move || {
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            buffer.remove_tag(&flash_tag, &start, &end);
        });
    }

    /// Selects a commit in the History and loads its diff through the normal
    /// mechanism (GITILANTE_SEARCH_SPEC.md section 26): the commit works even
    /// when it is outside the loaded History blocks.
    fn navigate_to_commit(&self, oid: &str) {
        self.search_dialog.close();
        self.select(Selection::Commit(oid.to_owned()));
    }

    /// Toggles the folding of a file and re-renders keeping the viewport
    /// (GITILANTE_DIFF_FOLDING_SPEC.md sections 23-25).
    fn toggle_file_fold(&self, key: folding::FileFoldKey) {
        let focus = folding::FoldFocus::File(key.clone());
        self.with_fold_state(focus, |state| state.toggle_file(key));
    }

    fn toggle_hunk_fold(&self, key: folding::HunkFoldKey) {
        let focus = folding::FoldFocus::Hunk(key.clone());
        self.with_fold_state(focus, |state| state.toggle_hunk(key));
    }

    fn with_fold_state(
        &self,
        focus: folding::FoldFocus,
        apply: impl FnOnce(&mut folding::DiffFoldState),
    ) {
        let document = {
            let state = self.state.borrow();
            state
                .selection
                .as_ref()
                .and_then(folding::DiffDocumentKey::from_selection)
        };
        let Some(document) = document else {
            return;
        };
        apply(self.folds.borrow_mut().for_document(&document));
        // The focus lands on the toggled disclosure (section 26).
        self.pending_focus.set(Some(focus));
        self.rerender_diff();
    }

    /// Applies a global folding action (GITILANTE_DIFF_FOLDING_SPEC.md §22).
    fn fold_all(&self, action: folding::FoldAction) {
        let Some((document, diff)) = self.current_document() else {
            return;
        };
        self.folds
            .borrow_mut()
            .for_document(&document)
            .apply(action, &diff);
        self.rerender_diff();
    }

    /// The rendered document and its fold key
    /// (GITILANTE_DIFF_FOLDING_SPEC.md sections 15 and 21).
    fn current_document(&self) -> Option<(folding::DiffDocumentKey, Diff)> {
        let state = self.state.borrow();
        let selection = state.selection.as_ref()?;
        let key = folding::DiffDocumentKey::from_selection(selection)?;
        let diff = match selection {
            Selection::Commit(oid) => {
                let (_, diff) = state
                    .commit_diff
                    .as_ref()
                    .filter(|(cached, _)| cached == oid)?;
                diff.clone()
            }
            Selection::Staged(path) | Selection::Unstaged(path) | Selection::Conflicted(path) => {
                let data = state.data.as_ref()?;
                let side = match selection {
                    Selection::Staged(_) => &data.staged,
                    _ => &data.worktree,
                };
                Diff {
                    files: vec![find_file(side, path)?.clone()],
                }
            }
            _ => return None,
        };
        Some((key, diff))
    }

    /// Expands the collapsed elements whose text matches the query so the
    /// local search finds hidden matches too (GITILANTE_DIFF_FOLDING_SPEC.md
    /// sections 30-32).
    fn reveal_hidden_matches(&self, query: &str) {
        {
            let mut last = self.last_reveal_query.borrow_mut();
            if last.as_deref() == Some(query) {
                return;
            }
            *last = Some(query.to_owned());
        }
        if query.is_empty() {
            return;
        }
        let Some((document, diff)) = self.current_document() else {
            return;
        };
        let case_sensitive = crate::search::matcher::is_case_sensitive(query);
        let mut revealed = false;
        {
            let mut store = self.folds.borrow_mut();
            let state = store.for_document(&document);
            for file in &diff.files {
                let file_key = folding::FileFoldKey::from_file(file);
                for hunk in &file.hunks {
                    let hunk_key = folding::HunkFoldKey::from_hunk(file, hunk);
                    if !state.is_file_collapsed(&file_key) && !state.is_hunk_collapsed(&hunk_key) {
                        continue;
                    }
                    let hit = hunk.lines.iter().any(|line| {
                        !crate::search::matcher::find_matches(&line.text(), query, case_sensitive)
                            .is_empty()
                    });
                    if hit {
                        state.reveal_key(&hunk_key);
                        revealed = true;
                    }
                }
            }
        }
        // Section 32: revealed content stays revealed.
        if revealed {
            self.rerender_diff();
        }
    }

    /// Re-renders the current diff keeping the viewport
    /// (GITILANTE_DIFF_FOLDING_SPEC.md section 25).
    fn rerender_diff(&self) {
        let adjustment = self.diff_scroll.vadjustment();
        let anchor = adjustment.value();
        self.render_diff();
        gtk4::glib::idle_add_local_once(move || {
            adjustment.set_value(anchor);
        });
    }

    /// Opens the read-only preview of a file (GITILANTE_SEARCH_SPEC.md §16).
    fn navigate_to_file(&self, path: &Path, line: Option<usize>, ranges: &[MatchRange]) {
        self.search_dialog.close();
        {
            let mut state = self.state.borrow_mut();
            state.preview_flash = (!ranges.is_empty()).then(|| ranges.to_vec());
        }
        self.select(Selection::FilePreview {
            path: path.to_path_buf(),
            line,
        });
    }

    fn navigate_to_change(&self, result: &ChangeSearchResult) {
        self.search_dialog.close();
        // Reveal the target even when it is collapsed
        // (GITILANTE_DIFF_FOLDING_SPEC.md sections 30-31).
        let hunk_key = {
            let state = self.state.borrow();
            let side = state.data.as_ref().map(|data| match result.side {
                DiffSide::Staged => &data.staged,
                _ => &data.worktree,
            });
            side.and_then(|diff| find_file(diff, &result.path))
                .and_then(|file| {
                    file.hunks
                        .get(result.hunk_index)
                        .map(|hunk| folding::HunkFoldKey::from_hunk(file, hunk))
                })
        };
        if let (Some(key), Some(document)) = (
            hunk_key.as_ref(),
            folding::document_key_for_file(result.side, &result.path),
        ) {
            self.folds
                .borrow_mut()
                .for_document(&document)
                .reveal_key(key);
        }
        let selection = match result.side {
            DiffSide::Staged => Selection::Staged(result.path.clone()),
            _ => Selection::Unstaged(result.path.clone()),
        };
        self.select(selection);
        // Mark the file in the Changes sidebar too.
        self.render_changes();

        // Scroll to the matched line and flash it briefly (section 12).
        let target = {
            let state = self.state.borrow();
            state
                .render_targets
                .iter()
                .find(|target| target.hunk == hunk_key)
                .cloned()
        };
        let Some(target) = target else {
            return;
        };
        let flash = search::flash_tag(&target.buffer);
        for range in &result.match_ranges {
            let (Some(start), Some(end)) = (
                target
                    .buffer
                    .iter_at_line_offset(result.line_index as i32, range.start as i32),
                target
                    .buffer
                    .iter_at_line_offset(result.line_index as i32, range.end as i32),
            ) else {
                continue;
            };
            target.buffer.apply_tag(&flash, &start, &end);
        }
        let offset = result
            .match_ranges
            .first()
            .map(|range| range.start as i32)
            .unwrap_or(0);
        if let Some(iter) = target
            .buffer
            .iter_at_line_offset(result.line_index as i32, offset)
        {
            search::reveal_iter(&self.diff_scroll, &target, &iter);
        }
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(1400), move || {
            let start = target.buffer.start_iter();
            let end = target.buffer.end_iter();
            target.buffer.remove_tag(&flash, &start, &end);
        });
    }

    /// Rebuilds the Changes sidebar.
    fn render_changes(&self) {
        let (data, selection) = {
            let state = self.state.borrow();
            let Some(data) = state.data.as_ref() else {
                return;
            };
            (data.status.clone(), state.selection.clone())
        };
        self.changes.update(&data, selection.as_ref());
    }

    /// Rebuilds the History sidebar.
    fn render_history(&self) {
        let (commits, refs, head, selection, loading, exhausted, mode) = {
            let state = self.state.borrow();
            (
                state.commits.clone(),
                state.refs.clone(),
                state.head.clone(),
                state.selection.clone(),
                state.history_loading,
                state.history_exhausted,
                state.history_mode.clone(),
            )
        };
        // The layout is pure and cheap: computed here, never while drawing
        // (BRANCH.md section 53).
        let graph = crate::graph::layout_commits(&commits);
        let dark = adw::StyleManager::default().is_dark();
        // Rebuilding the rows must not move the viewport: anchor it to its
        // current position (BRANCH.md section 57).
        let adjustment = self.sidebar_adjustment.clone();
        let anchor = adjustment.value();
        self.history_view.update(
            &commits,
            &graph,
            &refs,
            head.as_ref(),
            dark,
            selection.as_ref(),
            loading,
            exhausted,
            match &mode {
                HistoryMode::Repository => None,
                HistoryMode::File { display_path, .. } => Some(display_path.as_path()),
            },
        );
        gtk4::glib::idle_add_local_once(move || {
            adjustment.set_value(anchor);
        });
    }

    /// Rebuilds the diff pane for the current selection.
    fn render_diff(&self) {
        self.update_fold_menu();
        let mut state = self.state.borrow_mut();
        let focus = self.pending_focus.take();
        // Focus tracking starts over with the new widgets.
        state.current_hunk = None;
        while let Some(child) = self.diff_box.first_child() {
            self.diff_box.remove(&child);
        }

        // Taken before the match: the pending flash belongs to this render
        // (GITILANTE_SEARCH_SPEC.md section 22).
        let mut flash = state.preview_flash.take();
        let rendered = match &state.selection {
            None => diff_view::RenderedDiff::plain(diff_view::placeholder(
                "Select a file to see its diff",
            )),
            Some(Selection::Untracked(path)) => {
                diff_view::render_untracked(path, &self.diff_callbacks())
            }
            Some(Selection::Staged(path)) => match state.data.as_ref() {
                Some(data) => {
                    self.render_file_diff(&data.staged, path, DiffSide::Staged, focus.as_ref())
                }
                None => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
            Some(Selection::Unstaged(path)) => match state.data.as_ref() {
                Some(data) => {
                    self.render_file_diff(&data.worktree, path, DiffSide::Unstaged, focus.as_ref())
                }
                None => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
            Some(Selection::Conflicted(path)) => match state.conflict_sessions.get(path) {
                Some(session) => conflict_view::render(
                    &session.borrow(),
                    &self.highlighter,
                    &self.conflict_callbacks(),
                    self.window.width(),
                ),
                None => match state.conflict_errors.get(path) {
                    Some(message) => {
                        diff_view::RenderedDiff::plain(diff_view::placeholder(message))
                    }
                    None => {
                        diff_view::RenderedDiff::plain(diff_view::placeholder("Loading conflict…"))
                    }
                },
            },
            Some(Selection::Commit(oid)) => match &state.commit_diff {
                Some((cached, diff)) if cached == oid => {
                    let paths = match &state.history_mode {
                        HistoryMode::Repository => None,
                        HistoryMode::File { .. } => state
                            .file_entries
                            .iter()
                            .find(|entry| entry.commit.oid == *oid)
                            .map(|entry| entry.paths.as_slice()),
                    };
                    self.render_commit_diff(oid, diff, focus.as_ref(), paths)
                }
                _ => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading diff…")),
            },
            Some(Selection::FilePreview { path, line }) => match &state.file_preview {
                Some((cached, content)) if cached == path => match content {
                    Ok(bytes) => {
                        let tracked = state.data.as_ref().is_some_and(|data| {
                            !data
                                .status
                                .untracked_entries()
                                .any(|entry| path.starts_with(&entry.path))
                        });
                        self.render_file_preview(path, bytes, *line, flash.take(), tracked)
                    }
                    Err(_) => diff_view::RenderedDiff::plain(diff_view::placeholder(
                        "Cannot read this file",
                    )),
                },
                _ => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
        };
        self.diff_box.append(&rendered.widget);
        // Ctrl+F searches the freshly rendered text (GITILANTE_SEARCH_SPEC.md §3);
        // the same targets drive the result navigation (section 12).
        state.render_targets = rendered.targets.clone();
        self.search_bar.set_targets(rendered.targets);
        self.diff_scroll.vadjustment().set_value(0.0);
        // A flash not consumed by a preview render stays pending.
        if flash.is_some() {
            state.preview_flash = flash;
        }
    }

    fn render_file_diff(
        &self,
        diff: &Diff,
        path: &Path,
        side: DiffSide,
        focus: Option<&folding::FoldFocus>,
    ) -> diff_view::RenderedDiff {
        match find_file(diff, path) {
            Some(file) => {
                // The fold state lives outside the Git model and survives
                // refreshes best-effort (GITILANTE_DIFF_FOLDING_SPEC.md
                // sections 11 and 14).
                let mut store = self.folds.borrow_mut();
                let empty = folding::DiffFoldState::default();
                let folds: &folding::DiffFoldState =
                    match folding::document_key_for_file(side, path) {
                        Some(key) => {
                            let state = store.for_document(&key);
                            state.prune(diff);
                            state
                        }
                        None => &empty,
                    };
                diff_view::render(
                    file,
                    side,
                    &self.highlighter,
                    folds,
                    focus,
                    &self.diff_callbacks(),
                )
            }
            None => diff_view::RenderedDiff::plain(diff_view::placeholder(
                "No diff available for this file",
            )),
        }
    }

    /// Renders the read-only file preview (GITILANTE_SEARCH_SPEC.md §16).
    fn render_file_preview(
        &self,
        path: &Path,
        bytes: &[u8],
        line: Option<usize>,
        flash: Option<Vec<MatchRange>>,
        tracked: bool,
    ) -> diff_view::RenderedDiff {
        // Binary files have no textual preview.
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return diff_view::RenderedDiff::plain(diff_view::placeholder("Binary file"));
        }
        let content = String::from_utf8_lossy(bytes);
        let rendered = diff_view::render_file_preview(
            path,
            &content,
            &self.highlighter,
            tracked,
            &self.diff_callbacks(),
        );
        // Reveal the requested line and flash its matches
        // (GITILANTE_SEARCH_SPEC.md section 22).
        if let (Some(line), Some(target)) = (line, rendered.targets.first()) {
            if let Some(iter) = target
                .buffer
                .iter_at_line_offset((line.saturating_sub(1)) as i32, 0)
            {
                search::reveal_iter(&self.diff_scroll, target, &iter);
            }
            if let Some(ranges) = flash {
                let flash_tag = search::flash_tag(&target.buffer);
                for range in ranges {
                    let (Some(start), Some(end)) = (
                        target.buffer.iter_at_line_offset(
                            (line.saturating_sub(1)) as i32,
                            range.start as i32,
                        ),
                        target
                            .buffer
                            .iter_at_line_offset((line.saturating_sub(1)) as i32, range.end as i32),
                    ) else {
                        continue;
                    };
                    target.buffer.apply_tag(&flash_tag, &start, &end);
                }
                let buffer = target.buffer.clone();
                gtk4::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(1400),
                    move || {
                        let start = buffer.start_iter();
                        let end = buffer.end_iter();
                        buffer.remove_tag(&flash_tag, &start, &end);
                    },
                );
            }
        }
        rendered
    }

    /// Renders every file of a commit diff with the same renderer (SPEC §17).
    fn render_commit_diff(
        &self,
        oid: &str,
        diff: &Diff,
        focus: Option<&folding::FoldFocus>,
        paths: Option<&[PathBuf]>,
    ) -> diff_view::RenderedDiff {
        if paths.is_some_and(|paths| paths.is_empty()) {
            log::debug!("File history entry {oid} has no reported paths");
            return diff_view::RenderedDiff::plain(diff_view::placeholder(
                "No file diff available for this history entry",
            ));
        }
        if diff.files.is_empty() {
            return diff_view::RenderedDiff::plain(diff_view::placeholder(
                "No changes in this commit",
            ));
        }
        let mut store = self.folds.borrow_mut();
        let key = folding::DiffDocumentKey::Commit(oid.to_owned());
        let folds = store.for_document(&key);
        folds.prune(diff);
        let box_ = GtkBox::new(Orientation::Vertical, 24);
        let mut targets = Vec::new();
        for file in &diff.files {
            if paths.is_some_and(|paths| {
                !paths.iter().any(|path| {
                    file.old_path.as_ref() == Some(path) || file.new_path.as_ref() == Some(path)
                })
            }) {
                continue;
            }
            let rendered = diff_view::render(
                file,
                DiffSide::History,
                &self.highlighter,
                folds,
                focus,
                &self.diff_callbacks(),
            );
            box_.append(&rendered.widget);
            targets.extend(rendered.targets);
        }
        if box_.first_child().is_none() {
            log::debug!("No file diff matches the lineage paths in commit {oid}");
            return diff_view::RenderedDiff::plain(diff_view::placeholder(
                "No file diff available for this history entry",
            ));
        }
        diff_view::RenderedDiff {
            widget: box_.upcast(),
            targets,
        }
    }

    fn diff_callbacks(&self) -> Rc<diff_view::Callbacks> {
        let weak = self.self_weak.clone();
        Rc::new(diff_view::Callbacks {
            copy: op_callback(&weak, Inner::copy_action),
            open_editor: op_callback(&weak, Inner::open_in_editor),
            stage_hunk: op2_callback(&weak, Inner::stage_hunk),
            unstage_hunk: op2_callback(&weak, Inner::unstage_hunk),
            discard_hunk: {
                let weak = weak.clone();
                Box::new(move |file: FileDiff, hunk: Hunk| {
                    if let Some(inner) = weak.upgrade() {
                        inner.confirm_discard_hunk(file, hunk);
                    }
                })
            },
            revert_hunk: op2_callback(&weak, Inner::revert_hunk),
            stage_lines: {
                let weak = weak.clone();
                Box::new(move |file, hunk, selected| {
                    if let Some(inner) = weak.upgrade() {
                        inner.stage_lines(file, hunk, selected);
                    }
                })
            },
            unstage_lines: {
                let weak = weak.clone();
                Box::new(move |file, hunk, selected| {
                    if let Some(inner) = weak.upgrade() {
                        inner.unstage_lines(file, hunk, selected);
                    }
                })
            },
            discard_lines: {
                let weak = weak.clone();
                Box::new(move |file, hunk, selected| {
                    if let Some(inner) = weak.upgrade() {
                        inner.confirm_discard_lines(file, hunk, selected);
                    }
                })
            },
            file_history: op_callback(&weak, Inner::open_file_history),
            preview_history: op_callback(&weak, Inner::open_preview_history),
            stage_file: op_callback(&weak, Inner::stage_file),
            unstage_file: op_callback(&weak, Inner::unstage_file),
            focus_hunk: op_callback(&weak, Inner::focus_hunk),
            toggle_file: {
                let weak = weak.clone();
                Box::new(move |key| {
                    if let Some(inner) = weak.upgrade() {
                        inner.toggle_file_fold(key);
                    }
                })
            },
            toggle_hunk: {
                let weak = weak.clone();
                Box::new(move |key| {
                    if let Some(inner) = weak.upgrade() {
                        inner.toggle_hunk_fold(key);
                    }
                })
            },
            discard_file: {
                let weak = weak.clone();
                Box::new(move |path: PathBuf| {
                    if let Some(inner) = weak.upgrade() {
                        inner.confirm_discard_file(path);
                    }
                })
            },
        })
    }

    fn open_in_editor(&self, path: PathBuf) {
        if let Err(error) = external_editor::open(self.repo.root(), &path) {
            log::warn!(
                "Could not open external editor for {}: {error:?}",
                path.display()
            );
            let toast = adw::Toast::new(error.message());
            toast.set_timeout(5);
            self.toast_overlay.add_toast(toast);
        }
    }

    // --- clipboard -------------------------------------------------------

    fn copy_action(&self, action: clipboard::CopyAction) {
        let message = match clipboard::copy(action, self.repo.root()) {
            Ok(message) | Err(message) => message,
        };
        let toast = adw::Toast::new(message);
        toast.set_timeout(3);
        self.toast_overlay.add_toast(toast);
    }

    // --- operations ------------------------------------------------------

    /// Remembers the hunk that has keyboard focus.
    fn focus_hunk(&self, target: HunkTarget) {
        self.state.borrow_mut().current_hunk = Some(target);
    }

    /// The focused hunk, when it comes from `side`.
    fn focused_hunk(&self, side: DiffSide) -> Option<HunkTarget> {
        self.state
            .borrow()
            .current_hunk
            .clone()
            .filter(|target| target.side == side)
    }

    fn shortcut_stage_hunk(&self) {
        if let Some(target) = self.focused_hunk(DiffSide::Unstaged) {
            self.stage_hunk(target.file, target.hunk);
        }
    }

    fn shortcut_unstage_hunk(&self) {
        if let Some(target) = self.focused_hunk(DiffSide::Staged) {
            self.unstage_hunk(target.file, target.hunk);
        }
    }

    fn shortcut_discard_hunk(&self) {
        if let Some(target) = self.focused_hunk(DiffSide::Unstaged) {
            self.confirm_discard_hunk(target.file, target.hunk);
        }
    }

    fn shortcut_revert_hunk(&self) {
        if let Some(target) = self.focused_hunk(DiffSide::History) {
            self.revert_hunk(target.file, target.hunk);
        }
    }

    fn stage_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.stage_hunk(&file, &hunk));
    }

    fn unstage_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.unstage_hunk(&file, &hunk));
    }

    fn discard_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.discard_hunk(&file, &hunk));
    }

    fn stage_lines(&self, file: FileDiff, hunk: Hunk, selected: Vec<usize>) {
        if !selected.is_empty() {
            self.run_op(move |repo| repo.stage_lines(&file, &hunk, &selected));
        }
    }

    fn unstage_lines(&self, file: FileDiff, hunk: Hunk, selected: Vec<usize>) {
        if !selected.is_empty() {
            self.run_op(move |repo| repo.unstage_lines(&file, &hunk, &selected));
        }
    }

    fn discard_lines(&self, file: FileDiff, hunk: Hunk, selected: Vec<usize>) {
        self.run_op(move |repo| repo.discard_lines(&file, &hunk, &selected));
    }

    fn revert_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.revert_commit_hunk(&file, &hunk));
    }

    fn open_file_history(&self, file: FileDiff) {
        let Some(display_path) = file.path().map(Path::to_path_buf) else {
            return;
        };
        let seed_path = if file.status == crate::model::diff::FileStatus::Renamed {
            file.old_path.clone()
        } else {
            // A staged rename can have additional unstaged changes whose diff
            // only mentions the new path; status still knows its HEAD path.
            self.entry_for(&display_path).and_then(|entry| {
                (entry.staged_change() == Some(crate::model::status::ChangeKind::Renamed))
                    .then_some(entry.orig_path)
                    .flatten()
            })
        }
        .unwrap_or_else(|| display_path.clone());
        self.set_history_mode(HistoryMode::File {
            display_path,
            seed_path,
        });
    }

    fn open_preview_history(&self, display_path: PathBuf) {
        let seed_path = self
            .entry_for(&display_path)
            .and_then(|entry| {
                (entry.staged_change() == Some(crate::model::status::ChangeKind::Renamed))
                    .then_some(entry.orig_path)
                    .flatten()
            })
            .unwrap_or_else(|| display_path.clone());
        self.set_history_mode(HistoryMode::File {
            display_path,
            seed_path,
        });
    }

    fn stage_file(&self, path: PathBuf) {
        self.run_file_op(path, Repository::stage_file);
    }

    fn unstage_file(&self, path: PathBuf) {
        self.run_file_op(path, Repository::unstage_file);
    }

    fn discard_file(&self, path: PathBuf) {
        self.run_file_op(path, Repository::discard_file);
    }

    fn run_file_op(&self, path: PathBuf, op: fn(&Repository, &StatusEntry) -> Result<(), Error>) {
        let Some(entry) = self.entry_for(&path) else {
            self.refresh();
            return;
        };
        self.run_op(move |repo| op(repo, &entry));
    }

    /// Runs a Git operation on the worker thread, then refreshes (SPEC §20).
    fn run_op<F>(&self, op: F)
    where
        F: FnOnce(&Repository) -> Result<(), Error> + Send + 'static,
    {
        self.busy_start();
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || op(&repo),
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.busy_finish();
                    if let Err(error) = result {
                        inner.show_error(&error);
                    }
                    inner.refresh();
                }
            },
        );
    }

    fn entry_for(&self, path: &Path) -> Option<StatusEntry> {
        let state = self.state.borrow();
        let data = state.data.as_ref()?;
        data.status
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
    }

    // --- destructive actions (SPEC section 14) ---------------------------

    fn confirm_discard_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.confirm_discard("Discard this hunk?", move |inner| {
            inner.discard_hunk(file, hunk);
        });
    }

    fn confirm_discard_lines(&self, file: FileDiff, hunk: Hunk, selected: Vec<usize>) {
        if selected.is_empty() {
            return;
        }
        let heading = format!("Discard {} selected changed lines?", selected.len());
        self.confirm_discard(&heading, move |inner| {
            inner.discard_lines(file, hunk, selected);
        });
    }

    fn confirm_discard_file(&self, path: PathBuf) {
        let heading = format!(
            "Discard changes to {}?",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        self.confirm_discard(&heading, move |inner| {
            inner.discard_file(path);
        });
    }

    /// Asks for an explicit confirmation before destroying local changes.
    fn confirm_discard(&self, heading: &str, action: impl FnOnce(&Inner) + 'static) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body("This will permanently remove these local changes.")
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("discard", "Discard");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);

        let weak = self.self_weak.clone();
        dialog.choose(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |response| {
                if response == "discard" {
                    if let Some(inner) = weak.upgrade() {
                        action(&inner);
                    }
                }
            },
        );
    }

    // --- errors (SPEC section 29) ---------------------------------------

    fn show_error(&self, error: &Error) {
        log::warn!("{error}");
        if let Error::Git(git_error) = error {
            log::warn!("{}", git_error.details());
        }
        let toast = adw::Toast::new(&toast_message(error));
        toast.set_timeout(6);
        self.toast_overlay.add_toast(toast);
    }
}

/// Requests more history when the sidebar is scrolled near its bottom.
fn hook_history_scroll(adjustment: &Adjustment, weak: Weak<Inner>) {
    let check: Rc<dyn Fn(&Adjustment)> = Rc::new(move |adjustment| {
        let near_bottom = adjustment.upper() - (adjustment.value() + adjustment.page_size())
            < HISTORY_SCROLL_THRESHOLD;
        if near_bottom {
            if let Some(inner) = weak.upgrade() {
                inner.request_history_more();
            }
        }
    });
    {
        let check = check.clone();
        adjustment.connect_value_changed(move |adjustment| check(adjustment));
    }
    {
        let check = check.clone();
        adjustment.connect_upper_notify(move |adjustment| check(adjustment));
    }
}

/// Builds a single-argument operation callback bound to `weak`.
fn op_callback<T: 'static>(weak: &Weak<Inner>, op: fn(&Inner, T)) -> Box<dyn Fn(T)> {
    let weak = weak.clone();
    Box::new(move |value| {
        if let Some(inner) = weak.upgrade() {
            op(&inner, value);
        }
    })
}

/// Builds a two-argument operation callback bound to `weak`.
fn op2_callback<T: 'static, U: 'static>(
    weak: &Weak<Inner>,
    op: fn(&Inner, T, U),
) -> Box<dyn Fn(T, U)> {
    let weak = weak.clone();
    Box::new(move |first, second| {
        if let Some(inner) = weak.upgrade() {
            op(&inner, first, second);
        }
    })
}

/// True when `selection` still exists in `status`.
fn selection_exists(status: &Status, selection: &Selection) -> bool {
    match selection {
        Selection::Staged(path) => status.staged_entries().any(|entry| entry.path == *path),
        Selection::Unstaged(path) => status.unstaged_entries().any(|entry| entry.path == *path),
        Selection::Untracked(path) => status.untracked_entries().any(|entry| entry.path == *path),
        Selection::Conflicted(path) => status.unmerged_entries().any(|entry| entry.path == *path),
        // Commits are immutable: a selected commit stays valid across refreshes.
        Selection::Commit(_) => true,
        Selection::FilePreview { .. } => true,
    }
}

/// First entry worth selecting when nothing is selected yet.
fn first_selection(status: &Status) -> Option<Selection> {
    status
        .unstaged_entries()
        .next()
        .map(|entry| Selection::Unstaged(entry.path.clone()))
        .or_else(|| {
            status
                .staged_entries()
                .next()
                .map(|entry| Selection::Staged(entry.path.clone()))
        })
        .or_else(|| {
            status
                .untracked_entries()
                .next()
                .map(|entry| Selection::Untracked(entry.path.clone()))
        })
}

/// Finds the file diff for `path`.
fn find_file<'a>(diff: &'a Diff, path: &Path) -> Option<&'a FileDiff> {
    diff.files
        .iter()
        .find(|file| file.path() == Some(path) || file.old_path.as_deref() == Some(path))
}

fn subtitle_text(root: &Path, status: &Status) -> String {
    use crate::model::status::Head;
    let branch = match &status.branch.head {
        Head::Branch(name) => name.clone(),
        Head::Detached => "(detached)".to_owned(),
        Head::Unknown => String::new(),
    };
    if branch.is_empty() {
        short_path(root)
    } else {
        format!("{} — {}", short_path(root), branch)
    }
}

/// Short, comprehensible message for a toast (SPEC section 29).
fn toast_message(error: &Error) -> String {
    match error {
        Error::PatchDoesNotApply { .. } => {
            "This hunk can no longer be applied because the file has changed.".to_owned()
        }
        Error::HunkCannotBeReverted { .. } => {
            "Cannot revert this hunk cleanly because the file has changed since this commit."
                .to_owned()
        }
        Error::Git(git_error) => {
            let detail = git_error
                .stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("unknown error")
                .trim();
            format!("Git command failed: {detail}")
        }
        other => other
            .to_string()
            .lines()
            .next()
            .unwrap_or("Unknown error")
            .to_owned(),
    }
}

#[cfg(test)]
mod file_history_tests {
    use super::*;

    #[test]
    fn mode_switch_invalidates_old_pages_and_resets_pagination() {
        let mut state = State::default();
        let repository_request = state.history_generation;
        state.history_loading = true;
        state.history_exhausted = true;
        assert!(state.change_history_mode(HistoryMode::File {
            display_path: PathBuf::from("a.txt"),
            seed_path: PathBuf::from("old.txt"),
        }));
        assert_ne!(repository_request, state.history_generation);
        assert!(!state.history_loading);
        assert!(!state.history_exhausted);
        let file_a_request = state.history_generation;
        assert!(state.change_history_mode(HistoryMode::File {
            display_path: PathBuf::from("b.txt"),
            seed_path: PathBuf::from("b.txt"),
        }));
        assert_ne!(file_a_request, state.history_generation);
        let file_b_request = state.history_generation;
        assert!(state.change_history_mode(HistoryMode::Repository));
        assert_ne!(file_b_request, state.history_generation);
    }
}
