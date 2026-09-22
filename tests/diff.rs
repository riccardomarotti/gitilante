//! Diff model and patch reconstruction tests against real Git repositories.
//!
//! The case list follows SPEC.md, section 33. Patch reconstruction is verified
//! the strictest way available: feeding the rebuilt patch to `git apply` and
//! comparing it byte for byte with the original Git output.

mod common;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use common::TestRepo;
use gitilante::git::patch::single_hunk_patch;
use gitilante::model::diff::{Diff, DiffLineKind, FileDiff, FileStatus};

/// Finds a file diff by path, panicking when it is missing.
fn file<'a>(diff: &'a Diff, name: &str) -> &'a FileDiff {
    diff.files
        .iter()
        .find(|file| file.path() == Some(Path::new(name)))
        .unwrap_or_else(|| panic!("no file diff for {name:?}, got {:#?}", diff.files))
}

/// Builds a file with `count` numbered lines.
fn numbered_lines(count: usize) -> Vec<u8> {
    (1..=count)
        .map(|number| format!("line{number}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn empty_diff_for_a_clean_worktree() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"content\n");
    repo.commit_all("initial");

    assert!(repo.working_tree_diff().is_empty());
    assert!(repo.staged_diff().is_empty());
}

#[test]
fn single_hunk_file() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"a\nB\nc\n");

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.txt");
    assert_eq!(changed.status, FileStatus::Modified);
    assert_eq!(changed.old_path, Some(PathBuf::from("f.txt")));
    assert_eq!(changed.new_path, Some(PathBuf::from("f.txt")));
    assert!(!changed.binary);
    assert_eq!(changed.hunks.len(), 1);

    let hunk = &changed.hunks[0];
    assert_eq!((hunk.old_start, hunk.old_count), (1, 3));
    assert_eq!((hunk.new_start, hunk.new_count), (1, 3));
    let kinds = hunk.lines.iter().map(|line| line.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            DiffLineKind::Context,
            DiffLineKind::Deletion,
            DiffLineKind::Addition,
            DiffLineKind::Context
        ]
    );
    assert_eq!(hunk.lines[1].content, b"b");
    assert_eq!(hunk.lines[2].content, b"B");
}

#[test]
fn multiple_hunks_in_one_file() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(20));
    repo.commit_all("initial");

    let mut content = String::from_utf8(numbered_lines(20)).unwrap();
    content = content
        .replace("line8\n", "CHANGED8\n")
        .replace("line17\n", "CHANGED17\n");
    repo.write("multi.txt", content.as_bytes());

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    assert_eq!(changed.hunks.len(), 2);
    assert_eq!(changed.hunks[0].old_start, 5);
    assert_eq!(changed.hunks[1].old_start, 14);
}

#[test]
fn added_file_has_no_old_path() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"base\n");
    repo.commit_all("initial");
    repo.write("new.txt", b"x\ny\n");
    repo.git(&["add", "new.txt"]);

    let diff = repo.staged_diff();
    let added = file(&diff, "new.txt");
    assert_eq!(added.status, FileStatus::Added);
    assert_eq!(added.old_path, None);
    assert_eq!(added.new_path, Some(PathBuf::from("new.txt")));
    assert_eq!(added.hunks.len(), 1);
    assert_eq!((added.hunks[0].old_start, added.hunks[0].old_count), (0, 0));
}

#[test]
fn deleted_file_has_no_new_path() {
    let repo = TestRepo::new();
    repo.write("gone.txt", b"bye\n");
    repo.commit_all("initial");
    repo.git(&["rm", "-q", "gone.txt"]);

    let diff = repo.staged_diff();
    let deleted = file(&diff, "gone.txt");
    assert_eq!(deleted.status, FileStatus::Deleted);
    assert_eq!(deleted.new_path, None);
    assert_eq!(deleted.hunks.len(), 1);
}

#[test]
fn pure_rename_keeps_both_paths() {
    let repo = TestRepo::new();
    repo.write(
        "old name.txt",
        b"content long enough to be detected as a rename\nsecond\n",
    );
    repo.commit_all("initial");
    repo.git(&["mv", "old name.txt", "new name.txt"]);

    let diff = repo.staged_diff();
    assert_eq!(diff.files.len(), 1);
    let renamed = file(&diff, "new name.txt");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path, Some(PathBuf::from("old name.txt")));
    assert_eq!(renamed.new_path, Some(PathBuf::from("new name.txt")));
    assert!(renamed.hunks.is_empty());
}

#[test]
fn rename_with_changes_keeps_hunks() {
    let repo = TestRepo::new();
    repo.write("old.txt", &numbered_lines(20));
    repo.commit_all("initial");
    repo.git(&["mv", "old.txt", "new.txt"]);
    let mut content = String::from_utf8(numbered_lines(20)).unwrap();
    content = content.replace("line1\n", "CHANGED\n");
    repo.write("new.txt", content.as_bytes());
    repo.git(&["add", "-A"]);

    let diff = repo.staged_diff();
    let renamed = file(&diff, "new.txt");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path, Some(PathBuf::from("old.txt")));
    assert_eq!(renamed.hunks.len(), 1);
}

#[test]
fn no_newline_at_end_of_file() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\ntwo");
    repo.commit_all("initial");
    repo.write("f.txt", b"one\nTWO");

    let diff = repo.working_tree_diff();
    let hunk = &file(&diff, "f.txt").hunks[0];
    let kinds = hunk.lines.iter().map(|line| line.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            DiffLineKind::Context,
            DiffLineKind::Deletion,
            DiffLineKind::NoNewlineMarker,
            DiffLineKind::Addition,
            DiffLineKind::NoNewlineMarker
        ]
    );
}

#[test]
fn mode_change_only_has_no_hunks() {
    let repo = TestRepo::new();
    repo.write("script.sh", b"#!/bin/sh\n");
    repo.commit_all("initial");

    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(repo.root().join("script.sh"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(repo.root().join("script.sh"), permissions).unwrap();

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "script.sh");
    assert_eq!(changed.status, FileStatus::ModeChanged);
    assert!(changed.hunks.is_empty());
    assert_eq!(changed.old_path, Some(PathBuf::from("script.sh")));
}

#[test]
fn binary_diffs_have_no_hunks() {
    let repo = TestRepo::new();
    repo.write("blob.bin", b"\x00\x01\x02\x03");
    repo.commit_all("initial");

    // Worktree change: binary, no hunks.
    repo.write("blob.bin", b"\x00\x01\x02\x03\xff");
    let worktree = repo.working_tree_diff();
    let changed = file(&worktree, "blob.bin");
    assert!(changed.binary);
    assert!(changed.hunks.is_empty());

    // Staged addition: binary, paths still resolved.
    repo.write("binary file with spaces.bin", b"\x00\x01");
    repo.git(&["add", "-A"]);
    let staged = repo.staged_diff();
    let added = file(&staged, "binary file with spaces.bin");
    assert!(added.binary);
    assert_eq!(added.status, FileStatus::Added);
    assert!(added.hunks.is_empty());
}

#[test]
fn weird_pathnames_are_parsed_untouched() {
    let repo = TestRepo::new();
    repo.write("my file with spaces.txt", b"a\n");
    repo.write(
        PathBuf::from(OsString::from_vec(b"weird\nname.txt".to_vec())),
        b"b\n",
    );
    repo.write("café ☃.txt", b"c\n");
    repo.commit_all("initial");

    repo.write("my file with spaces.txt", b"a\nchanged\n");
    repo.write(
        PathBuf::from(OsString::from_vec(b"weird\nname.txt".to_vec())),
        b"b\nchanged\n",
    );
    repo.write("café ☃.txt", b"c\nchanged\n");

    let diff = repo.working_tree_diff();
    assert_eq!(diff.files.len(), 3);
    assert!(file(&diff, "my file with spaces.txt").hunks.len() == 1);
    assert!(file(&diff, "café ☃.txt").hunks.len() == 1);
    let weird = PathBuf::from(OsString::from_vec(b"weird\nname.txt".to_vec()));
    assert!(
        diff.files
            .iter()
            .any(|file| file.new_path.as_deref() == Some(weird.as_path()))
    );
}

#[test]
fn staged_and_unstaged_diffs_stay_separate() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"one\ntwo\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", b"one\ntwo\nthree\n");

    let staged = repo.staged_diff();
    let staged_hunk = &file(&staged, "f.txt").hunks[0];
    let addition = staged_hunk
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Addition)
        .expect("one addition");
    assert_eq!(addition.content, b"two");

    let worktree = repo.working_tree_diff();
    let worktree_hunk = &file(&worktree, "f.txt").hunks[0];
    let addition = worktree_hunk
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Addition)
        .expect("one addition");
    assert_eq!(addition.content, b"three");
}

#[test]
fn single_hunk_patch_matches_the_original_git_output() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"a\nB\nc\n");

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.txt");
    let patch = single_hunk_patch(changed, &changed.hunks[0]);

    let original = repo.git_bytes(&["diff", "--no-color"]);
    assert_eq!(patch, original);
}

#[test]
fn single_hunk_patch_round_trips_a_no_newline_file() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\ntwo");
    repo.commit_all("initial");
    repo.write("f.txt", b"one\nTWO");

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.txt");
    let patch = single_hunk_patch(changed, &changed.hunks[0]);
    assert_eq!(patch, repo.git_bytes(&["diff", "--no-color"]));
}

#[test]
fn single_hunk_patch_applies_with_git_apply() {
    let repo = TestRepo::new();
    repo.write("multi.txt", &numbered_lines(30));
    repo.commit_all("initial");
    let mut content = String::from_utf8(numbered_lines(30)).unwrap();
    content = content
        .replace("line3\n", "CHANGED3\n")
        .replace("line15\n", "CHANGED15\n")
        .replace("line27\n", "CHANGED27\n");
    repo.write("multi.txt", content.as_bytes());

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "multi.txt");
    assert_eq!(changed.hunks.len(), 3);

    // Stage only the middle hunk.
    let patch = single_hunk_patch(changed, &changed.hunks[1]);
    let (checked, _, stderr) = repo.git_with_stdin(&["apply", "--cached", "--check", "-"], &patch);
    assert!(checked, "git apply --cached --check failed: {stderr}");
    let (applied, _, stderr) = repo.git_with_stdin(&["apply", "--cached", "-"], &patch);
    assert!(applied, "git apply --cached failed: {stderr}");

    // Git itself confirms exactly that hunk is staged.
    let staged = repo.staged_diff();
    let staged_file = file(&staged, "multi.txt");
    assert_eq!(staged_file.hunks.len(), 1);
    assert_eq!(staged_file.hunks[0].old_start, 12);

    // The other two hunks are still unstaged in the working tree.
    let worktree = repo.working_tree_diff();
    let worktree_file = file(&worktree, "multi.txt");
    assert_eq!(worktree_file.hunks.len(), 2);
    assert_eq!(worktree_file.hunks[0].old_start, 1);
    assert_eq!(worktree_file.hunks[1].old_start, 24);
}

#[test]
fn single_hunk_patches_of_multiple_files_apply_independently() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"one\ntwo\nthree\n");
    repo.write("b.txt", b"one\ntwo\nthree\n");
    repo.commit_all("initial");
    repo.write("a.txt", b"one\nTWO\nthree\n");
    repo.write("b.txt", b"one\nTWO\nthree\n");

    let diff = repo.working_tree_diff();
    assert_eq!(diff.files.len(), 2);
    for changed in &diff.files {
        let patch = single_hunk_patch(changed, &changed.hunks[0]);
        let (checked, _, stderr) =
            repo.git_with_stdin(&["apply", "--cached", "--check", "-"], &patch);
        assert!(checked, "git apply --cached --check failed: {stderr}");
    }

    let b = file(&diff, "b.txt");
    let (applied, _, stderr) = repo.git_with_stdin(
        &["apply", "--cached", "-"],
        &single_hunk_patch(b, &b.hunks[0]),
    );
    assert!(applied, "git apply --cached failed: {stderr}");

    let staged = repo.staged_diff();
    assert_eq!(staged.files.len(), 1);
    assert_eq!(staged.files[0].path(), Some(Path::new("b.txt")));
}

#[test]
fn single_hunk_patch_of_an_added_file_applies() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"base\n");
    repo.commit_all("initial");
    repo.write("new.txt", b"x\ny\n");
    repo.git(&["add", "new.txt"]);

    let staged = repo.staged_diff();
    let added = file(&staged, "new.txt");
    // Unstage the added file by reverse-applying its only hunk to the index.
    let patch = single_hunk_patch(added, &added.hunks[0]);
    let (checked, _, stderr) =
        repo.git_with_stdin(&["apply", "--cached", "--check", "--reverse", "-"], &patch);
    assert!(
        checked,
        "git apply --cached --reverse --check failed: {stderr}"
    );
    let (applied, _, stderr) =
        repo.git_with_stdin(&["apply", "--cached", "--reverse", "-"], &patch);
    assert!(applied, "git apply --cached --reverse failed: {stderr}");
    assert!(repo.staged_diff().is_empty());
}
