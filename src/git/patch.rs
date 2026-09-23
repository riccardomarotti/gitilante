//! Patch reconstruction and application.
//!
//! Turns parts of a parsed diff back into unified diff text that `git apply`
//! accepts, and applies patches with a mandatory dry run first: a patch built
//! from an outdated diff must never modify the repository.

use std::path::Path;

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::model::diff::{DiffLineKind, FileDiff, Hunk};

/// What a patch is applied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTarget {
    /// The index only (`git apply --cached`), leaving the working tree untouched.
    Index,
    /// The working tree (`git apply`), leaving the index untouched.
    WorkTree,
}

/// Checks that `patch` applies cleanly, then applies it.
///
/// The check is a dry run (`git apply --check`): if it fails the repository is
/// left untouched and [`Error::PatchDoesNotApply`] is returned. `reverse`
/// applies the patch backwards, undoing the change it describes.
pub fn apply(root: &Path, patch: &[u8], target: ApplyTarget, reverse: bool) -> Result<()> {
    let mut args = vec!["apply"];
    if target == ApplyTarget::Index {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }

    // Dry run first: never modify anything on a stale or conflicting patch.
    let mut check_args = args.clone();
    check_args.extend(["--check", "-"]);
    match command::run_with_stdin(root, &check_args, Some(patch)) {
        Ok(_) => {}
        Err(Error::Git(git_error)) => {
            return Err(Error::PatchDoesNotApply {
                stderr: git_error.stderr,
            });
        }
        Err(error) => return Err(error),
    }

    args.push("-");
    command::run_with_stdin(root, &args, Some(patch))?;
    Ok(())
}

/// Rebuilds a minimal valid patch containing only `hunk` of `file`.
///
/// The patch repeats the file's `diff --git` line and its extended headers
/// verbatim, then the selected hunk, so it applies exactly like the original
/// diff while touching only that hunk.
pub fn single_hunk_patch(file: &FileDiff, hunk: &Hunk) -> Vec<u8> {
    let mut patch = Vec::new();
    push_line(&mut patch, &file.header);
    for line in &file.metadata {
        push_line(&mut patch, line);
    }
    push_line(&mut patch, &hunk.header);
    for line in &hunk.lines {
        patch.push(marker(line.kind));
        patch.extend_from_slice(&line.content);
        patch.push(b'\n');
    }
    patch
}

/// Unified diff marker byte of a line kind.
fn marker(kind: DiffLineKind) -> u8 {
    match kind {
        DiffLineKind::Context => b' ',
        DiffLineKind::Addition => b'+',
        DiffLineKind::Deletion => b'-',
        DiffLineKind::NoNewlineMarker => b'\\',
    }
}

fn push_line(patch: &mut Vec<u8>, line: &[u8]) {
    patch.extend_from_slice(line);
    patch.push(b'\n');
}
