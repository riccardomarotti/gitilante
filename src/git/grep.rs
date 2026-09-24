//! `git grep` for the Contents search scope
//! (GITILANTE_SEARCH_SPEC.md sections 18-19).
//!
//! Machine-readable output: `-z` NUL-terminates the filename and the two
//! numeric fields, so colons in paths and text never confuse the parser. A
//! match line is `path\0line\0column\0text\n`: the following record's path
//! trails the newline, and stays verbatim even when it contains newlines.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::git::repository::Repository;

/// One matching line of a tracked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// 1-based line number.
    pub line_number: usize,
    /// 1-based column of the first match, when known.
    pub column: Option<usize>,
    /// The full text of the matched line.
    pub text: String,
}

/// Searches the tracked files of the working tree with `git grep -F`.
///
/// Fixed-string matching only (GITILANTE_SEARCH_SPEC.md sections 18 and 49).
/// Exit code 1 means "no matches": a result, not an error.
pub fn grep(repo: &Repository, query: &str, case_sensitive: bool) -> Result<Vec<GrepMatch>> {
    let mut args = vec![
        "grep".to_owned(),
        "-z".to_owned(),
        "-n".to_owned(),
        "--column".to_owned(),
        "-I".to_owned(),
        "-F".to_owned(),
    ];
    if !case_sensitive {
        args.push("-i".to_owned());
    }
    args.push("-e".to_owned());
    args.push(query.to_owned());

    let output = match command::run(repo.root(), &args) {
        Ok(output) => output,
        Err(Error::Git(error)) if error.exit_code == Some(1) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
        command: "grep".to_owned(),
        detail,
    })
}

/// Parses `git grep -z -n --column` output.
pub fn parse(input: &[u8]) -> std::result::Result<Vec<GrepMatch>, String> {
    let mut matches = Vec::new();
    let mut pieces = input.split(|byte| *byte == 0);
    let mut pending_path: Option<PathBuf> = None;

    loop {
        let path = match pending_path.take() {
            Some(path) => path,
            None => match pieces.next() {
                Some(piece) if !piece.is_empty() => {
                    PathBuf::from(OsString::from_vec(piece.to_vec()))
                }
                Some(_) => continue,
                None => break,
            },
        };
        let Some(line) = pieces.next() else {
            break;
        };
        let Some(column) = pieces.next() else {
            return Err(format!("missing fields after {:?}", path.display()));
        };
        let Some(tail) = pieces.next() else {
            return Err(format!("missing text after {:?}", path.display()));
        };

        // `tail` is `text\n<next path>`: the text is one line, everything
        // after the first newline is the next record's path, verbatim.
        let (text, next) = match tail.iter().position(|byte| *byte == b'\n') {
            Some(position) => (
                &tail[..position],
                Some(PathBuf::from(OsString::from_vec(
                    tail[position + 1..].to_vec(),
                ))),
            ),
            None => (tail, None),
        };
        if let Some(next) = next {
            if next.as_os_str().is_empty() {
                // Trailing record.
            } else {
                pending_path = Some(next);
            }
        }

        let line_number = number(line)?;
        let column = number(column)?;
        matches.push(GrepMatch {
            path,
            line_number,
            column: Some(column),
            text: String::from_utf8_lossy(text).into_owned(),
        });
    }
    Ok(matches)
}

/// Decodes a decimal field of a grep record.
fn number(bytes: &[u8]) -> std::result::Result<usize, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        format!(
            "expected UTF-8 number, got {:?}",
            String::from_utf8_lossy(bytes)
        )
    })?;
    text.parse()
        .map_err(|_| format!("malformed number: {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_match_records() {
        let input = b"app.py\x003\x0010\x00def wait(timeout):\napp.py\x004\x009\x00    old: 10\n";
        let matches = parse(input).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].path, PathBuf::from("app.py"));
        assert_eq!(matches[0].line_number, 3);
        assert_eq!(matches[0].column, Some(10));
        assert_eq!(matches[0].text, "def wait(timeout):");
        assert_eq!(matches[1].line_number, 4);
        assert_eq!(matches[1].text, "    old: 10");
    }

    #[test]
    fn text_may_contain_colons() {
        let input = b"a:b.rs\x001\x002\x00key: value: timeout\n";
        let matches = parse(input).unwrap();
        assert_eq!(matches[0].path, PathBuf::from("a:b.rs"));
        assert_eq!(matches[0].text, "key: value: timeout");
    }

    #[test]
    fn next_path_keeps_its_newlines() {
        // GITILANTE_SEARCH_SPEC.md section 54: odd file names survive.
        let input = b"f\x001\x001\x00text\nwe\nird\x002\x001\x00more\n";
        let matches = parse(input).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[1].path, PathBuf::from("we\nird"));
    }

    #[test]
    fn empty_input_is_an_empty_list() {
        assert!(parse(b"").unwrap().is_empty());
    }

    #[test]
    fn malformed_records_report_an_error() {
        assert!(parse(b"only-path\x001\n").is_err());
    }
}
