//! `git diff` execution and internal unified diff parsing.
//!
//! Every invocation pins the output format (`--no-color`, `--no-ext-diff`,
//! `--no-textconv`, explicit path prefixes, forced path quoting) so that user
//! configuration cannot change the shape of the data being parsed.

use crate::git::Result;
use crate::git::command;
use crate::git::repository::Repository;
use crate::model::diff::{Diff, DiffLine, DiffLineKind, FileDiff, FileStatus, Hunk};
use std::path::PathBuf;

/// Arguments shared by every machine-readable diff: neutralize colors,
/// external diff drivers and textconv, pin the `a/`-`b/` path prefixes and
/// force C-style path quoting so pathnames stay parseable line by line.
const STABLE_DIFF_ARGS: &[&str] = &[
    "-c",
    "core.quotePath=true",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--submodule=short",
];

/// Result of the output parsers, failing with a human-readable detail.
type ParseResult<T> = std::result::Result<T, String>;

/// Diff of the working tree against the index (unstaged changes).
pub fn working_tree_diff(repo: &Repository) -> Result<Diff> {
    let output = command::run(repo.root(), STABLE_DIFF_ARGS)?;
    parse_or_error(&output.stdout)
}

/// Diff of the index against HEAD (staged changes).
pub fn staged_diff(repo: &Repository) -> Result<Diff> {
    let mut args = STABLE_DIFF_ARGS.to_vec();
    args.push("--cached");
    let output = command::run(repo.root(), &args)?;
    parse_or_error(&output.stdout)
}

fn parse_or_error(stdout: &[u8]) -> Result<Diff> {
    parse(stdout).map_err(|detail| crate::git::error::Error::MalformedOutput {
        command: "diff".to_owned(),
        detail,
    })
}

/// Parses unified diff output as produced by `git diff` / `git show`.
pub fn parse(input: &[u8]) -> ParseResult<Diff> {
    let mut lines = input.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    // A trailing newline produces an empty final piece that is not a line.
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    let mut diff = Diff::default();
    let mut file: Option<FileDiff> = None;
    let mut flags = FileFlags::default();
    let mut in_hunk = false;
    let mut in_binary_patch = false;

    for line in lines {
        if line.starts_with(b"diff --git ") {
            if let Some(previous) = file.take() {
                diff.files.push(previous.finish(&mut flags));
            }
            flags = FileFlags::default();
            in_hunk = false;
            in_binary_patch = false;
            file = Some(FileDiff {
                header: line.to_vec(),
                metadata: Vec::new(),
                old_path: None,
                new_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: Vec::new(),
            });
        } else if line.starts_with(b"@@ ") {
            let current = file.as_mut().ok_or_else(|| {
                format!(
                    "hunk header outside of any file: {:?}",
                    String::from_utf8_lossy(line)
                )
            })?;
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
            current.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                header: line.to_vec(),
                lines: Vec::new(),
            });
            in_hunk = true;
        } else if in_binary_patch {
            // A `GIT binary patch` block (only produced with --binary) is kept
            // verbatim so the file diff stays faithful to the Git output.
            file.as_mut()
                .expect("binary patch inside a file")
                .metadata
                .push(line.to_vec());
        } else if in_hunk {
            let current = file.as_mut().expect("hunk inside a file");
            current
                .hunks
                .last_mut()
                .expect("hunk was started")
                .lines
                .push(parse_hunk_line(line)?);
        } else if let Some(current) = file.as_mut() {
            parse_metadata(current, &mut flags, line)?;
            in_binary_patch = flags.binary_patch;
        } else if !line.is_empty() {
            return Err(format!(
                "line outside of any file diff: {:?}",
                String::from_utf8_lossy(line)
            ));
        }
    }

    if let Some(previous) = file.take() {
        diff.files.push(previous.finish(&mut flags));
    }
    Ok(diff)
}

/// Extended header state accumulated while parsing a file diff.
#[derive(Debug, Default)]
struct FileFlags {
    added: bool,
    deleted: bool,
    mode_changed: bool,
    rename_from: Option<PathBuf>,
    rename_to: Option<PathBuf>,
    copied: bool,
    binary: bool,
    binary_patch: bool,
}

impl FileDiff {
    /// Completes the file diff, deriving paths and status from the extended
    /// headers collected so far.
    fn finish(mut self, flags: &mut FileFlags) -> Self {
        let was_renamed = flags.rename_from.is_some() || flags.rename_to.is_some();
        let was_copied = flags.copied;
        // Rename headers are authoritative; fall back to `---`/`+++`, then to
        // the `diff --git` line (binary and mode-only diffs have no `---`/`+++`).
        if self.old_path.is_none() || self.new_path.is_none() {
            let (git_old, git_new) = parse_diff_git_header(&self.header);
            self.old_path = self
                .old_path
                .take()
                .or_else(|| flags.rename_from.take())
                .or(git_old);
            self.new_path = self
                .new_path
                .take()
                .or_else(|| flags.rename_to.take())
                .or(git_new);
        }
        if flags.added {
            self.old_path = None;
        }
        if flags.deleted {
            self.new_path = None;
        }

        self.binary = flags.binary || flags.binary_patch;
        self.status = if was_renamed {
            FileStatus::Renamed
        } else if was_copied {
            FileStatus::Copied
        } else if flags.added {
            FileStatus::Added
        } else if flags.deleted {
            FileStatus::Deleted
        } else if flags.mode_changed && self.hunks.is_empty() {
            FileStatus::ModeChanged
        } else {
            FileStatus::Modified
        };
        self
    }
}

/// Classifies one extended header line and records what it implies.
fn parse_metadata(file: &mut FileDiff, flags: &mut FileFlags, line: &[u8]) -> ParseResult<()> {
    file.metadata.push(line.to_vec());

    if let Some(value) = field(line, b"rename from ") {
        flags.rename_from = Some(path_from_bytes(parse_path_value(value)?));
    } else if let Some(value) = field(line, b"rename to ") {
        flags.rename_to = Some(path_from_bytes(parse_path_value(value)?));
    } else if field(line, b"copy from ").is_some() || field(line, b"copy to ").is_some() {
        flags.copied = true;
    } else if field(line, b"new file mode ").is_some() {
        flags.added = true;
    } else if field(line, b"deleted file mode ").is_some() {
        flags.deleted = true;
    } else if field(line, b"old mode ").is_some() || field(line, b"new mode ").is_some() {
        flags.mode_changed = true;
    } else if field(line, b"--- ").is_some() {
        file.old_path = parse_path_field(line.strip_prefix(b"--- ").expect("checked above"))?;
    } else if field(line, b"+++ ").is_some() {
        file.new_path = parse_path_field(line.strip_prefix(b"+++ ").expect("checked above"))?;
    } else if line.starts_with(b"Binary files ") && line.ends_with(b" differ") {
        flags.binary = true;
    } else if line == b"GIT binary patch" {
        flags.binary_patch = true;
    }
    // Unrecognized lines stay in `metadata` untouched (forward compatibility).
    Ok(())
}

/// Parses one hunk line: marker byte plus raw content.
fn parse_hunk_line(line: &[u8]) -> ParseResult<DiffLine> {
    let (kind, content) = match line.first() {
        Some(b' ') => (DiffLineKind::Context, &line[1..]),
        Some(b'+') => (DiffLineKind::Addition, &line[1..]),
        Some(b'-') => (DiffLineKind::Deletion, &line[1..]),
        Some(b'\\') => (DiffLineKind::NoNewlineMarker, &line[1..]),
        // Tolerate empty context lines without their marker (some tools emit
        // them); Git itself always writes the leading space.
        None => (DiffLineKind::Context, line),
        Some(other) => {
            return Err(format!(
                "unexpected line in hunk: {:?}",
                String::from_utf8_lossy(line)
                    .chars()
                    .next()
                    .unwrap_or(*other as char)
            ));
        }
    };
    Ok(DiffLine {
        kind,
        content: content.to_vec(),
    })
}

/// Parses `@@ -old[,count] +new[,count] @@ ...`; omitted counts mean 1.
fn parse_hunk_header(line: &[u8]) -> ParseResult<(usize, usize, usize, usize)> {
    let spec = line
        .strip_prefix(b"@@ ")
        .ok_or_else(|| format!("malformed hunk header: {:?}", String::from_utf8_lossy(line)))?;
    let (old_start, old_count, spec) = parse_range(spec, b'-')?;
    let spec = spec
        .strip_prefix(b" ")
        .ok_or_else(|| format!("malformed hunk header: {:?}", String::from_utf8_lossy(line)))?;
    let (new_start, new_count, _) = parse_range(spec, b'+')?;
    Ok((old_start, old_count, new_start, new_count))
}

/// Parses `[-|+]start[,count]`, returning the range and the remaining input.
fn parse_range(spec: &[u8], sign: u8) -> ParseResult<(usize, usize, &[u8])> {
    let detail = || format!("malformed hunk range: {:?}", String::from_utf8_lossy(spec));
    let spec = spec.strip_prefix(&[sign]).ok_or_else(detail)?;
    let (start, rest) = parse_number(spec).ok_or_else(detail)?;
    if let Some(rest) = rest.strip_prefix(b",") {
        let (count, rest) = parse_number(rest).ok_or_else(detail)?;
        Ok((start, count, rest))
    } else {
        Ok((start, 1, rest))
    }
}

/// Parses the leading decimal number of `spec`.
fn parse_number(spec: &[u8]) -> Option<(usize, &[u8])> {
    let digits = spec.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let value = std::str::from_utf8(&spec[..digits]).ok()?.parse().ok()?;
    Some((value, &spec[digits..]))
}

/// Parses the pathname of a `--- `/`+++ ` field, stripping its `a/`/`b/` side
/// prefix; `/dev/null` becomes `None`.
fn parse_path_field(field: &[u8]) -> ParseResult<Option<PathBuf>> {
    // Git appends a disambiguation tab when the name contains spaces.
    let field = field.strip_suffix(b"\t").unwrap_or(field);
    if field == b"/dev/null" {
        return Ok(None);
    }
    Ok(Some(path_from_bytes(
        strip_side_prefix(&parse_path_value(field)?).to_vec(),
    )))
}

/// Parses a quoted or plain pathname value from an extended header into its
/// raw bytes; `rename from/to` values have no side prefix to strip.
fn parse_path_value(value: &[u8]) -> ParseResult<Vec<u8>> {
    if value.starts_with(b"\"") {
        let (bytes, _) = unquote_c_style(value)?;
        return Ok(bytes);
    }
    Ok(value.to_vec())
}

/// Converts raw pathname bytes to a `PathBuf` without decoding them.
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(bytes))
}

/// Extracts the old and new paths from a `diff --git a/... b/...` line.
///
/// Only needed as a fallback (binary and mode-only diffs have no `---`/`+++`);
/// unquoted names containing spaces make the line ambiguous, so a symmetric
/// split (both sides equal) is preferred, then the first ` b/` split.
fn parse_diff_git_header(line: &[u8]) -> (Option<PathBuf>, Option<PathBuf>) {
    let Some(spec) = line.strip_prefix(b"diff --git ") else {
        return (None, None);
    };

    if spec.starts_with(b"\"") {
        let Ok((old, rest)) = unquote_c_style(spec) else {
            return (None, None);
        };
        let Some(rest) = rest.strip_prefix(b" ") else {
            return (None, None);
        };
        let Ok((new, tail)) = unquote_c_style(rest) else {
            return (None, None);
        };
        if !tail.is_empty() {
            return (None, None);
        }
        return (
            Some(path_from_bytes(strip_side_prefix(&old).to_vec())),
            Some(path_from_bytes(strip_side_prefix(&new).to_vec())),
        );
    }

    let mut fallback = None;
    for separator in find_all(spec, b" b/") {
        let (left, right) = (&spec[..separator], &spec[separator + 1..]);
        let (Some(old), Some(new)) = (left.strip_prefix(b"a/"), right.strip_prefix(b"b/")) else {
            continue;
        };
        if old == new {
            return (
                Some(path_from_bytes(old.to_vec())),
                Some(path_from_bytes(new.to_vec())),
            );
        }
        fallback.get_or_insert((old.to_vec(), new.to_vec()));
    }

    match fallback {
        Some((old, new)) => (Some(path_from_bytes(old)), Some(path_from_bytes(new))),
        None => (None, None),
    }
}

/// Finds every occurrence of `needle` in `haystack`.
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect()
}

/// Strips the `a/` or `b/` side prefix from pathname bytes.
fn strip_side_prefix(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_prefix(b"a/".as_slice())
        .or_else(|| bytes.strip_prefix(b"b/".as_slice()))
        .unwrap_or(bytes)
}

/// Unquotes a C-style quoted pathname (`"a/caf\303\251\n.txt"`).
fn unquote_c_style(value: &[u8]) -> ParseResult<(Vec<u8>, &[u8])> {
    let mut bytes = Vec::new();
    let mut rest = value
        .strip_prefix(b"\"")
        .ok_or_else(|| "quoted pathname without opening quote".to_owned())?;

    loop {
        let Some((byte, tail)) = rest.split_first() else {
            return Err("unterminated quoted pathname".to_owned());
        };
        match byte {
            b'"' => return Ok((bytes, tail)),
            b'\\' => {
                let Some((escape, tail)) = tail.split_first() else {
                    return Err("trailing backslash in quoted pathname".to_owned());
                };
                match escape {
                    b'a' => bytes.push(0x07),
                    b'b' => bytes.push(0x08),
                    b'f' => bytes.push(0x0c),
                    b'n' => bytes.push(b'\n'),
                    b'r' => bytes.push(0x0d),
                    b't' => bytes.push(b'\t'),
                    b'v' => bytes.push(0x0b),
                    b'\\' | b'"' => bytes.push(*escape),
                    b'0'..=b'7' => {
                        let mut value = u16::from(*escape - b'0');
                        let mut tail = tail;
                        for _ in 0..2 {
                            let Some((digit, rest)) = tail.split_first() else {
                                break;
                            };
                            let Some(digit) =
                                (b'0'..=b'7').contains(digit).then_some(*digit - b'0')
                            else {
                                break;
                            };
                            value = value * 8 + u16::from(digit);
                            tail = rest;
                        }
                        if value > 0xff {
                            return Err("octal escape out of range in quoted pathname".to_owned());
                        }
                        bytes.push(value as u8);
                        rest = tail;
                        continue;
                    }
                    other => {
                        return Err(format!(
                            "unknown escape in quoted pathname: {:?}",
                            *other as char
                        ));
                    }
                }
                rest = tail;
            }
            byte => {
                bytes.push(*byte);
                rest = tail;
            }
        }
    }
}

/// Returns `line`'s content after `prefix`, if present.
fn field<'a>(line: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    line.strip_prefix(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE_HUNK: &[u8] = b"diff --git a/f.txt b/f.txt\nindex 71d74e4..0a5e6d0 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,5 @@\n a\n \n-b\n+B\n c\n";
    const MULTI_HUNK: &[u8] = b"diff --git a/multi.txt b/multi.txt\nindex b03757e..00d2adf 100644\n--- a/multi.txt\n+++ b/multi.txt\n@@ -5,7 +5,7 @@ d\n e\n f\n g\n-h\n+CHANGED\n i\n j\n k\n@@ -13,4 +13,4 @@ l\n m\n n\n o\n-p\n+P\n";

    #[test]
    fn parses_single_hunk() {
        let diff = parse(SINGLE_HUNK).unwrap();
        assert_eq!(diff.files.len(), 1);
        let file = &diff.files[0];
        assert_eq!(file.old_path, Some(PathBuf::from("f.txt")));
        assert_eq!(file.new_path, Some(PathBuf::from("f.txt")));
        assert_eq!(file.status, FileStatus::Modified);
        assert!(!file.binary);
        assert_eq!(file.hunks.len(), 1);
        let hunk = &file.hunks[0];
        assert_eq!((hunk.old_start, hunk.old_count), (1, 5));
        assert_eq!((hunk.new_start, hunk.new_count), (1, 5));
        assert_eq!(hunk.header, b"@@ -1,5 +1,5 @@");
        assert_eq!(hunk.lines.len(), 5);
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[2].kind, DiffLineKind::Deletion);
        assert_eq!(hunk.lines[3].kind, DiffLineKind::Addition);
        assert_eq!(hunk.lines[3].content, b"B");
    }

    #[test]
    fn parses_multiple_hunks_and_keeps_the_section_heading() {
        let diff = parse(MULTI_HUNK).unwrap();
        let file = &diff.files[0];
        assert_eq!(file.hunks.len(), 2);
        assert_eq!((file.hunks[0].old_start, file.hunks[0].new_start), (5, 5));
        assert_eq!((file.hunks[1].old_start, file.hunks[1].new_start), (13, 13));
        assert_eq!(file.hunks[1].header, b"@@ -13,4 +13,4 @@ l");
    }

    #[test]
    fn parses_added_and_deleted_files() {
        let added = parse(b"diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..7898192\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n").unwrap();
        let file = &added.files[0];
        assert_eq!(file.status, FileStatus::Added);
        assert_eq!(file.old_path, None);
        assert_eq!(file.new_path, Some(PathBuf::from("new.txt")));
        assert_eq!((file.hunks[0].old_start, file.hunks[0].old_count), (0, 0));

        let deleted = parse(b"diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 7898192..0000000\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n").unwrap();
        let file = &deleted.files[0];
        assert_eq!(file.status, FileStatus::Deleted);
        assert_eq!(file.old_path, Some(PathBuf::from("gone.txt")));
        assert_eq!(file.new_path, None);
    }

    #[test]
    fn parses_pure_rename_without_hunks() {
        let diff = parse(b"diff --git a/old name.txt b/new name.txt\nsimilarity index 100%\nrename from old name.txt\nrename to new name.txt\n").unwrap();
        let file = &diff.files[0];
        assert_eq!(file.status, FileStatus::Renamed);
        assert_eq!(file.old_path, Some(PathBuf::from("old name.txt")));
        assert_eq!(file.new_path, Some(PathBuf::from("new name.txt")));
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn parses_no_newline_marker() {
        let diff = parse(b"diff --git a/f.txt b/f.txt\nindex 7898192..7061c57 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-one\n\\ No newline at end of file\n+two\n\\ No newline at end of file\n").unwrap();
        let hunk = &diff.files[0].hunks[0];
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[1].kind, DiffLineKind::NoNewlineMarker);
        assert_eq!(hunk.lines[3].kind, DiffLineKind::NoNewlineMarker);
    }

    #[test]
    fn parses_mode_change_without_hunks() {
        let diff = parse(b"diff --git a/script.sh b/script.sh\nold mode 100644\nnew mode 100755\n")
            .unwrap();
        let file = &diff.files[0];
        assert_eq!(file.status, FileStatus::ModeChanged);
        assert_eq!(file.old_path, Some(PathBuf::from("script.sh")));
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn parses_binary_diff_and_its_paths() {
        let diff = parse(b"diff --git a/binary file with spaces.bin b/binary file with spaces.bin\nnew file mode 100644\nindex 0000000..8352675\nBinary files /dev/null and b/binary file with spaces.bin differ\n").unwrap();
        let file = &diff.files[0];
        assert!(file.binary);
        assert_eq!(file.status, FileStatus::Added);
        assert_eq!(
            file.new_path,
            Some(PathBuf::from("binary file with spaces.bin"))
        );
    }

    #[test]
    fn strips_the_disambiguation_tab_and_unquotes_paths() {
        let diff = parse(b"diff --git \"a/caf\\303\\251 \\342\\230\\203.txt\" \"b/caf\\303\\251 \\342\\230\\203.txt\"\nindex 7898192..6178079 100644\n--- \"a/caf\\303\\251 \\342\\230\\203.txt\"\t\n+++ \"b/caf\\303\\251 \\342\\230\\203.txt\"\t\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(diff.files[0].new_path, Some(PathBuf::from("café ☃.txt")));

        let diff = parse(b"diff --git a/my file.txt b/my file.txt\nindex 7898192..6178079 100644\n--- a/my file.txt\t\n+++ b/my file.txt\t\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(diff.files[0].old_path, Some(PathBuf::from("my file.txt")));
    }

    #[test]
    fn hunk_header_omitted_counts_mean_one() {
        let diff =
            parse(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3 +3 @@ fn f() {\n-x\n+y\n").unwrap();
        let hunk = &diff.files[0].hunks[0];
        assert_eq!((hunk.old_start, hunk.old_count), (3, 1));
        assert_eq!((hunk.new_start, hunk.new_count), (3, 1));
    }

    #[test]
    fn deletion_lines_look_like_headers_are_still_hunk_lines() {
        // `-` + `- x` produces a line that starts with `--- `.
        let diff =
            parse(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n--- x\n+-- y\n").unwrap();
        let hunk = &diff.files[0].hunks[0];
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Deletion);
        assert_eq!(hunk.lines[0].content, b"-- x");
        assert_eq!(hunk.lines[1].kind, DiffLineKind::Addition);
    }

    #[test]
    fn empty_context_lines_without_marker_are_tolerated() {
        let diff =
            parse(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n\n-b\n").unwrap();
        let hunk = &diff.files[0].hunks[0];
        assert_eq!(hunk.lines[1].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[1].content, b"");
    }

    #[test]
    fn empty_input_is_an_empty_diff() {
        assert!(parse(b"").unwrap().is_empty());
    }

    #[test]
    fn malformed_input_reports_an_error() {
        assert!(parse(b"garbage\n").is_err());
        assert!(parse(b"diff --git a/f b/f\n@@ bogus\n").is_err());
        assert!(
            parse(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\ngarbage in hunk\n").is_err()
        );
    }

    #[test]
    fn unquote_handles_escapes_and_octal() {
        let (bytes, rest) = unquote_c_style(br#""a/caf\303\251 \"x\"\n.txt""#).unwrap();
        assert_eq!(bytes, "a/café \"x\"\n.txt".as_bytes());
        assert!(rest.is_empty());
    }
}
