//! Conflict-solver Git operations: operation detection and stage access.
//!
//! The operation is detected from Git pseudo-refs and plumbing only; it is
//! never deduced from conflict marker text (spec §4).

use crate::conflict::context::{CommitIdentity, ConflictContext, ConflictOperation};
use crate::conflict::presentation::{ConflictPresentation, classify};
use crate::git::error::Error;
use crate::git::repository::Repository;
use crate::git::{Result, command, status};
use crate::model::status::UnmergedInfo;
use std::ffi::OsStr;
use std::path::Path;

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

/// Everything the conflict solver needs for one conflicted path (§58).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictLoad {
    /// Operation context with the resolved labels.
    pub context: ConflictContext,
    /// Unmerged stage metadata from the status record.
    pub unmerged: UnmergedInfo,
    /// Working tree snapshot taken while opening the solver (§16, §39).
    pub working_tree: Option<Vec<u8>>,
    /// Stage 2 blob content, when the stage exists.
    pub stage_a: Option<Vec<u8>>,
    /// Stage 3 blob content, when the stage exists.
    pub stage_b: Option<Vec<u8>>,
    /// How the conflict must be presented (§55).
    pub presentation: ConflictPresentation,
}

/// Loads the conflict state of one path, or `None` when it is not unmerged.
///
/// The working tree file is the primary source of the in-progress result:
/// manual edits made outside Gitilante are preserved exactly (§16).
pub fn load(repo: &Repository, path: &Path) -> Result<Option<ConflictLoad>> {
    let repository_status = status::status(repo)?;
    let Some(entry) = repository_status
        .entries
        .iter()
        .find(|entry| entry.path == path)
    else {
        return Ok(None);
    };
    let Some(unmerged) = entry.unmerged().cloned() else {
        return Ok(None);
    };

    let working_tree = read_working_tree(repo, path)?;
    let stage_a = read_stage(repo, &unmerged.stage2)?;
    let stage_b = read_stage(repo, &unmerged.stage3)?;
    let presentation = classify(
        &unmerged,
        working_tree.as_deref(),
        stage_a.as_deref(),
        stage_b.as_deref(),
    );
    Ok(Some(ConflictLoad {
        context: conflict_context(repo)?,
        unmerged,
        working_tree,
        stage_a,
        stage_b,
        presentation,
    }))
}

/// Reads the working tree file; a missing file is `None` (§48-50).
fn read_working_tree(repo: &Repository, path: &Path) -> Result<Option<Vec<u8>>> {
    let full_path = repo.root().join(path);
    match std::fs::read(&full_path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            command: format!("read working tree file {path:?}"),
            source,
        }),
    }
}

/// Reads one stage blob through its OID (§15).
fn read_stage(
    repo: &Repository,
    stage: &crate::model::status::ConflictStage,
) -> Result<Option<Vec<u8>>> {
    match &stage.oid {
        Some(oid) => Ok(Some(
            command::run(
                repo.root(),
                &[OsStr::new("cat-file"), OsStr::new("blob"), oid.as_ref()],
            )?
            .stdout,
        )),
        None => Ok(None),
    }
}

/// Verifies the working tree file still matches the solver's snapshot (§39).
///
/// The caller must refuse to apply a resolution when this returns `false`:
/// external edits are never overwritten silently (§40).
pub fn unchanged_since(repo: &Repository, path: &Path, snapshot: Option<&[u8]>) -> Result<bool> {
    Ok(read_working_tree(repo, path)?.as_deref() == snapshot)
}

/// Writes the resolution to the working tree and stages it (§43).
///
/// Writing and staging are not atomic: when staging fails the file keeps the
/// resolution on disk and the error is surfaced unchanged (§44).
pub fn apply_resolution(repo: &Repository, path: &Path, content: &[u8]) -> Result<()> {
    let full_path = repo.root().join(path);
    std::fs::write(&full_path, content).map_err(|source| Error::Io {
        command: format!("write working tree file {path:?}"),
        source,
    })?;
    mark_resolved(repo, path)
}

/// Stages the current working tree content as the resolution (§41).
///
/// The file content is not modified.
pub fn mark_resolved(repo: &Repository, path: &Path) -> Result<()> {
    let args = [OsStr::new("add"), OsStr::new("--"), path.as_os_str()];
    command::run(repo.root(), &args)?;
    Ok(())
}

/// Resolves a file-level conflict by deleting the file (§45).
pub fn mark_deleted(repo: &Repository, path: &Path) -> Result<()> {
    let args = [
        OsStr::new("rm"),
        OsStr::new("-f"),
        OsStr::new("-q"),
        OsStr::new("--"),
        path.as_os_str(),
    ];
    command::run(repo.root(), &args)?;
    Ok(())
}
