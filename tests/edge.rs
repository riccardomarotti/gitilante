//! Edge case tests against real Git repositories.
//!
//! Covers the robustness requirements of SPEC.md sections 8, 24 and 25:
//! conflicted (unmerged) files must be represented correctly and their
//! combined diffs must not break the parser, and submodules must not crash
//! the backend.

mod common;

use common::TestRepo;
use gitilante::model::diff::FileStatus;

/// Builds a repository left in an unresolved merge conflict on `f.txt`.
fn conflicted_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("f.txt", b"a\nSIDE\nc\n");
    repo.commit_all("side change");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", b"a\nMAIN\nc\n");
    repo.commit_all("main change");
    let (ok, _, _) = repo.try_git(&["merge", "--no-ff", "-m", "merge side", "side"]);
    assert!(!ok, "the merge must conflict");
    repo
}

#[test]
fn conflicted_files_are_listed_as_unmerged() {
    let repo = conflicted_repo();

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    let entry = &status.entries[0];
    assert!(entry.is_unmerged());
    assert_eq!(entry.path, std::path::PathBuf::from("f.txt"));
    assert_eq!(status.unmerged_entries().count(), 1);
    assert!(status.staged_entries().next().is_none());
    assert!(status.unstaged_entries().next().is_none());
}

#[test]
fn combined_diffs_of_conflicted_files_parse() {
    let repo = conflicted_repo();

    // The whole working tree diff must survive the combined format (SPEC §8).
    let worktree = repo.working_tree_diff();
    assert_eq!(worktree.files.len(), 1);
    let file = &worktree.files[0];
    assert_eq!(file.path(), Some(std::path::Path::new("f.txt")));
    assert_eq!(file.status, FileStatus::Modified);
    assert_eq!(file.hunks.len(), 1);
    let hunk = &file.hunks[0];
    assert_eq!(hunk.lines.len(), 7);
    assert!(
        hunk.lines
            .iter()
            .any(|line| line.content == b"<<<<<<< HEAD")
    );
    assert!(
        hunk.lines
            .iter()
            .any(|line| line.content == b">>>>>>> side")
    );

    // The staged diff only reports the unmerged path (a star record).
    assert!(repo.staged_diff().is_empty());
}

#[test]
fn submodule_changes_do_not_break_status_or_diffs() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"base\n");
    repo.commit_all("base");

    // A nested repository added as a submodule (file protocol is local-only).
    let nested = TestRepo::new();
    nested.write("inner.txt", b"one\n");
    nested.commit_all("inner one");
    nested.write("inner.txt", b"one\ntwo\n");
    nested.commit_all("inner two");

    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        nested.root().to_str().unwrap(),
        "sub",
    ]);
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "-q",
        "--init",
    ]);
    repo.commit_all("add submodule");

    // Move the submodule to another commit: a submodule-only change.
    repo.git(&["-C", "sub", "checkout", "-q", "HEAD~1"]);

    // Minimal representation (SPEC section 25): no crash, entry visible.
    let status = repo.status();
    assert!(
        status
            .entries
            .iter()
            .any(|entry| entry.path == std::path::Path::new("sub"))
    );

    let worktree = repo.working_tree_diff();
    let file = &worktree.files[0];
    assert_eq!(file.path(), Some(std::path::Path::new("sub")));
    assert!(file.hunks.len() <= 1);
}

#[test]
fn symlink_changes_do_not_break_diffs() {
    let repo = TestRepo::new();
    repo.write("target.txt", b"one\n");
    repo.commit_all("base");

    // Replacing a file with a symlink is a type change (SPEC sections 24-25).
    repo.remove("target.txt");
    repo.symlink("somewhere else", "target.txt");

    let worktree = repo.working_tree_diff();
    assert!(!worktree.files.is_empty());
    for file in &worktree.files {
        assert_eq!(file.path(), Some(std::path::Path::new("target.txt")));
    }
}
