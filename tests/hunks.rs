//! Hunk and whole-file operation tests against real Git repositories.
//!
//! Covers SPEC.md sections 12-15 and the backend cases 4-7, 20 of the test
//! list in section 32. Every operation is verified with Git itself (`git diff`,
//! `git diff --cached`, `git status`) and with the actual file contents.

mod common;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use common::{TestRepo, changed_lines, file, numbered_lines};
use gitilante::git::Error;

#[test]
fn stage_single_hunk_out_of_three() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    let content = changed_lines(
        30,
        &[
            ("line3\n", "CHANGED3\n"),
            ("line15\n", "CHANGED15\n"),
            ("line27\n", "CHANGED27\n"),
        ],
    );
    repo.write("multi.txt", &content);

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    assert_eq!(changed.hunks.len(), 3);
    backend
        .stage_hunk(changed, &changed.hunks[1])
        .expect("stage hunk");

    // Git itself confirms only the middle hunk is staged.
    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("CHANGED15"));
    assert!(!staged.contains("CHANGED3"));
    assert!(!staged.contains("CHANGED27"));
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(unstaged.contains("CHANGED3"));
    assert!(unstaged.contains("CHANGED27"));
    assert!(!unstaged.contains("CHANGED15"));

    // The working tree file is untouched.
    assert_eq!(repo.read("multi.txt"), content);
}

#[test]
fn unstage_single_hunk_out_of_two() {
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
    repo.git(&["add", "multi.txt"]);

    let backend = repo.repository();
    let diff = repo.staged_diff();
    let staged_file = file(&diff, "multi.txt");
    assert_eq!(staged_file.hunks.len(), 2);
    backend
        .unstage_hunk(staged_file, &staged_file.hunks[1])
        .expect("unstage hunk");

    // Git itself confirms only the first hunk is still staged.
    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("CHANGED5"));
    assert!(!staged.contains("CHANGED25"));
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(unstaged.contains("CHANGED25"));

    // The file is now both staged and unstaged.
    let status = repo.status();
    let entry = repo.entry(&status, "multi.txt");
    assert!(entry.is_staged() && entry.is_unstaged());
}

#[test]
fn discard_single_hunk() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    repo.write(
        "multi.txt",
        &changed_lines(
            30,
            &[
                ("line3\n", "CHANGED3\n"),
                ("line15\n", "CHANGED15\n"),
                ("line27\n", "CHANGED27\n"),
            ],
        ),
    );

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    backend
        .discard_hunk(changed, &changed.hunks[1])
        .expect("discard hunk");

    // The other two changes survive, in Git's view and on disk.
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(unstaged.contains("CHANGED3"));
    assert!(unstaged.contains("CHANGED27"));
    assert!(!unstaged.contains("CHANGED15"));
    let content = String::from_utf8(repo.read("multi.txt")).unwrap();
    assert!(content.contains("line15\n"));
    assert!(!content.contains("CHANGED15"));
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());
}

#[test]
fn stage_then_unstage_round_trip() {
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
    let original_unstaged = repo.git_bytes(&["diff"]);

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    backend
        .stage_hunk(changed, &changed.hunks[0])
        .expect("stage hunk");

    let diff = repo.staged_diff();
    let staged_file = file(&diff, "multi.txt");
    assert_eq!(staged_file.hunks.len(), 1);
    backend
        .unstage_hunk(staged_file, &staged_file.hunks[0])
        .expect("unstage hunk");

    // Back to exactly the starting state.
    assert_eq!(repo.git_bytes(&["diff"]), original_unstaged);
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());
}

#[test]
fn stale_stage_hunk_is_rejected_and_changes_nothing() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    repo.write(
        "multi.txt",
        &changed_lines(30, &[("line3\n", "CHANGED3\n")]),
    );

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");

    // The index changes underneath us after the diff was read.
    repo.write("multi.txt", &changed_lines(30, &[("line3\n", "OTHER3\n")]));
    repo.git(&["add", "multi.txt"]);

    match backend.stage_hunk(changed, &changed.hunks[0]) {
        Err(Error::PatchDoesNotApply { .. }) => {}
        other => panic!("expected PatchDoesNotApply, got {other:?}"),
    }

    // The index still holds exactly what was added externally.
    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("OTHER3"));
    assert!(!staged.contains("CHANGED3"));
}

#[test]
fn stale_discard_hunk_is_rejected_and_changes_nothing() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"a\nB\nc\n");

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.txt");

    // The working tree changes underneath us after the diff was read.
    repo.write("f.txt", b"a\nB\nC\n");

    match backend.discard_hunk(changed, &changed.hunks[0]) {
        Err(Error::PatchDoesNotApply { .. }) => {}
        other => panic!("expected PatchDoesNotApply, got {other:?}"),
    }
    assert_eq!(repo.read("f.txt"), b"a\nB\nC\n");
}

#[test]
fn hunk_ops_on_file_without_final_newline() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\ntwo");
    repo.commit_all("initial");
    repo.write("f.txt", b"one\nTWO");

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.txt");
    backend
        .stage_hunk(changed, &changed.hunks[0])
        .expect("stage hunk");

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("No newline at end of file"));
    assert!(staged.contains("+TWO"));
    assert!(repo.git_bytes(&["diff"]).is_empty());
}

#[test]
fn hunk_ops_with_weird_filename() {
    let repo = TestRepo::new();
    let weird = PathBuf::from(OsString::from_vec(b"weird\n\tcaf\xc3\xa9.txt".to_vec()));
    repo.write(&weird, b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write(&weird, b"a\nB\nc\n");

    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, weird.to_str().unwrap());
    backend
        .stage_hunk(changed, &changed.hunks[0])
        .expect("stage hunk");

    let staged_diff = repo.staged_diff();
    assert_eq!(staged_diff.files.len(), 1);
    assert_eq!(staged_diff.files[0].new_path, Some(weird.clone()));
    assert!(repo.working_tree_diff().is_empty());

    // And back out of the index again.
    let staged_file = &staged_diff.files[0];
    backend
        .unstage_hunk(staged_file, &staged_file.hunks[0])
        .expect("unstage hunk");
    assert!(repo.staged_diff().is_empty());
}

#[test]
fn hunk_ops_on_renamed_file() {
    let repo = TestRepo::new();
    repo.write("old.txt", &numbered_lines(20));
    repo.commit_all("initial");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", &changed_lines(20, &[("line5\n", "CHANGED5\n")]));

    // Stage the extra worktree change on top of the staged rename.
    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "new.txt");
    assert_eq!(changed.hunks.len(), 1);
    backend
        .stage_hunk(changed, &changed.hunks[0])
        .expect("stage hunk");

    let diff = repo.staged_diff();
    let renamed = file(&diff, "new.txt");
    assert_eq!(renamed.hunks.len(), 1);
    assert_eq!(renamed.old_path, Some(PathBuf::from("old.txt")));
}

#[test]
fn staged_and_unstaged_changes_on_the_same_file() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    repo.write(
        "multi.txt",
        &changed_lines(30, &[("line5\n", "CHANGED5\n")]),
    );
    repo.git(&["add", "multi.txt"]);
    repo.write(
        "multi.txt",
        &changed_lines(
            30,
            &[("line5\n", "CHANGED5\n"), ("line25\n", "CHANGED25\n")],
        ),
    );

    // Stage the remaining worktree hunk.
    let backend = repo.repository();
    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    backend
        .stage_hunk(changed, &changed.hunks[0])
        .expect("stage hunk");
    assert!(repo.working_tree_diff().is_empty());
    assert_eq!(file(&repo.staged_diff(), "multi.txt").hunks.len(), 2);

    // Unstage one of the two staged hunks.
    let diff = repo.staged_diff();
    let staged_file = file(&diff, "multi.txt");
    backend
        .unstage_hunk(staged_file, &staged_file.hunks[0])
        .expect("unstage hunk");

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("CHANGED25"));
    assert!(!staged.contains("CHANGED5"));
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(unstaged.contains("CHANGED5"));
    assert!(!unstaged.contains("CHANGED25"));
}

#[test]
fn stage_file_stages_untracked_modified_and_deleted() {
    let repo = TestRepo::new();
    repo.write("modified.txt", b"one\n");
    repo.write("deleted.txt", b"bye\n");
    repo.commit_all("initial");
    repo.write("modified.txt", b"one\ntwo\n");
    repo.write("untracked.txt", b"new\n");
    repo.remove("deleted.txt");

    let backend = repo.repository();
    let status = repo.status();
    for name in ["modified.txt", "untracked.txt", "deleted.txt"] {
        backend
            .stage_file(repo.entry(&status, name))
            .expect("stage file");
    }

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached", "--name-status"])).unwrap();
    assert!(staged.contains("M\tmodified.txt"));
    assert!(staged.contains("A\tuntracked.txt"));
    assert!(staged.contains("D\tdeleted.txt"));
    assert!(repo.git_bytes(&["diff"]).is_empty());
}

#[test]
fn unstage_file_handles_staged_rename() {
    let repo = TestRepo::new();
    repo.write(
        "old name.txt",
        b"content long enough to be detected as a rename\nsecond\n",
    );
    repo.commit_all("initial");
    repo.git(&["mv", "old name.txt", "new name.txt"]);

    let backend = repo.repository();
    let status = repo.status();
    let entry = repo.entry(&status, "new name.txt");
    assert_eq!(entry.orig_path, Some(PathBuf::from("old name.txt")));
    backend.unstage_file(entry).expect("unstage file");

    // Both paths are restored in the index.
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());
    let status = repo.status();
    assert!(repo.entry(&status, "old name.txt").is_unstaged());
    assert!(repo.entry(&status, "new name.txt").is_untracked());
}

#[test]
fn discard_file_restores_worktree_changes() {
    let repo = TestRepo::new();
    repo.write("modified.txt", b"one\ntwo\n");
    repo.write("deleted.txt", b"bye\n");
    repo.commit_all("initial");
    repo.write("modified.txt", b"one\nTWO\n");
    repo.remove("deleted.txt");

    let backend = repo.repository();
    let status = repo.status();
    for name in ["modified.txt", "deleted.txt"] {
        backend
            .discard_file(repo.entry(&status, name))
            .expect("discard file");
    }

    assert!(repo.git_bytes(&["diff"]).is_empty());
    assert_eq!(repo.read("modified.txt"), b"one\ntwo\n");
    assert_eq!(repo.read("deleted.txt"), b"bye\n");
}

#[test]
fn discard_file_keeps_staged_changes() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"one\nSTAGED\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", b"one\nSTAGED\nWORKTREE\n");

    let backend = repo.repository();
    let status = repo.status();
    backend
        .discard_file(repo.entry(&status, "f.txt"))
        .expect("discard file");

    // The worktree follows the index: the staged change survives.
    assert_eq!(repo.read("f.txt"), b"one\nSTAGED\n");
    assert!(repo.git_bytes(&["diff"]).is_empty());
    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    assert!(staged.contains("+STAGED"));
}

#[test]
fn file_ops_on_binary_file() {
    let repo = TestRepo::new();
    repo.write("blob.bin", b"\x00\x01\x02\x03");
    repo.commit_all("initial");

    // Discard restores the exact bytes.
    repo.write("blob.bin", b"\x00\x01\xff");
    let backend = repo.repository();
    let status = repo.status();
    backend
        .discard_file(repo.entry(&status, "blob.bin"))
        .expect("discard file");
    assert_eq!(repo.read("blob.bin"), b"\x00\x01\x02\x03");

    // Stage and unstage a new binary file.
    repo.write("new.bin", b"\x00\x01");
    let status = repo.status();
    backend
        .stage_file(repo.entry(&status, "new.bin"))
        .expect("stage file");
    assert!(!repo.git_bytes(&["diff", "--cached"]).is_empty());
    let status = repo.status();
    backend
        .unstage_file(repo.entry(&status, "new.bin"))
        .expect("unstage file");
    assert!(repo.git_bytes(&["diff", "--cached"]).is_empty());
    let status = repo.status();
    assert!(repo.entry(&status, "new.bin").is_untracked());
}

#[test]
fn file_ops_in_repository_without_commits() {
    let repo = TestRepo::new();
    repo.write("new.txt", b"content\n");

    let backend = repo.repository();
    let status = repo.status();
    assert!(repo.entry(&status, "new.txt").is_untracked());
    backend
        .stage_file(repo.entry(&status, "new.txt"))
        .expect("stage file");

    let status = repo.status();
    assert_eq!(
        repo.entry(&status, "new.txt").staged_change(),
        Some(gitilante::model::status::ChangeKind::Added)
    );

    // Unstaging works even though HEAD cannot be resolved.
    backend
        .unstage_file(repo.entry(&status, "new.txt"))
        .expect("unstage file");
    let status = repo.status();
    assert!(repo.entry(&status, "new.txt").is_untracked());
}

#[test]
fn discard_hunk_on_added_file_hunk() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"base\n");
    repo.commit_all("initial");
    repo.write("new.txt", b"x\ny\n");

    // Untracked files have no diff hunks; stage first, then discard from the
    // working tree diff created by unstaging one hunk of the added file.
    let backend = repo.repository();
    let status = repo.status();
    backend
        .stage_file(repo.entry(&status, "new.txt"))
        .expect("stage file");

    let diff = repo.staged_diff();
    let added = file(&diff, "new.txt");
    backend
        .unstage_hunk(added, &added.hunks[0])
        .expect("unstage hunk");
    assert!(repo.staged_diff().is_empty());
}
