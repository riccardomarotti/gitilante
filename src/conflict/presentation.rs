//! Conflict classification for the visual solver (§46-55).
//!
//! This module is Git-free: it decides how a conflict must be presented from
//! the parsed status metadata plus the raw contents already loaded. When the
//! kind of conflict is not supported, the presentation declares it instead of
//! guessing.

use crate::conflict::markers::parse;
use crate::conflict::model::ConflictFile;
use crate::model::status::UnmergedInfo;

/// Which source a piece of content comes from (stage 2 = A, stage 3 = B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    /// Stage 2 side (under the `<<<<<<<` marker).
    SourceA,
    /// Stage 3 side (above the `>>>>>>>` marker).
    SourceB,
}

/// A complete file version for whole-file choices (§51-52).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    /// Full file bytes of this version.
    pub bytes: Vec<u8>,
    /// File mode in octal notation, when known.
    pub mode: Option<u32>,
    /// True when the bytes are not safe to edit as text.
    pub binary: bool,
}

/// How a conflicted path must be presented by the solver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictPresentation {
    /// Textual conflict with per-block resolution (§46).
    TextBlocks(ConflictFile),
    /// One side deleted the file: keep it or delete it (§48-49).
    KeepOrDelete {
        /// Content of the surviving version (working tree preferred, §16).
        content: Vec<u8>,
        /// Which source produced the surviving content.
        content_side: ConflictSide,
    },
    /// Both sides deleted the file (§50).
    ResolveAsDeleted,
    /// Whole-file choice: binary or non-mergeable content (§47, §51).
    WholeFileChoice {
        /// Complete stage 2 version.
        version_a: FileVersion,
        /// Complete stage 3 version.
        version_b: FileVersion,
    },
    /// Markers already removed by hand; only staging is needed (§41).
    AlreadyManuallyResolved,
    /// Not supported by the visual solver (§53-54); resolve externally.
    Unsupported(String),
}

/// True when the bytes look binary (a NUL in the first 8000 bytes, like Git).
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

/// True when the mode is not a regular file (symlink, gitlink, ...).
fn is_special_mode(mode: Option<u32>) -> bool {
    matches!(mode, Some(mode) if mode != 0o100644 && mode != 0o100755)
}

/// Classifies a conflicted path from its metadata and contents.
///
/// `stage_a`/`stage_b` are the raw stage 2/3 blobs, present only when the
/// stage exists and has been loaded.
pub fn classify(
    unmerged: &UnmergedInfo,
    working_tree: Option<&[u8]>,
    stage_a: Option<&[u8]>,
    stage_b: Option<&[u8]>,
) -> ConflictPresentation {
    use ConflictPresentation::*;
    use ConflictSide::*;

    if unmerged.submodule != crate::model::status::SubmoduleState::NotSubmodule {
        return Unsupported("submodule conflicts are not supported by the visual solver; resolve them externally and refresh.".to_owned());
    }
    if is_special_mode(unmerged.base.mode)
        || is_special_mode(unmerged.stage2.mode)
        || is_special_mode(unmerged.stage3.mode)
        || is_special_mode(unmerged.worktree_mode)
    {
        return Unsupported("symlink or special file mode conflicts are not supported by the visual solver; resolve them externally and refresh.".to_owned());
    }

    let has_a = unmerged.stage2.oid.is_some();
    let has_b = unmerged.stage3.oid.is_some();
    match (has_a, has_b) {
        // Both sides contribute content: text blocks or whole-file choice.
        (true, true) => {
            let (Some(bytes_a), Some(bytes_b)) = (stage_a, stage_b) else {
                return Unsupported("the conflict stage contents could not be read.".to_owned());
            };
            let Some(working_tree) = working_tree else {
                return Unsupported("the working tree file is missing; resolve the conflict externally and refresh.".to_owned());
            };
            if is_binary(bytes_a) || is_binary(bytes_b) || is_binary(working_tree) {
                return WholeFileChoice {
                    version_a: FileVersion {
                        bytes: bytes_a.to_vec(),
                        mode: unmerged.stage2.mode,
                        binary: true,
                    },
                    version_b: FileVersion {
                        bytes: bytes_b.to_vec(),
                        mode: unmerged.stage3.mode,
                        binary: true,
                    },
                };
            }
            if std::str::from_utf8(working_tree).is_err() {
                // §21: no visual editing that could corrupt the encoding.
                return WholeFileChoice {
                    version_a: FileVersion {
                        bytes: bytes_a.to_vec(),
                        mode: unmerged.stage2.mode,
                        binary: true,
                    },
                    version_b: FileVersion {
                        bytes: bytes_b.to_vec(),
                        mode: unmerged.stage3.mode,
                        binary: true,
                    },
                };
            }
            match parse(working_tree) {
                Ok(file) if file.conflict_count() > 0 => TextBlocks(file),
                Ok(_) => AlreadyManuallyResolved,
                Err(error) => Unsupported(format!(
                    "the file contains malformed conflict markers and cannot be solved visually: {error}"
                )),
            }
        }
        // One side deleted the file (§48-49).
        (true, false) | (false, true) => {
            let content_side = if has_a { SourceA } else { SourceB };
            let content = working_tree
                .map(<[u8]>::to_vec)
                .or_else(|| if has_a { stage_a } else { stage_b }.map(<[u8]>::to_vec))
                .unwrap_or_default();
            KeepOrDelete {
                content,
                content_side,
            }
        }
        // Both sides deleted the file (§50).
        (false, false) => ResolveAsDeleted,
    }
}
