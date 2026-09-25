//! Sidebar with the Staged, Unstaged, Conflicted and Untracked file lists (SPEC section 5).
//!
//! Each row shows the file name with its change letter and the whole-file
//! actions of its section (SPEC section 15): Stage, Unstage, Discard.
//! Section headers are collapsible: the collapse state lives outside the
//! rebuilt rows, so it survives refreshes and selection switches.

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

/// Sidebar section; every section header toggles its own collapse state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Staged,
    Unstaged,
    Conflicted,
    Untracked,
}

impl Group {
    const ALL: [Group; 4] = [
        Group::Staged,
        Group::Unstaged,
        Group::Conflicted,
        Group::Untracked,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn title(self) -> &'static str {
        match self {
            Group::Staged => "Staged",
            Group::Unstaged => "Unstaged",
            Group::Conflicted => "Conflicted",
            Group::Untracked => "Untracked",
        }
    }
}

/// One planned row of the sidebar list.
enum Row {
    /// Section header; activating it toggles the group.
    Header(Group),
    /// A file row with everything its widget needs.
    File(Box<FileRow>),
}

struct FileRow {
    selection: Selection,
    entry: StatusEntry,
    letter: char,
    buttons: &'static [(&'static str, Op)],
}

impl Row {
    fn selection(&self) -> Option<&Selection> {
        match self {
            Row::File(row) => Some(&row.selection),
            Row::Header(_) => None,
        }
    }
}

static STAGED_BUTTONS: [(&str, Op); 1] = [("Unstage", Op::Unstage)];
static UNSTAGED_BUTTONS: [(&str, Op); 2] = [("Stage", Op::Stage), ("Discard", Op::Discard)];
static UNTRACKED_BUTTONS: [(&str, Op); 1] = [("Stage", Op::Stage)];

/// Pure plan of the visible rows: every header always exists, its file rows
/// only when the group is expanded.
fn plan_rows(status: &Status, collapsed: &[bool; 4]) -> Vec<Row> {
    let mut rows = Vec::new();
    for group in Group::ALL {
        rows.push(Row::Header(group));
        if collapsed[group.index()] {
            continue;
        }
        let entries: Vec<StatusEntry> = match group {
            Group::Staged => status.staged_entries().cloned().collect(),
            Group::Unstaged => status.unstaged_entries().cloned().collect(),
            Group::Conflicted => status.unmerged_entries().cloned().collect(),
            Group::Untracked => status.untracked_entries().cloned().collect(),
        };
        for entry in entries {
            let (selection, letter, buttons) = match group {
                Group::Staged => (
                    Selection::Staged(entry.path.clone()),
                    entry.staged_change().map(change_letter).unwrap_or(' '),
                    &STAGED_BUTTONS[..],
                ),
                Group::Unstaged => (
                    Selection::Unstaged(entry.path.clone()),
                    entry.unstaged_change().map(change_letter).unwrap_or(' '),
                    &UNSTAGED_BUTTONS[..],
                ),
                Group::Conflicted => {
                    // The solver handles resolution; there is no whole-file
                    // action on a conflicted sidebar row.
                    (
                        Selection::Conflicted(entry.path.clone()),
                        'U',
                        &[] as &[(&str, Op)],
                    )
                }
                Group::Untracked => (
                    Selection::Untracked(entry.path.clone()),
                    '?',
                    &UNTRACKED_BUTTONS[..],
                ),
            };
            rows.push(Row::File(Box::new(FileRow {
                selection,
                entry,
                letter,
                buttons,
            })));
        }
    }
    rows
}

/// State shared with the list's signal handlers.
struct Shared {
    /// Planned rows of the current rebuild, aligned with `gtk::ListBoxRow::index`.
    rows: RefCell<Vec<Row>>,
    /// Collapse state of each section; survives refreshes.
    collapsed: RefCell<[bool; 4]>,
    /// Last rendered status/selection, re-rendered on collapse toggles.
    last_status: RefCell<Option<Status>>,
    last_selection: RefCell<Option<Selection>>,
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
    pub fn new(callbacks: Callbacks, initially_collapsed: bool) -> Self {
        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Single);
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(true);

        let shared = Rc::new(Shared {
            rows: RefCell::new(Vec::new()),
            collapsed: RefCell::new([initially_collapsed; 4]),
            last_status: RefCell::new(None),
            last_selection: RefCell::new(None),
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
                    .rows
                    .borrow()
                    .get(row.index() as usize)
                    .and_then(Row::selection)
                    .cloned();
                if let Some(selection) = selection {
                    *shared.last_selection.borrow_mut() = Some(selection.clone());
                    (shared.callbacks.select)(selection);
                }
            });
        }

        // Activating a section header toggles its collapse state.
        {
            let shared = shared.clone();
            list.connect_row_activated(move |list, row| {
                let index = row.index() as usize;
                let group = match shared.rows.borrow().get(index) {
                    Some(Row::Header(group)) => *group,
                    _ => return,
                };
                {
                    let mut collapsed = shared.collapsed.borrow_mut();
                    collapsed[group.index()] = !collapsed[group.index()];
                }
                rebuild(list, &shared);
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
        *self.shared.last_status.borrow_mut() = Some(status.clone());
        *self.shared.last_selection.borrow_mut() = selection.cloned();
        rebuild(&self.list, &self.shared);
    }

    /// Clears the row selection (used when a History commit is selected).
    pub fn clear_selection(&self) {
        self.shared.rebuilding.set(true);
        self.list.unselect_all();
        *self.shared.last_selection.borrow_mut() = None;
        self.shared.rebuilding.set(false);
    }
}

/// Rebuilds the whole list from the last stored status.
fn rebuild(list: &ListBox, shared: &Rc<Shared>) {
    shared.rebuilding.set(true);
    while let Some(row) = list.row_at_index(0) {
        clipboard::detach_context_menu(&row);
        list.remove(&row);
    }
    let mut rows = shared.rows.borrow_mut();
    rows.clear();

    let Some(status) = shared.last_status.borrow().clone() else {
        drop(rows);
        shared.rebuilding.set(false);
        return;
    };
    let planned = plan_rows(&status, &shared.collapsed.borrow());
    for row in &planned {
        match row {
            Row::Header(group) => push_header(list, shared, *group),
            Row::File(file) => push_entry(list, shared, file),
        }
    }
    rows.extend(planned);

    // Restore the selection on the matching row, if any.
    if let Some(selection) = shared.last_selection.borrow().as_ref() {
        for (index, row) in rows.iter().enumerate() {
            if row.selection() == Some(selection) {
                list.select_row(list.row_at_index(index as i32).as_ref());
                break;
            }
        }
    } else {
        list.unselect_all();
    }

    drop(rows);
    shared.rebuilding.set(false);
}

/// Appends a collapsible section header row.
fn push_header(list: &ListBox, shared: &Rc<Shared>, group: Group) {
    let row = ListBoxRow::new();
    row.set_activatable(true);
    row.set_selectable(false);

    let box_ = GtkBox::new(Orientation::Horizontal, 6);
    box_.set_margin_top(12);
    box_.set_margin_bottom(4);
    box_.set_margin_start(6);

    let collapsed = shared.collapsed.borrow()[group.index()];
    let arrow = Label::new(Some(if collapsed { "▶" } else { "▼" }));
    arrow.set_width_chars(1);
    arrow.add_css_class("dim-label");
    box_.append(&arrow);

    let label = Label::new(Some(group.title()));
    label.add_css_class("heading");
    label.set_xalign(0.0);
    box_.append(&label);

    row.set_child(Some(&box_));
    list.append(&row);
}

/// Appends a file row with its action buttons.
fn push_entry(list: &ListBox, shared: &Rc<Shared>, row: &FileRow) {
    let entry = &row.entry;
    let list_row = ListBoxRow::new();
    list_row.set_activatable(true);

    let box_ = GtkBox::new(Orientation::Horizontal, 6);
    box_.set_margin_top(4);
    box_.set_margin_bottom(4);
    box_.set_margin_start(6);
    box_.set_margin_end(6);

    let letter_label = Label::new(Some(&row.letter.to_string()));
    letter_label.add_css_class("dim-label");
    letter_label.set_width_chars(1);
    letter_label.set_xalign(0.5);
    box_.append(&letter_label);

    let name = Label::new(Some(&display_name(&entry.path, entry.orig_path.as_deref())));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(pango::EllipsizeMode::Middle);
    box_.append(&name);

    for (label, op) in row.buttons {
        let button = Button::with_label(label);
        button.add_css_class("flat");
        button.set_valign(Align::Center);
        button.set_tooltip_text(Some(&tooltip(*op, &entry.path)));
        let path = entry.path.clone();
        let shared = shared.clone();
        let op = *op;
        button.connect_clicked(move |_| match op {
            Op::Stage => (shared.callbacks.stage_file)(path.clone()),
            Op::Unstage => (shared.callbacks.unstage_file)(path.clone()),
            Op::Discard => (shared.callbacks.discard_file)(path.clone()),
        });
        box_.append(&button);
    }

    list_row.set_child(Some(&box_));
    let path = entry.path.clone();
    let absolute_path = path.clone();
    let copy_shared = shared.clone();
    let absolute_shared = shared.clone();
    let editor_shared = shared.clone();
    let editor_path = entry.path.clone();
    clipboard::context_menu(
        &list_row,
        vec![
            (
                "Open in Editor",
                Box::new(move || (editor_shared.callbacks.open_editor)(editor_path.clone())),
            ),
            (
                "Copy path",
                Box::new(move || (copy_shared.callbacks.copy)(CopyAction::Path(path.clone()))),
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
    list.append(&list_row);
}

fn tooltip(op: Op, path: &std::path::Path) -> String {
    let name = display_name(path, None);
    match op {
        Op::Stage => format!("Stage all changes of {name}"),
        Op::Unstage => format!("Unstage all changes of {name}"),
        Op::Discard => format!("Discard the working tree changes of {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::status::{
        BranchInfo, ChangeKind, ConflictStage, StatusEntryKind, SubmoduleState, UnmergedCode,
        UnmergedInfo,
    };

    fn entry(path: &str, index: ChangeKind, worktree: ChangeKind) -> StatusEntry {
        StatusEntry {
            path: PathBuf::from(path),
            orig_path: None,
            kind: StatusEntryKind::Tracked { index, worktree },
        }
    }

    fn status() -> Status {
        Status {
            branch: BranchInfo::default(),
            entries: vec![
                entry("staged.rs", ChangeKind::Modified, ChangeKind::Unmodified),
                entry("both.rs", ChangeKind::Modified, ChangeKind::Modified),
                entry("work.rs", ChangeKind::Unmodified, ChangeKind::Modified),
                StatusEntry {
                    path: PathBuf::from("new.rs"),
                    orig_path: None,
                    kind: StatusEntryKind::Untracked,
                },
            ],
        }
    }

    fn summary(rows: &[Row]) -> Vec<(Option<Group>, String)> {
        rows.iter()
            .map(|row| match row {
                Row::Header(group) => (Some(*group), String::new()),
                Row::File(file) => (None, file.entry.path.display().to_string()),
            })
            .collect()
    }

    #[test]
    fn expanded_groups_plan_header_and_file_rows() {
        let rows = plan_rows(&status(), &[false; 4]);
        assert_eq!(
            summary(&rows),
            vec![
                (Some(Group::Staged), String::new()),
                (None, "staged.rs".to_owned()),
                (None, "both.rs".to_owned()),
                (Some(Group::Unstaged), String::new()),
                (None, "both.rs".to_owned()),
                (None, "work.rs".to_owned()),
                (Some(Group::Conflicted), String::new()),
                (Some(Group::Untracked), String::new()),
                (None, "new.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn file_history_startup_can_begin_with_every_group_collapsed() {
        let rows = plan_rows(&status(), &[true; 4]);
        assert_eq!(
            summary(&rows),
            Group::ALL
                .into_iter()
                .map(|group| (Some(group), String::new()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn collapsed_groups_keep_the_header_and_hide_their_rows() {
        let mut collapsed = [false; 4];
        collapsed[Group::Unstaged.index()] = true;
        let rows = plan_rows(&status(), &collapsed);
        assert_eq!(
            summary(&rows),
            vec![
                (Some(Group::Staged), String::new()),
                (None, "staged.rs".to_owned()),
                (None, "both.rs".to_owned()),
                (Some(Group::Unstaged), String::new()),
                (Some(Group::Conflicted), String::new()),
                (Some(Group::Untracked), String::new()),
                (None, "new.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn conflicted_group_can_hide_and_restore_its_file_rows() {
        let mut status = status();
        status.entries.push(StatusEntry {
            path: PathBuf::from("conflict.rs"),
            orig_path: None,
            kind: StatusEntryKind::Unmerged(UnmergedInfo {
                code: UnmergedCode::BothModified,
                submodule: SubmoduleState::NotSubmodule,
                base: ConflictStage::default(),
                stage2: ConflictStage::default(),
                stage3: ConflictStage::default(),
                worktree_mode: None,
            }),
        });
        let expanded = plan_rows(&status, &[false; 4]);
        assert!(expanded.iter().any(|row| {
            row.selection() == Some(&Selection::Conflicted(PathBuf::from("conflict.rs")))
        }));
        let mut collapsed = [false; 4];
        collapsed[Group::Conflicted.index()] = true;
        let hidden = plan_rows(&status, &collapsed);
        assert!(
            hidden
                .iter()
                .any(|row| matches!(row, Row::Header(Group::Conflicted)))
        );
        assert!(!hidden.iter().any(|row| {
            row.selection() == Some(&Selection::Conflicted(PathBuf::from("conflict.rs")))
        }));
    }

    #[test]
    fn file_rows_carry_their_section_selection_and_actions() {
        let rows = plan_rows(&status(), &[false; 4]);
        let files: Vec<&FileRow> = rows
            .iter()
            .filter_map(|row| match row {
                Row::File(file) => Some(file.as_ref()),
                Row::Header(_) => None,
            })
            .collect();
        assert_eq!(
            files[0].selection,
            Selection::Staged(PathBuf::from("staged.rs"))
        );
        assert_eq!(files[0].letter, 'M');
        assert_eq!(files[0].buttons, &STAGED_BUTTONS[..]);
        assert_eq!(
            files[2].selection,
            Selection::Unstaged(PathBuf::from("both.rs"))
        );
        assert_eq!(files[2].buttons, &UNSTAGED_BUTTONS[..]);
        assert_eq!(
            files[4].selection,
            Selection::Untracked(PathBuf::from("new.rs"))
        );
        assert_eq!(files[4].letter, '?');
        assert_eq!(files[4].buttons, &UNTRACKED_BUTTONS[..]);
    }
}
