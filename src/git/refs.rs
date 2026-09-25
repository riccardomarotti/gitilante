//! `git for-each-ref` and HEAD resolution for the History graph
//! (BRANCH.md sections 9-12).
//!
//! Refs are read with a machine-readable format: human-oriented decoration
//! text (`HEAD -> main, origin/main`) is never parsed. Symbolic remote HEADs
//! such as `origin/HEAD` carry no information and are skipped.

use std::collections::HashMap;

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::model::refs::{CommitRef, HeadRef, RefKind};
use crate::model::revisions::{RevisionCandidate, RevisionKind};

/// Machine readable ref record: object name and full refname, NUL separated.
const REF_FORMAT: &str = "%(objectname)%00%(refname)";

/// Result of the output parsers, failing with a human-readable detail.
type ParseResult<T> = std::result::Result<T, String>;

/// Maps the commit of every local and remote branch to its refs.
///
/// Refs are sorted (local branches first) so the badge order is deterministic.
/// A repository without refs yields an empty map (BRANCH.md section 7).
pub fn commit_refs(repo: &Repository) -> Result<HashMap<String, Vec<CommitRef>>> {
    let format_arg = format!("--format={REF_FORMAT}");
    let args = [
        "for-each-ref",
        format_arg.as_str(),
        "refs/heads/",
        "refs/remotes/",
    ];

    let output = command::run(repo.root(), &args)?;
    let entries = parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
        command: "for-each-ref".to_owned(),
        detail,
    })?;

    let mut refs: HashMap<String, Vec<CommitRef>> = HashMap::new();
    for (oid, commit_ref) in entries {
        refs.entry(oid).or_default().push(commit_ref);
    }
    for commit_refs in refs.values_mut() {
        commit_refs.sort();
    }
    Ok(refs)
}

/// Returns deterministic suggestions for arbitrary commit-ish input fields.
///
/// This is deliberately separate from [`commit_refs`]: tags belong in the
/// comparison dialog but not in History graph badges.
pub fn revision_candidates(repo: &Repository) -> Result<Vec<RevisionCandidate>> {
    let output = command::run(
        repo.root(),
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )?;
    let refs = std::str::from_utf8(&output.stdout).map_err(|error| Error::MalformedOutput {
        command: "for-each-ref".to_owned(),
        detail: format!("expected UTF-8 ref names: {error}"),
    })?;

    let mut candidates = Vec::new();
    if repo.head()?.is_some() {
        candidates.push(RevisionCandidate {
            label: "HEAD".to_owned(),
            spec: "HEAD".to_owned(),
            kind: RevisionKind::Head,
        });
    }
    for refname in refs.lines() {
        let (prefix, kind) = if let Some(name) = refname.strip_prefix("refs/heads/") {
            (name, RevisionKind::LocalBranch)
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if name == "HEAD" || name.ends_with("/HEAD") {
                continue;
            }
            (name, RevisionKind::RemoteBranch)
        } else if let Some(name) = refname.strip_prefix("refs/tags/") {
            (name, RevisionKind::Tag)
        } else {
            continue;
        };
        candidates.push(RevisionCandidate {
            label: prefix.to_owned(),
            spec: refname.to_owned(),
            kind,
        });
    }
    candidates.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.spec.cmp(&right.spec))
    });
    Ok(candidates)
}

/// Resolves HEAD (BRANCH.md section 11).
///
/// Returns `None` on an unborn branch (a repository without commits). A
/// detached HEAD reports `branch: None`.
pub fn head(repo: &Repository) -> Result<Option<HeadRef>> {
    let oid = match command::run(repo.root(), &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Err(Error::Git(_)) => return Ok(None),
        Err(error) => return Err(error),
    };

    let branch = match command::run(repo.root(), &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(output) => Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        Err(Error::Git(_)) => None,
        Err(error) => return Err(error),
    };

    Ok(Some(HeadRef { oid, branch }))
}

/// Parses `git for-each-ref` output produced with [`REF_FORMAT`].
///
/// Symbolic remote HEADs are skipped (BRANCH.md section 12).
pub fn parse(input: &[u8]) -> ParseResult<Vec<(String, CommitRef)>> {
    let mut entries = Vec::new();
    for line in input.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&[u8]> = line.split(|byte| *byte == 0).collect();
        if fields.len() != 2 {
            return Err(format!(
                "malformed ref record with {} fields: {:?}",
                fields.len(),
                String::from_utf8_lossy(line)
            ));
        }
        let oid = text(fields[0])?;
        let refname = text(fields[1])?;
        let Some(name) = classify(&refname) else {
            continue;
        };
        entries.push((oid, name));
    }
    Ok(entries)
}

/// Classifies a full refname; `None` for refs that are not displayed.
fn classify(refname: &str) -> Option<CommitRef> {
    if let Some(name) = refname.strip_prefix("refs/heads/") {
        return Some(CommitRef {
            name: name.to_owned(),
            kind: RefKind::LocalBranch,
        });
    }
    if let Some(name) = refname.strip_prefix("refs/remotes/") {
        // Symbolic remote HEADs like origin/HEAD add no information.
        if name == "HEAD" || name.ends_with("/HEAD") {
            return None;
        }
        return Some(CommitRef {
            name: name.to_owned(),
            kind: RefKind::RemoteBranch,
        });
    }
    None
}

/// Decodes an ASCII field such as the object name.
fn text(bytes: &[u8]) -> ParseResult<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| format!("expected UTF-8 text: {:?}", String::from_utf8_lossy(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote_refs() {
        let input = b"abc123\x00refs/heads/main\nabc123\x00refs/remotes/origin/main\ndef456\x00refs/heads/feature\n";
        let entries = parse(input).unwrap();
        assert_eq!(
            entries,
            vec![
                (
                    "abc123".to_owned(),
                    CommitRef {
                        name: "main".to_owned(),
                        kind: RefKind::LocalBranch
                    }
                ),
                (
                    "abc123".to_owned(),
                    CommitRef {
                        name: "origin/main".to_owned(),
                        kind: RefKind::RemoteBranch
                    }
                ),
                (
                    "def456".to_owned(),
                    CommitRef {
                        name: "feature".to_owned(),
                        kind: RefKind::LocalBranch
                    }
                ),
            ]
        );
    }

    #[test]
    fn symbolic_remote_heads_are_skipped() {
        let input = b"abc123\x00refs/remotes/origin/HEAD\nabc123\x00refs/remotes/origin/main\n";
        let entries = parse(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.name, "origin/main");
    }

    #[test]
    fn empty_input_is_an_empty_ref_list() {
        assert!(parse(b"").unwrap().is_empty());
    }

    #[test]
    fn malformed_records_report_an_error() {
        assert!(parse(b"only-one-field\n").is_err());
    }
}
