//! Byte-safe parsing of Git conflict markers.
//!
//! The parser recognizes the marker structure and never the meaning of the
//! label text after a marker. All content bytes are preserved verbatim:
//! parsing works on `split_inclusive` lines, so LF/CRLF terminators, trailing
//! whitespace and a missing newline at EOF survive untouched.
//!
//! Only the standard seven-character markers produced by Git are supported.
//!
//! Malformed structures (unterminated blocks, markers in impossible states)
//! fail the parse instead of producing a partial reconstruction.

use crate::conflict::model::{ConflictBlock, ConflictFile, ConflictFilePart};

/// Marker introducing the first alternative.
const MARKER_A: &[u8] = b"<<<<<<<";
/// Marker introducing the common base (diff3/zdiff3).
const MARKER_BASE: &[u8] = b"|||||||";
/// Marker separating the two alternatives.
const MARKER_SPLIT: &[u8] = b"=======";
/// Marker closing the conflict block.
const MARKER_B: &[u8] = b">>>>>>>";

/// Why a file could not be parsed as a conflicted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerParseError {
    /// The marker structure is incomplete or inconsistent.
    Malformed {
        /// 1-based line number where the problem was detected.
        line: usize,
        /// Human-readable detail.
        detail: String,
    },
}

impl std::fmt::Display for MarkerParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed { line, detail } => {
                write!(
                    formatter,
                    "malformed conflict markers at line {line}: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for MarkerParseError {}

/// A conflict block whose closing marker has not been seen yet.
struct OpenBlock {
    id: usize,
    source_a: Vec<u8>,
    base: Option<Vec<u8>>,
    source_b: Vec<u8>,
    marker_a: Option<String>,
    marker_base: Option<String>,
}

/// Parser state: outside or inside a conflict block.
enum State {
    Plain,
    InA(OpenBlock),
    InBase(OpenBlock),
    InB(OpenBlock),
}

/// Returns the marker kind a line opens and its opaque label, if any.
///
/// A marker line is the seven-character run at the start of the line followed
/// by nothing or by a space and the label text.
fn marker_of(line: &[u8]) -> Option<(&'static [u8], Option<String>)> {
    for run in [MARKER_A, MARKER_BASE, MARKER_SPLIT, MARKER_B] {
        if let Some(rest) = line.strip_prefix(run) {
            if rest.is_empty() {
                return Some((run, None));
            }
            if let Some(label) = rest.strip_prefix(b" ") {
                return Some((run, Some(String::from_utf8_lossy(label).into_owned())));
            }
        }
    }
    None
}

/// Line without its end-of-line bytes, used for marker identification only.
fn content_of(raw_line: &[u8]) -> &[u8] {
    let without_lf = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

/// Parses a conflicted working tree file into plain segments and blocks.
///
/// Files without markers parse to a single `Plain` part.
pub fn parse(input: &[u8]) -> Result<ConflictFile, MarkerParseError> {
    let mut parts: Vec<ConflictFilePart> = Vec::new();
    let mut plain: Vec<u8> = Vec::new();
    let mut next_id = 1;
    let mut state = State::Plain;

    for (index, raw_line) in input.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let line_number = index + 1;
        let marker = marker_of(content_of(raw_line));

        state = match (state, marker) {
            // Opens a conflict block.
            (State::Plain, Some((MARKER_A, label))) => {
                if !plain.is_empty() {
                    parts.push(ConflictFilePart::Plain(std::mem::take(&mut plain)));
                }
                let id = next_id;
                next_id += 1;
                State::InA(OpenBlock {
                    id,
                    source_a: Vec::new(),
                    base: None,
                    source_b: Vec::new(),
                    marker_a: label,
                    marker_base: None,
                })
            }
            // Marker-looking lines outside a conflict are ordinary content
            // (e.g. `=======` underlines or deep `>>>>>>>` quotes).
            (State::Plain, _) => {
                plain.extend_from_slice(raw_line);
                State::Plain
            }

            // Content lines go to the side currently being accumulated.
            (State::InA(mut open), None) => {
                open.source_a.extend_from_slice(raw_line);
                State::InA(open)
            }
            (State::InBase(mut open), None) => {
                open.base
                    .get_or_insert_with(Vec::new)
                    .extend_from_slice(raw_line);
                State::InBase(open)
            }
            (State::InB(mut open), None) => {
                open.source_b.extend_from_slice(raw_line);
                State::InB(open)
            }

            // Diff3 base marker starts the base section.
            (State::InA(mut open), Some((MARKER_BASE, label))) => {
                open.base = Some(Vec::new());
                open.marker_base = label;
                State::InBase(open)
            }
            // Split marker ends the first alternative (and the base, if any).
            (State::InA(open), Some((MARKER_SPLIT, _)))
            | (State::InBase(open), Some((MARKER_SPLIT, _))) => State::InB(open),
            // Closing marker ends the block.
            (State::InB(open), Some((MARKER_B, label))) => {
                parts.push(ConflictFilePart::Conflict(ConflictBlock {
                    id: open.id,
                    source_a: open.source_a,
                    base: open.base,
                    source_b: open.source_b,
                    marker_a: open.marker_a,
                    marker_base: open.marker_base,
                    marker_b: label,
                }));
                State::Plain
            }
            // Any other marker is in a state where it makes no sense.
            (_, Some(_)) => {
                return Err(MarkerParseError::Malformed {
                    line: line_number,
                    detail: "unexpected conflict marker".to_owned(),
                });
            }
        };
    }

    if !matches!(state, State::Plain) {
        return Err(MarkerParseError::Malformed {
            line: input.split(|byte| *byte == b'\n').count(),
            detail: "unterminated conflict block".to_owned(),
        });
    }
    if !plain.is_empty() {
        parts.push(ConflictFilePart::Plain(plain));
    }
    Ok(ConflictFile::from_parts(parts))
}
