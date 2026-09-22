//! `git status` execution and parsing of its machine-readable output.
//!
//! The parser consumes `--porcelain=v2 -z` output: NUL-delimited records with
//! unescaped pathnames, so paths containing spaces, tabs, newlines or Unicode
//! survive untouched.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::git::command;
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::model::status::{BranchInfo, ChangeKind, Head, Status, StatusEntry, StatusEntryKind};

/// Result of the output parsers, failing with a human-readable detail.
type ParseResult<T> = std::result::Result<T, String>;

/// Reads the repository status with `git status --porcelain=v2 -z --branch`.
pub fn status(repo: &Repository) -> crate::git::Result<Status> {
    let args = [
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        // Explicit, stable behaviour regardless of user configuration.
        "--untracked-files=normal",
    ];
    let output = command::run(repo.root(), &args)?;
    parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
        command: "status".to_owned(),
        detail,
    })
}

/// Parses NUL-delimited `git status --porcelain=v2` output.
pub fn parse(input: &[u8]) -> ParseResult<Status> {
    // Renamed/copied records carry the original pathname as an extra NUL
    // terminated field, so an empty-filtered token list is walked with an index.
    let records: Vec<&[u8]> = input
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();

    let mut status = Status::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        match record[0] {
            b'#' => parse_header(&mut status.branch, record)?,
            b'1' => status.entries.push(parse_ordinary(record)?),
            b'2' => {
                let orig_path = records
                    .get(index)
                    .ok_or_else(|| "renamed entry without original pathname".to_owned())?;
                index += 1;
                status.entries.push(parse_renamed(record, orig_path)?);
            }
            b'u' => status.entries.push(parse_unmerged(record)?),
            b'?' => status
                .entries
                .push(parse_flagged(record, StatusEntryKind::Untracked)?),
            b'!' => status
                .entries
                .push(parse_flagged(record, StatusEntryKind::Ignored)?),
            other => return Err(format!("unknown status record type: {:?}", other as char)),
        }
    }
    Ok(status)
}

/// Parses a `# <key> <value>` header record. Unknown keys are ignored for
/// forward compatibility.
fn parse_header(branch: &mut BranchInfo, record: &[u8]) -> ParseResult<()> {
    let body = record
        .strip_prefix(b"# ")
        .ok_or_else(|| "malformed header record".to_owned())?;
    let (key, value) = split_first_space(body)?;
    match key {
        b"branch.oid" => {
            branch.oid = if value == b"(initial)" {
                None
            } else {
                Some(utf8(value)?)
            }
        }
        b"branch.head" => {
            branch.head = match value {
                b"(detached)" => Head::Detached,
                b"(unknown)" => Head::Unknown,
                name => Head::Branch(utf8(name)?),
            }
        }
        b"branch.upstream" => branch.upstream = Some(utf8(value)?),
        b"branch.ab" => {
            let (ahead, behind) = parse_ahead_behind(value)?;
            branch.ahead = Some(ahead);
            branch.behind = Some(behind);
        }
        _ => {}
    }
    Ok(())
}

/// Parses `+<ahead> -<behind>`.
fn parse_ahead_behind(value: &[u8]) -> ParseResult<(u64, u64)> {
    let fields = value.split(|byte| *byte == b' ').collect::<Vec<_>>();
    if fields.len() != 2 {
        return Err("malformed branch.ab header".to_owned());
    }
    let ahead = fields[0]
        .strip_prefix(b"+")
        .ok_or_else(|| "malformed branch.ab header".to_owned())?;
    let behind = fields[1]
        .strip_prefix(b"-")
        .ok_or_else(|| "malformed branch.ab header".to_owned())?;
    Ok((
        utf8(ahead)?
            .parse()
            .map_err(|_| "malformed branch.ab header".to_owned())?,
        utf8(behind)?
            .parse()
            .map_err(|_| "malformed branch.ab header".to_owned())?,
    ))
}

/// Parses an ordinary entry: `1 XY sub mH mI mW hH hI <path>`.
fn parse_ordinary(record: &[u8]) -> ParseResult<StatusEntry> {
    let fields = split_fields(record, 9)?;
    Ok(StatusEntry {
        path: path(fields[8]),
        orig_path: None,
        kind: StatusEntryKind::Tracked {
            index: ChangeKind::from_byte(xy_byte(fields[1], 0)?)?,
            worktree: ChangeKind::from_byte(xy_byte(fields[1], 1)?)?,
        },
    })
}

/// Parses a renamed/copied entry: `2 XY sub mH mI mW hH hI Xscore <path>`,
/// where the record is followed by a separate field holding the original path.
fn parse_renamed(record: &[u8], orig_path: &[u8]) -> ParseResult<StatusEntry> {
    let fields = split_fields(record, 10)?;
    Ok(StatusEntry {
        path: path(fields[9]),
        orig_path: Some(path(orig_path)),
        kind: StatusEntryKind::Tracked {
            index: ChangeKind::from_byte(xy_byte(fields[1], 0)?)?,
            worktree: ChangeKind::from_byte(xy_byte(fields[1], 1)?)?,
        },
    })
}

/// Parses an unmerged entry: `u XY sub m1 m2 m3 mW h1 h2 h3 <path>`.
fn parse_unmerged(record: &[u8]) -> ParseResult<StatusEntry> {
    let fields = split_fields(record, 11)?;
    Ok(StatusEntry {
        path: path(fields[10]),
        orig_path: None,
        kind: StatusEntryKind::Unmerged,
    })
}

/// Parses a flagged entry: `? <path>` (untracked) or `! <path>` (ignored).
fn parse_flagged(record: &[u8], kind: StatusEntryKind) -> ParseResult<StatusEntry> {
    let raw_path = record
        .strip_prefix(b"? ")
        .or_else(|| record.strip_prefix(b"! "))
        .ok_or_else(|| "malformed flagged record".to_owned())?;
    Ok(StatusEntry {
        path: path(raw_path),
        orig_path: None,
        kind,
    })
}

/// Splits a record into exactly `count` space-separated fields; the last field
/// is the pathname and may itself contain spaces.
fn split_fields(record: &[u8], count: usize) -> ParseResult<Vec<&[u8]>> {
    let fields = record
        .splitn(count, |byte| *byte == b' ')
        .collect::<Vec<_>>();
    if fields.len() != count {
        return Err(format!(
            "malformed record with {} fields: {:?}",
            fields.len(),
            String::from_utf8_lossy(record)
        ));
    }
    Ok(fields)
}

/// Returns the `X` (index 0) or `Y` (worktree 1) byte of a two-character XY field.
fn xy_byte(xy: &[u8], position: usize) -> ParseResult<u8> {
    xy.get(position)
        .copied()
        .ok_or_else(|| format!("malformed XY field: {:?}", String::from_utf8_lossy(xy)))
}

/// Splits `body` at the first space into a header key and value.
fn split_first_space(body: &[u8]) -> ParseResult<(&[u8], &[u8])> {
    let separator = body
        .iter()
        .position(|byte| *byte == b' ')
        .ok_or_else(|| "malformed header record".to_owned())?;
    Ok((&body[..separator], &body[separator + 1..]))
}

/// Converts a pathname field to a `PathBuf` without any encoding assumption.
fn path(raw: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(raw.to_vec()))
}

/// Converts a branch or commit name to a `String` for display.
fn utf8(bytes: &[u8]) -> ParseResult<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| format!("expected UTF-8 text: {:?}", String::from_utf8_lossy(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Joins record fragments with NUL terminators, like `git status -z`.
    fn records(parts: &[&[u8]]) -> Vec<u8> {
        let mut output = Vec::new();
        for part in parts {
            output.extend_from_slice(part);
            output.push(0);
        }
        output
    }

    #[test]
    fn parses_branch_headers() {
        let input = records(&[
            b"# branch.oid 3421e7af013d6385bd60bada8908aed0b838cecc",
            b"# branch.head main",
            b"# branch.upstream origin/main",
            b"# branch.ab +1 -2",
        ]);
        let status = parse(&input).unwrap();
        assert_eq!(
            status.branch.oid.as_deref(),
            Some("3421e7af013d6385bd60bada8908aed0b838cecc")
        );
        assert_eq!(status.branch.head, Head::Branch("main".to_owned()));
        assert_eq!(status.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.branch.ahead, Some(1));
        assert_eq!(status.branch.behind, Some(2));
        assert!(status.entries.is_empty());
    }

    #[test]
    fn parses_unborn_branch() {
        let status = parse(&records(&[
            b"# branch.oid (initial)",
            b"# branch.head main",
        ]))
        .unwrap();
        assert_eq!(status.branch.oid, None);
        assert_eq!(status.branch.head, Head::Branch("main".to_owned()));
    }

    #[test]
    fn parses_detached_head() {
        let status = parse(&records(&[
            b"# branch.oid 3421e7af",
            b"# branch.head (detached)",
        ]))
        .unwrap();
        assert_eq!(status.branch.head, Head::Detached);
    }

    #[test]
    fn parses_ordinary_entry() {
        let status = parse(&records(&[
            b"1 M. N... 100644 100644 100644 587be6b4c3 b77b4eb1d9 src/file.rs",
        ]))
        .unwrap();
        assert_eq!(status.entries.len(), 1);
        let entry = &status.entries[0];
        assert_eq!(entry.path, PathBuf::from("src/file.rs"));
        assert_eq!(entry.orig_path, None);
        assert_eq!(entry.staged_change(), Some(ChangeKind::Modified));
        assert_eq!(entry.unstaged_change(), None);
    }

    #[test]
    fn renamed_entry_consumes_original_path_field() {
        let input = records(&[
            b"2 R. N... 100644 100644 100644 7898192261 7898192261 R100 new name.txt",
            b"old name.txt",
            b"1 .M N... 100644 100644 100644 6178079822 6178079822 other.txt",
        ]);
        let status = parse(&input).unwrap();
        assert_eq!(status.entries.len(), 2);
        let renamed = &status.entries[0];
        assert_eq!(renamed.path, PathBuf::from("new name.txt"));
        assert_eq!(renamed.orig_path, Some(PathBuf::from("old name.txt")));
        assert_eq!(renamed.staged_change(), Some(ChangeKind::Renamed));
        let next = &status.entries[1];
        assert_eq!(next.path, PathBuf::from("other.txt"));
        assert_eq!(next.unstaged_change(), Some(ChangeKind::Modified));
    }

    #[test]
    fn parses_untracked_and_ignored_entries() {
        let status = parse(&records(&[b"? new file.rs", b"! ignored.txt"])).unwrap();
        assert!(status.entries[0].is_untracked());
        assert!(status.entries[1].is_ignored());
    }

    #[test]
    fn parses_unmerged_entry() {
        let status = parse(&records(&[
            b"u UU N... 100644 100644 100644 100644 587be6b4c3 b77b4eb1d9 3f2c8a1d4e conflict.txt",
        ]))
        .unwrap();
        assert!(status.entries[0].is_unmerged());
    }

    #[test]
    fn pathnames_are_never_decoded() {
        let status = parse(&records(&[b"? weird\n\tname \xe2\x98\x83.txt"])).unwrap();
        assert_eq!(
            status.entries[0].path,
            PathBuf::from(OsString::from_vec(
                b"weird\n\tname \xe2\x98\x83.txt".to_vec()
            ))
        );
    }

    #[test]
    fn malformed_records_report_an_error() {
        assert!(parse(&records(&[b"1 M."])).is_err());
        assert!(
            parse(&records(&[
                b"2 R. N... 100644 100644 100644 7898192261 7898192261 R100 new.txt"
            ]))
            .is_err()
        );
        assert!(parse(&records(&[b"3 nonsense"])).is_err());
        assert!(parse(&records(&[b"# branch.ab 1 2"])).is_err());
    }

    #[test]
    fn empty_input_is_a_clean_repository() {
        let status = parse(&[]).unwrap();
        assert!(status.entries.is_empty());
    }
}
