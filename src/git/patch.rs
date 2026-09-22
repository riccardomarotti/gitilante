//! Patch reconstruction.
//!
//! Turns parts of a parsed diff back into unified diff text that `git apply`
//! accepts. Used to stage, unstage, discard and revert single hunks.

use crate::model::diff::{DiffLineKind, FileDiff, Hunk};

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
