//! Historical hunk revert tests against real Git repositories (SPEC §18).
//!
//! Covers the revert cases 19-20 of the test list in SPEC.md, section 32:
//! reverting a hunk of a recent commit must leave a plain local modification
//! without creating commits, and a hunk that no longer applies must change
//! nothing at all (acceptance scenario G).

mod common;

use common::{TestRepo, changed_lines, file, numbered_lines};
use gitilante::git::Error;

#[test]
fn revert_hunk_of_a_recent_commit_leaves_a_local_modification() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    repo.write(
        "multi.txt",
        &changed_lines(
            30,
            &[("line5\n", "CHANGED5\n"), ("line25\n", "CHANGED25\n")],
        ),
    );
    repo.commit_all("the change to revert");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&head).expect("commit diff");
    let commit_file = file(&diff, "multi.txt");
    assert_eq!(commit_file.hunks.len(), 2);

    // Revert only the second hunk (acceptance scenario F).
    backend
        .revert_commit_hunk(commit_file, &commit_file.hunks[1])
        .expect("revert hunk");

    // No commit is created and nothing is staged (SPEC section 18).
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head.as_str());
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());

    // The result is a plain local modification of just that region.
    let worktree = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(worktree.contains("-CHANGED25"));
    assert!(worktree.contains("+line25"));
    assert!(!worktree.contains("CHANGED5"));
    let content = String::from_utf8(repo.read("multi.txt")).unwrap();
    assert!(content.contains("line25\n"));
    assert!(content.contains("CHANGED5\n"));
}

#[test]
fn revert_hunk_fails_when_the_file_changed_since_the_commit() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"a\nB\nc\n");
    repo.commit_all("the change to revert");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    // The same region changes locally after the commit.
    repo.write("f.txt", b"a\nB local\nc\n");
    let before = repo.read("f.txt");
    let diff_before = repo.git_bytes(&["diff"]);

    let backend = repo.repository();
    let diff = backend.commit_diff(&head).expect("commit diff");
    let commit_file = file(&diff, "f.txt");

    match backend.revert_commit_hunk(commit_file, &commit_file.hunks[0]) {
        Err(Error::HunkCannotBeReverted { .. }) => {}
        other => panic!("expected HunkCannotBeReverted, got {other:?}"),
    }

    // Nothing changed at all (acceptance scenario G).
    assert_eq!(repo.read("f.txt"), before);
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head.as_str());
    assert_eq!(repo.git_bytes(&["diff"]), diff_before);
}

#[test]
fn revert_preserves_unrelated_local_changes() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    repo.write(
        "multi.txt",
        &changed_lines(30, &[("line5\n", "CHANGED5\n")]),
    );
    repo.commit_all("the change to revert");

    // A local change in an unrelated region.
    repo.write(
        "multi.txt",
        &changed_lines(30, &[("line5\n", "CHANGED5\n"), ("line25\n", "LOCAL25\n")]),
    );
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&head).expect("commit diff");
    let commit_file = file(&diff, "multi.txt");
    backend
        .revert_commit_hunk(commit_file, &commit_file.hunks[0])
        .expect("revert hunk");

    // Both the revert and the local change are local modifications.
    let worktree = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(worktree.contains("-CHANGED5"));
    assert!(worktree.contains("+line5"));
    assert!(worktree.contains("+LOCAL25"));
    let content = String::from_utf8(repo.read("multi.txt")).unwrap();
    assert!(content.contains("line5\n"));
    assert!(content.contains("LOCAL25\n"));
}

#[test]
fn revert_hunk_with_a_weird_filename() {
    let repo = TestRepo::new();
    repo.write("weird\n\tcaf\u{e9}.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("weird\n\tcaf\u{e9}.txt", b"a\nB\nc\n");
    repo.commit_all("the change to revert");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&head).expect("commit diff");
    let commit_file = file(&diff, "weird\n\tcaf\u{e9}.txt");
    backend
        .revert_commit_hunk(commit_file, &commit_file.hunks[0])
        .expect("revert hunk");

    assert_eq!(repo.read("weird\n\tcaf\u{e9}.txt"), b"a\nb\nc\n");
    let worktree = repo.working_tree_diff();
    assert_eq!(worktree.files.len(), 1);
}
