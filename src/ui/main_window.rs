//! Main window: sidebar + diff view, refresh handling and error reporting.
//!
//! Responsibilities (SPEC sections 14, 20, 21, 29):
//! - runs every Git operation on the worker thread and refreshes afterwards;
//! - drops stale results with a generation counter;
//! - refreshes when the window regains focus;
//! - reports errors with toasts, reserving dialogs for decisions (discard).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation, Paned, PolicyType, ScrolledWindow};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::git::Error;
use crate::git::repository::Repository;
use crate::model::diff::{Diff, FileDiff, Hunk};
use crate::model::status::{Status, StatusEntry};
use crate::ui::worker::Worker;
use crate::ui::{DiffSide, Selection, changes, diff_view, short_path};

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
}

struct Inner {
    repo: Repository,
    worker: Worker,
    state: RefCell<State>,
    self_weak: Weak<Inner>,
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    subtitle: adw::WindowTitle,
    changes: changes::ChangesView,
    diff_box: GtkBox,
    diff_scroll: ScrolledWindow,
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

        let changes = changes::ChangesView::new(changes::Callbacks {
            select: {
                let weak = self_weak.clone();
                Box::new(move |selection| {
                    if let Some(inner) = weak.upgrade() {
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

        let sidebar_scroll = ScrolledWindow::new();
        sidebar_scroll.set_child(Some(changes.widget()));
        sidebar_scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        sidebar_scroll.set_width_request(280);

        let diff_scroll = ScrolledWindow::new();
        diff_scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);
        diff_scroll.set_hexpand(true);
        diff_scroll.set_vexpand(true);
        let diff_box = GtkBox::new(Orientation::Vertical, 0);
        diff_scroll.set_child(Some(&diff_box));

        let paned = Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar_scroll));
        paned.set_end_child(Some(&diff_scroll));
        paned.set_position(300);
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

        Self {
            repo,
            worker: Worker::new(),
            state: RefCell::new(State::default()),
            self_weak,
            window,
            toast_overlay,
            subtitle,
            changes,
            diff_box,
            diff_scroll,
        }
    }

    /// Reloads status and diffs from Git (SPEC section 20).
    fn refresh(&self) {
        let generation = {
            let mut state = self.state.borrow_mut();
            state.generation += 1;
            state.generation
        };
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
    }

    /// Applies a refresh result unless a newer refresh superseded it.
    fn finish_refresh(&self, generation: u64, result: Result<RefreshData, Error>) {
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
                // Show a diff right away: when nothing is selected (startup or a
                // selection that vanished), pick the first available entry.
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
        let data = state.data.as_ref().expect("just set");
        let selection = state.selection.clone();
        self.subtitle
            .set_subtitle(&subtitle_text(&self.repo, &data.status));
        self.changes.update(&data.status, selection.as_ref());
        self.render_diff(data, selection.as_ref());
    }

    /// Handles sidebar row selection.
    fn select(&self, selection: Selection) {
        self.state.borrow_mut().selection = Some(selection);
        let state = self.state.borrow();
        let Some(data) = state.data.as_ref() else {
            return;
        };
        self.render_diff(data, state.selection.as_ref());
    }

    /// Rebuilds the diff pane for the current selection.
    fn render_diff(&self, data: &RefreshData, selection: Option<&Selection>) {
        while let Some(child) = self.diff_box.first_child() {
            self.diff_box.remove(&child);
        }

        let widget = match selection {
            None => diff_view::placeholder("Select a file to see its diff"),
            Some(Selection::Untracked(path)) => {
                diff_view::render_untracked(path, &self.diff_callbacks())
            }
            Some(Selection::Staged(path)) => {
                self.render_file_diff(&data.staged, path, DiffSide::Staged)
            }
            Some(Selection::Unstaged(path)) => {
                self.render_file_diff(&data.worktree, path, DiffSide::Unstaged)
            }
        };
        self.diff_box.append(&widget);
        self.diff_scroll.vadjustment().set_value(0.0);
    }

    fn render_file_diff(&self, diff: &Diff, path: &Path, side: DiffSide) -> gtk4::Widget {
        match find_file(diff, path) {
            Some(file) => diff_view::render(file, side, &self.diff_callbacks()),
            None => diff_view::placeholder("No diff available for this file"),
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
            stage_file: op_callback(&weak, Inner::stage_file),
            unstage_file: op_callback(&weak, Inner::unstage_file),
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

    fn stage_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.stage_hunk(&file, &hunk));
    }

    fn unstage_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.unstage_hunk(&file, &hunk));
    }

    fn discard_hunk(&self, file: FileDiff, hunk: Hunk) {
        self.run_op(move |repo| repo.discard_hunk(&file, &hunk));
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
        let repo = self.repo.clone();
        let weak = self.self_weak.clone();
        self.worker.run(
            move || op(&repo),
            move |result| {
                if let Some(inner) = weak.upgrade() {
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

fn subtitle_text(repo: &Repository, status: &Status) -> String {
    use crate::model::status::Head;
    let branch = match &status.branch.head {
        Head::Branch(name) => name.clone(),
        Head::Detached => "(detached)".to_owned(),
        Head::Unknown => String::new(),
    };
    if branch.is_empty() {
        short_path(repo.root())
    } else {
        format!("{} — {}", short_path(repo.root()), branch)
    }
}

/// Short, comprehensible message for a toast (SPEC section 29).
fn toast_message(error: &Error) -> String {
    match error {
        Error::PatchDoesNotApply { .. } => {
            "This hunk can no longer be applied because the file has changed.".to_owned()
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
