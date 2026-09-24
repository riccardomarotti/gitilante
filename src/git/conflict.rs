//! Conflict-solver Git operations: operation detection and stage access.
//!
//! The operation is detected from Git pseudo-refs and plumbing only; it is
//! never deduced from conflict marker text (spec §4).

use crate::conflict::context::{CommitIdentity, ConflictContext, ConflictOperation};
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::git::{Result, command};

/// Detects the operation in progress and builds the conflict context.
pub fn conflict_context(repo: &Repository) -> Result<ConflictContext> {
    let current_branch = current_branch(repo)?;
    let current = CommitIdentity {
        oid: verify_oid(repo, "HEAD")?,
        ..CommitIdentity::default()
    };
    let (operation, incoming_oid) = detect_operation(repo)?;
    let incoming = match incoming_oid {
        Some(oid) => identity_for(repo, &oid)?,
        None => CommitIdentity::default(),
    };
    Ok(ConflictContext::new(
        operation,
        current_branch,
        current,
        incoming,
    ))
}

/// Maps the first pseudo-ref found to its operation.
///
/// Most specific first: `REBASE_HEAD` and friends are single-operation
/// markers, while a merge can linger in the background of other states.
fn detect_operation(repo: &Repository) -> Result<(ConflictOperation, Option<String>)> {
    for (revision, operation) in [
        ("REBASE_HEAD", ConflictOperation::Rebase),
        ("CHERRY_PICK_HEAD", ConflictOperation::CherryPick),
        ("REVERT_HEAD", ConflictOperation::Revert),
        ("MERGE_HEAD", ConflictOperation::Merge),
    ] {
        if let Some(oid) = verify_oid(repo, revision)? {
            return Ok((operation, Some(oid)));
        }
    }
    Ok((ConflictOperation::Unknown, None))
}

/// OID of a revision, or `None` when it does not exist.
fn verify_oid(repo: &Repository, revision: &str) -> Result<Option<String>> {
    match command::run(repo.root(), &["rev-parse", "-q", "--verify", revision]) {
        Ok(output) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )),
        Err(Error::Git(error)) if error.exit_code == Some(1) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Branch HEAD points at, or `None` when HEAD is detached or unborn.
fn current_branch(repo: &Repository) -> Result<Option<String>> {
    match command::run(repo.root(), &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(output) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )),
        Err(Error::Git(error)) if error.exit_code == Some(1) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Collects OID, subject and branch names pointing at a commit.
fn identity_for(repo: &Repository, oid: &str) -> Result<CommitIdentity> {
    let subject_output = command::run(repo.root(), &["log", "-1", "--format=%s", oid])?;
    let refs_output = command::run(
        repo.root(),
        &[
            "for-each-ref",
            "--points-at",
            oid,
            "--format=%(refname:short)",
            "refs/heads",
        ],
    )?;
    Ok(CommitIdentity {
        oid: Some(oid.to_owned()),
        subject: Some(
            String::from_utf8_lossy(&subject_output.stdout)
                .trim()
                .to_owned(),
        ),
        refs: String::from_utf8_lossy(&refs_output.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
    })
}
