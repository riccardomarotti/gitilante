//! `git log` and `git show` for the History view (SPEC sections 16 and 17).
//!
//! History is loaded incrementally with `--skip`/`--max-count`, one NUL
//! separated record per commit. Commit diffs go through the same unified diff
//! parser and renderer used for the working tree and the index.

use crate::git::Result;
use crate::git::command;
use crate::git::diff::STABLE_DIFF_FLAGS;
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::model::commit::Commit;
use crate::model::diff::Diff;

/// Machine readable commit record: object name, parents, author name, author
/// email, author time and subject, separated by NUL (one record per line).
const LOG_FORMAT: &str = "%H%x00%P%x00%an%x00%ae%x00%at%x00%s";

/// Result of the output parsers, failing with a human-readable detail.
type ParseResult<T> = std::result::Result<T, String>;

/// Loads up to `max_count` commits of HEAD starting at `skip`.
///
/// A repository without commits has no history and yields an empty list.
pub fn history(repo: &Repository, skip: usize, max_count: usize) -> Result<Vec<Commit>> {
    if !head_exists(repo)? {
        return Ok(Vec::new());
    }

    let args = [
        "-c".to_owned(),
        "core.quotePath=true".to_owned(),
        "log".to_owned(),
        // Machine readable output must not depend on user configuration.
        "--no-decorate".to_owned(),
        "--no-show-signature".to_owned(),
        format!("--format={LOG_FORMAT}"),
        format!("--skip={skip}"),
        format!("--max-count={max_count}"),
        "HEAD".to_owned(),
    ];

    let output = command::run(repo.root(), &args)?;
    parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
        command: "log".to_owned(),
        detail,
    })
}

/// Diff of a single commit (`git show --format= --patch`).
///
/// The result uses the same [`Diff`] model and renderer as the working tree
/// and staged diffs (SPEC section 17).
pub fn commit_diff(repo: &Repository, oid: &str) -> Result<Diff> {
    let mut args = vec![
        "-c".to_owned(),
        "core.quotePath=true".to_owned(),
        "show".to_owned(),
        "--format=".to_owned(),
        "--patch".to_owned(),
    ];
    args.extend(STABLE_DIFF_FLAGS.iter().map(|flag| (*flag).to_owned()));
    args.push(oid.to_owned());

    let output = command::run(repo.root(), &args)?;
    crate::git::diff::parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
        command: "show".to_owned(),
        detail,
    })
}

/// Parses `git log` output produced with [`LOG_FORMAT`].
pub fn parse(input: &[u8]) -> ParseResult<Vec<Commit>> {
    let mut commits = Vec::new();
    for line in input.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&[u8]> = line.split(|byte| *byte == 0).collect();
        if fields.len() != 6 {
            return Err(format!(
                "malformed commit record with {} fields: {:?}",
                fields.len(),
                String::from_utf8_lossy(line)
            ));
        }
        let time_text = text(fields[4])?;
        let author_time = time_text
            .parse()
            .map_err(|_| format!("malformed author time: {time_text:?}"))?;
        commits.push(Commit {
            oid: text(fields[0])?,
            parents: fields[1]
                .split(|byte| *byte == b' ')
                .filter(|parent| !parent.is_empty())
                .map(|parent| String::from_utf8_lossy(parent).into_owned())
                .collect(),
            author_name: String::from_utf8_lossy(fields[2]).into_owned(),
            author_email: String::from_utf8_lossy(fields[3]).into_owned(),
            author_time,
            subject: String::from_utf8_lossy(fields[5]).into_owned(),
        });
    }
    Ok(commits)
}

/// True when HEAD resolves to a commit (false on an unborn branch).
fn head_exists(repo: &Repository) -> Result<bool> {
    match command::run(repo.root(), &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Ok(_) => Ok(true),
        Err(Error::Git(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Decodes an ASCII field such as the object name or the author time.
fn text(bytes: &[u8]) -> ParseResult<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| format!("expected UTF-8 text: {:?}", String::from_utf8_lossy(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commits_with_metadata() {
        let input = b"3421e7af01\x0048b811915f 66719ece15\x00Ada Lovelace\x00ada@example.com\x001790138795\x00Add Phase 4\n\
                      66719ece15\x00\x00Ada Lovelace\x00ada@example.com\x001790103508\x00Initial commit\n";
        let commits = parse(input).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, "3421e7af01");
        assert_eq!(
            commits[0].parents,
            vec!["48b811915f".to_owned(), "66719ece15".to_owned()]
        );
        assert_eq!(commits[0].author_name, "Ada Lovelace");
        assert_eq!(commits[0].author_email, "ada@example.com");
        assert_eq!(commits[0].author_time, 1790138795);
        assert_eq!(commits[0].subject, "Add Phase 4");
        assert!(commits[1].parents.is_empty());
    }

    #[test]
    fn short_oid_is_seven_characters() {
        let commits = parse(b"3421e7af013d6385\x00\x00a\x00a@a\x001\x00subject\n").unwrap();
        assert_eq!(commits[0].short_oid(), "3421e7a");
    }

    #[test]
    fn empty_input_is_an_empty_history() {
        assert!(parse(b"").unwrap().is_empty());
    }

    #[test]
    fn malformed_records_report_an_error() {
        assert!(parse(b"only\x00three\n").is_err());
        assert!(parse(b"oid\x00\x00name\x00mail\x00not-a-time\x00subject\n").is_err());
    }
}
