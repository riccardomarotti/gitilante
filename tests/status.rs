//! Status parsing tests against real Git repositories.
//!
//! The scenario names follow the backend test list in SPEC.md, section 32.

mod common;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use common::TestRepo;
use gitilante::model::status::{ChangeKind, Head, StatusEntry};

/// Finds an entry by path, panicking when it is missing.
fn entry<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
    entries
        .iter()
        .find(|entry| entry.path == Path::new(path))
        .unwrap_or_else(|| panic!("no status entry for {path:?}, got {entries:#?}"))
}

#[test]
fn clean_repository_has_no_entries() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");

    let status = repo.status();
    assert!(status.entries.is_empty());
    assert_eq!(status.branch.head, Head::Branch("main".to_owned()));
    assert!(status.branch.oid.is_some());
}

#[test]
fn single_modified_file() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("file.txt", b"one\ntwo\n");

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    let modified = entry(&status.entries, "file.txt");
    assert_eq!(modified.staged_change(), None);
    assert_eq!(modified.unstaged_change(), Some(ChangeKind::Modified));
}

#[test]
fn multiple_modified_files() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a\n");
    repo.write("b/c.txt", b"c\n");
    repo.write("d.txt", b"d\n");
    repo.commit_all("initial");
    repo.write("a.txt", b"a\na2\n");
    repo.write("b/c.txt", b"c\nc2\n");
    repo.remove("d.txt");

    let status = repo.status();
    assert_eq!(status.entries.len(), 3);
    assert_eq!(
        entry(&status.entries, "a.txt").unstaged_change(),
        Some(ChangeKind::Modified)
    );
    assert_eq!(
        entry(&status.entries, "b/c.txt").unstaged_change(),
        Some(ChangeKind::Modified)
    );
    assert_eq!(
        entry(&status.entries, "d.txt").unstaged_change(),
        Some(ChangeKind::Deleted)
    );
    assert_eq!(status.unstaged_entries().count(), 3);
}

#[test]
fn staged_and_unstaged_changes_on_the_same_file() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("file.txt", b"one\ntwo\n");
    repo.git(&["add", "file.txt"]);
    repo.write("file.txt", b"one\ntwo\nthree\n");

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    let both = entry(&status.entries, "file.txt");
    assert_eq!(both.staged_change(), Some(ChangeKind::Modified));
    assert_eq!(both.unstaged_change(), Some(ChangeKind::Modified));
    assert_eq!(status.staged_entries().count(), 1);
    assert_eq!(status.unstaged_entries().count(), 1);
}

#[test]
fn new_untracked_file() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("new.rs", b"fn main() {}\n");

    let status = repo.status();
    assert_eq!(status.untracked_entries().count(), 1);
    let untracked = entry(&status.entries, "new.rs");
    assert!(untracked.is_untracked());
    assert_eq!(untracked.staged_change(), None);
    assert_eq!(untracked.unstaged_change(), None);
}

#[test]
fn staged_new_file_is_added() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("new.rs", b"fn main() {}\n");
    repo.git(&["add", "new.rs"]);

    let status = repo.status();
    let added = entry(&status.entries, "new.rs");
    assert_eq!(added.staged_change(), Some(ChangeKind::Added));
    assert_eq!(added.unstaged_change(), None);
    assert_eq!(status.untracked_entries().count(), 0);
}

#[test]
fn deleted_file_in_the_worktree() {
    let repo = TestRepo::new();
    repo.write("gone.txt", b"bye\n");
    repo.commit_all("initial");
    repo.remove("gone.txt");

    let status = repo.status();
    assert_eq!(
        entry(&status.entries, "gone.txt").unstaged_change(),
        Some(ChangeKind::Deleted)
    );
}

#[test]
fn staged_deletion() {
    let repo = TestRepo::new();
    repo.write("gone.txt", b"bye\n");
    repo.commit_all("initial");
    repo.git(&["rm", "-q", "gone.txt"]);

    let status = repo.status();
    let deleted = entry(&status.entries, "gone.txt");
    assert_eq!(deleted.staged_change(), Some(ChangeKind::Deleted));
    assert_eq!(deleted.unstaged_change(), None);
}

#[test]
fn renamed_file_reports_its_original_path() {
    let repo = TestRepo::new();
    repo.write(
        "old name.txt",
        b"content that is long enough to be detected as a rename\n",
    );
    repo.commit_all("initial");
    repo.git(&["mv", "old name.txt", "new name.txt"]);

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    let renamed = entry(&status.entries, "new name.txt");
    assert_eq!(renamed.orig_path, Some(PathBuf::from("old name.txt")));
    assert_eq!(renamed.staged_change(), Some(ChangeKind::Renamed));
}

#[test]
fn filename_with_spaces() {
    let repo = TestRepo::new();
    repo.write("my file with spaces.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("my file with spaces.txt", b"one\ntwo\n");

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    assert_eq!(
        status.entries[0].path,
        PathBuf::from("my file with spaces.txt")
    );
    assert_eq!(
        status.entries[0].unstaged_change(),
        Some(ChangeKind::Modified)
    );
}

#[test]
fn filename_with_unicode() {
    let repo = TestRepo::new();
    repo.write("café ☃.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("café ☃.txt", b"one\ntwo\n");

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].path, PathBuf::from("café ☃.txt"));
}

#[test]
fn filename_with_newline_and_tab() {
    let repo = TestRepo::new();
    let weird = PathBuf::from(OsString::from_vec(b"weird\n\tname.txt".to_vec()));
    repo.write(&weird, b"one\n");
    repo.commit_all("initial");
    repo.write(&weird, b"one\ntwo\n");

    let status = repo.status();
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].path, weird);
    assert_eq!(
        status.entries[0].unstaged_change(),
        Some(ChangeKind::Modified)
    );
}

#[test]
fn binary_file() {
    let repo = TestRepo::new();
    repo.write("blob.bin", b"\x00\x01\x02\x03");
    repo.commit_all("initial");
    repo.write("blob.bin", b"\x00\x01\x02\x03\xff\xfe");

    let status = repo.status();
    assert_eq!(
        entry(&status.entries, "blob.bin").unstaged_change(),
        Some(ChangeKind::Modified)
    );
}

#[test]
fn symlink_is_reported_without_crashing() {
    let repo = TestRepo::new();
    repo.write("target.txt", b"target\n");
    repo.commit_all("initial");
    repo.symlink("target.txt", "link.txt");

    let status = repo.status();
    assert!(entry(&status.entries, "link.txt").is_untracked());
}

#[test]
fn mode_change_is_reported_as_modified() {
    let repo = TestRepo::new();
    repo.write("script.sh", b"#!/bin/sh\n");
    repo.commit_all("initial");

    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(repo.root().join("script.sh"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(repo.root().join("script.sh"), permissions).unwrap();

    let status = repo.status();
    assert_eq!(
        entry(&status.entries, "script.sh").unstaged_change(),
        Some(ChangeKind::Modified)
    );
}

#[test]
fn repository_without_commits_reports_an_unborn_branch() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");

    let status = repo.status();
    assert_eq!(status.branch.oid, None);
    assert_eq!(status.branch.head, Head::Branch("main".to_owned()));
    assert!(entry(&status.entries, "file.txt").is_untracked());
}

#[test]
fn detached_head() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.git(&["checkout", "-q", "--detach"]);
    repo.write("file.txt", b"one\ntwo\n");

    let status = repo.status();
    assert_eq!(status.branch.head, Head::Detached);
    assert!(status.branch.oid.is_some());
    assert_eq!(
        entry(&status.entries, "file.txt").unstaged_change(),
        Some(ChangeKind::Modified)
    );
}

#[test]
fn staged_addition_and_untracked_file_are_kept_apart() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("initial");
    repo.write("staged.txt", b"staged\n");
    repo.write("untracked.txt", b"untracked\n");
    repo.git(&["add", "staged.txt"]);

    let status = repo.status();
    assert_eq!(
        entry(&status.entries, "staged.txt").staged_change(),
        Some(ChangeKind::Added)
    );
    assert!(entry(&status.entries, "untracked.txt").is_untracked());
    assert_eq!(status.staged_entries().count(), 1);
    assert_eq!(status.untracked_entries().count(), 1);
}
