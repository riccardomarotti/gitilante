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

use crate::git::Error;
use crate::git::repository::Repository;
use crate::model::commit::Commit;
use crate::model::diff::{Diff, FileDiff, Hunk};
use crate::model::refs::{CommitRef, HeadRef};
use crate::model::status::{Status, StatusEntry};
use crate::search::SearchQuery;
use crate::search::SearchScope;
use crate::search::result::{ChangeSearchResult, MatchRange, SearchResult};
use crate::ui::worker::Worker;
use crate::ui::{
    DiffSide, HunkTarget, Selection, changes, diff_view, history, search, search_dialog, short_path,
};

/// Commits per history block (SPEC section 16).
const HISTORY_PAGE: usize = 200;

/// Distance from the sidebar bottom that triggers loading more history.
const HISTORY_SCROLL_THRESHOLD: f64 = 300.0;

/// Everything read from Git in one history block: the commits of the page plus
/// the refs and HEAD needed by the badges (BRANCH.md sections 4-11).
type HistoryData = (
    Vec<Commit>,
    HashMap<String, Vec<CommitRef>>,
    Option<HeadRef>,
);

/// Everything read from Git in one refresh.
struct RefreshData {
    status: Status,
    worktree: Diff,
    staged: Diff,
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
    /// Bumped on every selection change; stale commit diffs are dropped.
    commit_diff_generation: u64,
    /// Hunk that has keyboard focus, target of the contextual shortcuts.
    current_hunk: Option<HunkTarget>,
    /// Searchable text of the currently rendered diff (section 3).
    render_targets: Vec<crate::ui::diff_view::SearchTarget>,
}

struct Inner {
    repo: Repository,
    worker: Worker,
    state: RefCell<State>,
    /// In-flight requests, shown by the header bar spinner.
    busy: Cell<u32>,
    /// Shared syntax highlighting resources (COLORS.md section 49).
    highlighter: crate::syntax::Highlighter,
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
    /// Bumped on every search: stale results are dropped (section 35).
    search_generation: Cell<u64>,
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

        // Loading state: spins while any Git request is in flight (SPEC §35).
        let spinner = gtk4::Spinner::new();
        spinner.set_visible(false);
        spinner.set_tooltip_text(Some("Loading…"));
        header.pack_end(&spinner);

        let changes = changes::ChangesView::new(changes::Callbacks {
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
            select: {
                let weak = self_weak.clone();
                Box::new(move |selection| {
                    if let Some(inner) = weak.upgrade() {
                        inner.changes.clear_selection();
                        inner.select(selection);
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
            search::SearchBar::new(
                diff_scroll.clone(),
                Box::new(move |focused| {
                    // Typing must never trigger the hunk shortcuts (section 51).
                    crate::ui::app::suspend_hunk_shortcuts(&app, focused);
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
                activate: {
                    let weak = self_weak.clone();
                    Box::new(move |result| {
                        if let Some(inner) = weak.upgrade() {
                            inner.activate_search_result(result);
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
                    if let Some(inner) = weak.upgrade() {
                        inner.refresh();
                    }
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
            self_weak,
            window,
            toast_overlay,
            subtitle,
            spinner,
            changes,
            history_view,
            search_bar,
            search_dialog,
            search_generation: Cell::new(0),
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
        let count = {
            let state = self.state.borrow();
            state.commits.len().max(HISTORY_PAGE)
        };
        self.request_history(false, 0, count);
    }

    /// Loads the next block of history (SPEC sections 16 and 22).
    fn request_history_more(&self) {
        let skip = self.state.borrow().commits.len();
        self.request_history(true, skip, HISTORY_PAGE);
    }

    /// Single-flight history request: `append` adds to the loaded commits,
    /// otherwise the list is replaced.
    fn request_history(&self, append: bool, skip: usize, count: usize) {
        {
            let mut state = self.state.borrow_mut();
            if state.history_loading || (append && state.history_exhausted) {
                return;
            }
            state.history_loading = true;
        }
        self.busy_start();
        self.render_history();

        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || -> Result<HistoryData, Error> {
                // Refs and HEAD are read with the page so the badges follow
                // the normal refresh cycle (BRANCH.md section 50).
                let commits = repo.history(skip, count)?;
                let refs = repo.commit_refs()?;
                let head = repo.head()?;
                Ok((commits, refs, head))
            },
            move |result| {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_history(append, count, result);
                }
            },
        );
    }

    fn finish_history(&self, append: bool, requested: usize, result: Result<HistoryData, Error>) {
        self.busy_finish();
        match result {
            Ok((commits, refs, head)) => {
                let mut state = self.state.borrow_mut();
                state.history_loading = false;
                state.history_exhausted = commits.len() < requested;
                state.refs = refs;
                state.head = head;
                if append {
                    state.commits.extend(commits);
                } else {
                    state.commits = commits;
                }
            }
            Err(error) => {
                self.state.borrow_mut().history_loading = false;
                self.show_error(&error);
            }
        }
        self.render_history();
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
                    let selection = state
                        .selection
                        .clone()
                        .filter(|selection| selection_exists(&data.status, selection));
                    state.data = Some(data);
                    // Show a diff right away: when nothing is selected (startup
                    // or a selection that vanished), pick the first entry.
                    state.selection = selection.or_else(|| {
                        let data = state.data.as_ref().expect("just set");
                        first_selection(&data.status)
                    });
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

    /// Runs the global search on the worker thread
    /// (GITILANTE_SEARCH_SPEC.md sections 52-53).
    fn run_search(&self, query: SearchQuery) {
        let generation = self.search_generation.get() + 1;
        self.search_generation.set(generation);
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
                results
            },
            move |results| {
                if let Some(inner) = weak.upgrade() {
                    // A stale search never overwrites newer ones (section 35).
                    if inner.search_generation.get() == generation {
                        inner.search_dialog.show_results(results);
                    }
                }
            },
        );
    }

    /// Navigates to an activated search result (GITILANTE_SEARCH_SPEC.md §12).
    fn activate_search_result(&self, result: SearchResult) {
        match result {
            SearchResult::Change(change) => self.navigate_to_change(&change),
            SearchResult::File(file) => self.navigate_to_file(&file.path, None, &[]),
            SearchResult::Content(content) => self.navigate_to_file(
                &content.path,
                Some(content.line_number),
                &content.match_ranges,
            ),
            // The other providers arrive with their phases.
            _ => {}
        }
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
            state.render_targets.get(result.hunk_index).cloned()
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
        let (commits, refs, head, selection, loading, exhausted) = {
            let state = self.state.borrow();
            (
                state.commits.clone(),
                state.refs.clone(),
                state.head.clone(),
                state.selection.clone(),
                state.history_loading,
                state.history_exhausted,
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
        );
        gtk4::glib::idle_add_local_once(move || {
            adjustment.set_value(anchor);
        });
    }

    /// Rebuilds the diff pane for the current selection.
    fn render_diff(&self) {
        let mut state = self.state.borrow_mut();
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
                Some(data) => self.render_file_diff(&data.staged, path, DiffSide::Staged),
                None => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
            Some(Selection::Unstaged(path)) => match state.data.as_ref() {
                Some(data) => self.render_file_diff(&data.worktree, path, DiffSide::Unstaged),
                None => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
            Some(Selection::Conflicted(path)) => match state.data.as_ref() {
                Some(data) => self.render_file_diff(&data.worktree, path, DiffSide::Conflicted),
                None => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading…")),
            },
            Some(Selection::Commit(oid)) => match &state.commit_diff {
                Some((cached, diff)) if cached == oid => self.render_commit_diff(diff),
                _ => diff_view::RenderedDiff::plain(diff_view::placeholder("Loading diff…")),
            },
            Some(Selection::FilePreview { path, line }) => match &state.file_preview {
                Some((cached, content)) if cached == path => match content {
                    Ok(bytes) => self.render_file_preview(path, bytes, *line, flash.take()),
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
    ) -> diff_view::RenderedDiff {
        match find_file(diff, path) {
            Some(file) => diff_view::render(file, side, &self.highlighter, &self.diff_callbacks()),
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
    ) -> diff_view::RenderedDiff {
        // Binary files have no textual preview.
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return diff_view::RenderedDiff::plain(diff_view::placeholder("Binary file"));
        }
        let content = String::from_utf8_lossy(bytes);
        let rendered = diff_view::render_file_preview(path, &content, &self.highlighter);
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
    fn render_commit_diff(&self, diff: &Diff) -> diff_view::RenderedDiff {
        if diff.files.is_empty() {
            return diff_view::RenderedDiff::plain(diff_view::placeholder(
                "No changes in this commit",
            ));
        }
        let box_ = GtkBox::new(Orientation::Vertical, 24);
        let mut targets = Vec::new();
        for file in &diff.files {
            let rendered = diff_view::render(
                file,
                DiffSide::History,
                &self.highlighter,
                &self.diff_callbacks(),
            );
            box_.append(&rendered.widget);
            targets.extend(rendered.targets);
        }
        diff_view::RenderedDiff {
            widget: box_.upcast(),
            targets,
        }
    }

    fn diff_callbacks(&self) -> Rc<diff_view::Callbacks> {
        let weak = self.self_weak.clone();
        Rc::new(diff_view::Callbacks {
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
            stage_file: op_callback(&weak, Inner::stage_file),
            unstage_file: op_callback(&weak, Inner::unstage_file),
            focus_hunk: op_callback(&weak, Inner::focus_hunk),
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

    fn revert_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.revert_commit_hunk(&file, &hunk));
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
