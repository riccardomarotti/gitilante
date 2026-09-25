//! Commit-to-commit revision comparison.
//!
//! Inputs are resolved once to full commit object IDs before the diff runs, so
//! moving refs cannot change the comparison after resolution.

use crate::git::Result;
use crate::git::command;
use crate::git::diff::STABLE_DIFF_FLAGS;
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::model::revisions::RevisionComparison;

/// Resolves one arbitrary Git revision expression to a full commit object ID.
pub fn resolve_revision(repo: &Repository, revision: &str) -> Result<String> {
    let revision = revision.trim();
    if revision.is_empty() {
        return Err(Error::InvalidRevision {
            revision: revision.to_owned(),
            detail: "revision is empty".to_owned(),
        });
    }
    let expression = format!("{revision}^{{commit}}");
    let output = command::run(
        repo.root(),
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            expression.as_str(),
        ],
    )
    .map_err(|error| match error {
        Error::Git(git_error) => Error::InvalidRevision {
            revision: revision.to_owned(),
            detail: git_error.stderr,
        },
        other => other,
    })?;
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::MalformedOutput {
            command: "rev-parse".to_owned(),
            detail: format!("expected a full commit object ID, got {oid:?}"),
        });
    }
    Ok(oid.to_ascii_lowercase())
}

/// Resolves both inputs and runs the ordinary two-commit `git diff`.
pub fn compare_revisions(repo: &Repository, from: &str, to: &str) -> Result<RevisionComparison> {
    let from_input = from.trim().to_owned();
    let to_input = to.trim().to_owned();
    let from_oid = resolve_revision(repo, &from_input)?;
    let to_oid = resolve_revision(repo, &to_input)?;

    let mut args = vec![
        "-c".to_owned(),
        "core.quotePath=true".to_owned(),
        "diff".to_owned(),
        "-M".to_owned(),
    ];
    args.extend(STABLE_DIFF_FLAGS.iter().map(|flag| (*flag).to_owned()));
    args.push(from_oid.clone());
    args.push(to_oid.clone());
    let output = command::run(repo.root(), &args)?;
    let mut diff =
        crate::git::diff::parse(&output.stdout).map_err(|detail| Error::MalformedOutput {
            command: "diff".to_owned(),
            detail,
        })?;
    crate::intraline::analyze_diff(&mut diff);

    Ok(RevisionComparison {
        from_input,
        to_input,
        from_oid,
        to_oid,
        diff,
    })
}
