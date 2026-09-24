//! Patch reconstruction and application.
//!
//! Turns parts of a parsed diff back into unified diff text that `git apply`
//! accepts, and applies patches with a mandatory dry run first: a patch built
//! from an outdated diff must never modify the repository.

use std::path::Path;

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::model::diff::{DiffLineKind, FileDiff, FileStatus, Hunk};

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

/// Only ordinary modified text hunks are safe for partial application.
pub fn supports_selected_lines(file: &FileDiff, hunk: &Hunk) -> bool {
    file.status == FileStatus::Modified
        && !file.binary
        && !file
            .metadata
            .iter()
            .any(|line| line.starts_with(b"old mode ") || line.starts_with(b"new mode "))
        && !hunk
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::NoNewlineMarker)
}

/// Rebuilds one hunk with only the selected changed lines.
///
/// For forward application, unselected deletions become context and additions
/// disappear. Reverse application instead keeps the new-side text as context.
/// Raw bytes are preserved so non-UTF-8 lines remain safe to apply.
pub fn selected_lines_patch(file: &FileDiff, hunk: &Hunk, selected: &[usize]) -> Result<Vec<u8>> {
    selected_lines_patch_for_direction(file, hunk, selected, false)
}

/// When reversing a patch, unselected lines must match the new side of the
/// diff (the current index or worktree), not the old side.
pub fn selected_lines_patch_for_direction(
    file: &FileDiff,
    hunk: &Hunk,
    selected: &[usize],
    reverse: bool,
) -> Result<Vec<u8>> {
    let invalid = || Error::InvalidLineSelection;
    if !supports_selected_lines(file, hunk) || selected.is_empty() {
        return Err(invalid());
    }
    let mut indexes = selected.to_vec();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.len() != selected.len()
        || indexes.iter().any(|&index| {
            !matches!(
                hunk.lines.get(index).map(|line| line.kind),
                Some(DiffLineKind::Addition | DiffLineKind::Deletion)
            )
        })
    {
        return Err(invalid());
    }

    let mut lines = Vec::new();
    let (mut old_count, mut new_count) = (0, 0);
    for (index, line) in hunk.lines.iter().enumerate() {
        let chosen = indexes.binary_search(&index).is_ok();
        let kind = match (line.kind, chosen, reverse) {
            (DiffLineKind::Addition, false, false) | (DiffLineKind::Deletion, false, true) => {
                continue;
            }
            (DiffLineKind::Deletion, false, false) | (DiffLineKind::Addition, false, true) => {
                DiffLineKind::Context
            }
            (kind, _, _) => kind,
        };
        if kind != DiffLineKind::Addition {
            old_count += 1;
        }
        if kind != DiffLineKind::Deletion {
            new_count += 1;
        }
        lines.push((kind, &line.content));
    }

    let mut patch = Vec::new();
    push_line(&mut patch, &file.header);
    for line in &file.metadata {
        push_line(&mut patch, line);
    }
    push_line(
        &mut patch,
        format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, old_count, hunk.new_start, new_count
        )
        .as_bytes(),
    );
    for (kind, content) in lines {
        patch.push(marker(kind));
        patch.extend_from_slice(content);
        patch.push(b'\n');
    }
    Ok(patch)
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
