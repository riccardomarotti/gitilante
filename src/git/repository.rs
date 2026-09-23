//! Repository discovery and high-level operations.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::git::patch::{self, ApplyTarget};
use crate::model::diff::{Diff, FileDiff, Hunk};
use crate::model::status::{Status, StatusEntry};

/// A Git repository, rooted at its working tree root.
///
/// All operations run `git -C <root>`; the process working directory is never
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    root: PathBuf,
}

impl Repository {
    /// Opens the repository containing `path`.
    ///
    /// `path` may be the repository root or any subdirectory of it; the
    /// repository root is resolved with `git rev-parse --show-toplevel`.
    pub fn discover(path: &Path) -> Result<Self> {
        let output = match command::run(path, &["rev-parse", "--show-toplevel"]) {
            Ok(output) => output,
            // Any `rev-parse` failure here means the path is not inside a work tree.
            Err(Error::Git(_)) => {
                return Err(Error::NotARepository {
                    path: path.to_path_buf(),
                });
            }
            Err(error) => return Err(error),
        };

        // `--show-toplevel` prints the root followed by a newline (or NUL with -z);
        // strip trailing separators to get the exact pathname.
        let root_bytes: Vec<u8> = output
            .stdout
            .iter()
            .copied()
            .take_while(|byte| *byte != 0 && *byte != b'\n')
            .collect();
        if root_bytes.is_empty() {
            return Err(Error::NotARepository {
                path: path.to_path_buf(),
            });
        }

        Ok(Self {
            root: PathBuf::from(OsString::from_vec(root_bytes)),
        })
    }

    /// Working tree root of the repository.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the current status of the working tree and index.
    pub fn status(&self) -> Result<Status> {
        crate::git::status::status(self)
    }

    /// Diff of the working tree against the index (unstaged changes).
    pub fn working_tree_diff(&self) -> Result<Diff> {
        crate::git::diff::working_tree_diff(self)
    }

    /// Diff of the index against HEAD (staged changes).
    pub fn staged_diff(&self) -> Result<Diff> {
        crate::git::diff::staged_diff(self)
    }

    /// Stages a single hunk of a working tree diff into the index (SPEC §12).
    ///
    /// The hunk is rebuilt into a minimal patch and applied with
    /// `git apply --cached` after a dry run, so a stale hunk changes nothing.
    pub fn stage_hunk(&self, file: &FileDiff, hunk: &Hunk) -> Result<()> {
        patch::apply(
            &self.root,
            &patch::single_hunk_patch(file, hunk),
            ApplyTarget::Index,
            false,
        )
    }

    /// Removes a single hunk of a staged diff from the index (SPEC §13).
    ///
    /// The hunk is rebuilt into a minimal patch and reverse-applied to the
    /// index with `git apply --cached --reverse` after a dry run.
    pub fn unstage_hunk(&self, file: &FileDiff, hunk: &Hunk) -> Result<()> {
        patch::apply(
            &self.root,
            &patch::single_hunk_patch(file, hunk),
            ApplyTarget::Index,
            true,
        )
    }

    /// Discards a single hunk of a working tree diff (SPEC §14).
    ///
    /// This destroys uncommitted local changes: the UI must ask for an explicit
    /// confirmation before calling it.
    pub fn discard_hunk(&self, file: &FileDiff, hunk: &Hunk) -> Result<()> {
        patch::apply(
            &self.root,
            &patch::single_hunk_patch(file, hunk),
            ApplyTarget::WorkTree,
            true,
        )
    }

    /// Stages every change of the entry's path (SPEC §15).
    ///
    /// Works for modified files, whole deletions and untracked files.
    pub fn stage_file(&self, entry: &StatusEntry) -> Result<()> {
        self.run_on_paths(&["add"], entry, false)
    }

    /// Removes every staged change of the entry's path from the index (SPEC §15).
    ///
    /// Uses `git reset` so it also works in repositories without commits.
    pub fn unstage_file(&self, entry: &StatusEntry) -> Result<()> {
        self.run_on_paths(&["reset", "-q", "HEAD"], entry, true)
    }

    /// Discards the working tree changes of the entry's path (SPEC §15).
    ///
    /// Staged changes are preserved. This destroys uncommitted local changes:
    /// the UI must ask for an explicit confirmation before calling it.
    pub fn discard_file(&self, entry: &StatusEntry) -> Result<()> {
        self.run_on_paths(&["restore", "--worktree"], entry, false)
    }

    /// Runs a command on the entry's path, adding the original path of a
    /// renamed entry when requested (unstaging a rename must restore both).
    fn run_on_paths(&self, args: &[&str], entry: &StatusEntry, with_orig_path: bool) -> Result<()> {
        let mut command_args: Vec<&OsStr> = args.iter().copied().map(OsStr::new).collect();
        command_args.push(OsStr::new("--"));
        command_args.push(entry.path.as_os_str());
        if with_orig_path {
            if let Some(orig_path) = &entry.orig_path {
                command_args.push(orig_path.as_os_str());
            }
        }
        command::run(&self.root, &command_args)?;
        Ok(())
    }
}
