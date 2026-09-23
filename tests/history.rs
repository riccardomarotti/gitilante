//! History and commit diff tests against real Git repositories.
//!
//! Covers the history and commit diff cases (17-18) of the test list in
//! SPEC.md, section 32, plus incremental loading and edge cases (merge
//! commits, root commits, Unicode metadata, weird pathnames).

mod common;

use common::TestRepo;
use gitilante::git::patch::single_hunk_patch;

#[test]
fn history_lists_commits_with_metadata() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.commit_all("first commit");
    repo.write("f.txt", b"one\ntwo\n");
    repo.commit_all("second commit");
    repo.write("f.txt", b"one\ntwo\nthree\n");
    repo.commit_all("third commit");

    let backend = repo.repository();
    let commits = backend.history(0, 10).expect("history");

    // Newest first, verified against Git itself.
    assert_eq!(commits.len(), 3);
    let expected = repo
        .git(&["rev-list", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        commits
            .iter()
            .map(|commit| commit.oid.clone())
            .collect::<Vec<_>>(),
        expected
    );

    assert_eq!(commits[0].subject, "third commit");
    assert_eq!(commits[2].subject, "first commit");
    assert_eq!(commits[0].author_name, "Gitilante Test");
    assert_eq!(commits[0].author_email, "test@gitilante.invalid");
    assert!(commits[0].author_time > 0);
    assert_eq!(commits[0].parents, vec![expected[1].clone()]);
    assert_eq!(commits[2].parents, Vec::<String>::new());
}

#[test]
fn history_is_loaded_in_blocks() {
    let repo = TestRepo::new();
    for number in 1..=5 {
        repo.write("f.txt", format!("content {number}\n").as_bytes());
        repo.commit_all(&format!("commit {number}"));
    }

    let backend = repo.repository();
    let first = backend.history(0, 2).expect("first block");
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].subject, "commit 5");
    assert_eq!(first[1].subject, "commit 4");

    let second = backend.history(2, 2).expect("second block");
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].subject, "commit 3");
    assert_eq!(second[1].subject, "commit 2");

    let last = backend.history(4, 2).expect("last block");
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].subject, "commit 1");

    // Beyond the end of history: empty block.
    assert!(backend.history(5, 2).expect("past the end").is_empty());
}

#[test]
fn history_of_a_repository_without_commits_is_empty() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");

    let backend = repo.repository();
    assert!(backend.history(0, 10).expect("empty history").is_empty());
}

#[test]
fn history_of_detached_head() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.commit_all("first commit");
    repo.git(&["checkout", "-q", "--detach"]);

    let backend = repo.repository();
    let commits = backend.history(0, 10).expect("history");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "first commit");
}

#[test]
fn merge_commits_record_all_parents() {
    let repo = TestRepo::new();
    repo.write("base.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", b"side\n");
    repo.commit_all("side change");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("main.txt", b"main\n");
    repo.commit_all("main change");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);

    let backend = repo.repository();
    let commits = backend.history(0, 10).expect("history");
    let merge = &commits[0];
    assert_eq!(merge.subject, "merge side");
    assert_eq!(merge.parents.len(), 2);
}

#[test]
fn history_keeps_unicode_metadata() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "-c",
        "user.name=Ada Lovelace ☃",
        "-c",
        "user.email=ada@esempio.it",
        "commit",
        "-q",
        "-m",
        "soggetto ☃ con spazi",
    ]);

    let backend = repo.repository();
    let commits = backend.history(0, 10).expect("history");
    assert_eq!(commits[0].author_name, "Ada Lovelace ☃");
    assert_eq!(commits[0].author_email, "ada@esempio.it");
    assert_eq!(commits[0].subject, "soggetto ☃ con spazi");
}

#[test]
fn commit_diff_uses_the_same_diff_model() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"one\ntwo\nthree\n");
    repo.write("b.txt", b"one\ntwo\nthree\n");
    repo.commit_all("initial");
    repo.write("a.txt", b"one\nTWO\nthree\n");
    repo.write("b.txt", b"one\nTWO\nthree\n");
    repo.commit_all("changes two files");
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&oid).expect("commit diff");
    assert_eq!(diff.files.len(), 2);
    for file in &diff.files {
        assert_eq!(file.hunks.len(), 1);
        assert!(
            file.hunks[0]
                .lines
                .iter()
                .any(|line| line.content == b"TWO")
        );
    }
}

#[test]
fn commit_diff_of_a_root_commit_shows_added_files() {
    let repo = TestRepo::new();
    repo.write("new.txt", b"x\ny\n");
    repo.commit_all("initial");
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&oid).expect("commit diff");
    let file = &diff.files[0];
    assert_eq!(file.status, gitilante::model::diff::FileStatus::Added);
    assert_eq!(file.old_path, None);
    assert_eq!(
        file.new_path.as_deref(),
        Some(std::path::Path::new("new.txt"))
    );
}

#[test]
fn commit_diff_of_a_merge_commit_is_empty() {
    let repo = TestRepo::new();
    repo.write("base.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", b"side\n");
    repo.commit_all("side change");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("main.txt", b"main\n");
    repo.commit_all("main change");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    // `git show` without --m/--c shows no diff for merges.
    let backend = repo.repository();
    let diff = backend.commit_diff(&oid).expect("commit diff");
    assert!(diff.files.is_empty());
}

#[test]
fn commit_diff_patches_match_git_show_byte_for_byte() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"a\nB\nc\n");
    repo.commit_all("one change");
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&oid).expect("commit diff");
    let file = &diff.files[0];
    let patch = single_hunk_patch(file, &file.hunks[0]);
    assert_eq!(
        patch,
        repo.git_bytes(&["show", "--format=", "--patch", "--no-color", &oid])
    );
}

#[test]
fn commit_diff_handles_weird_pathnames() {
    let repo = TestRepo::new();
    repo.write("plain.txt", b"base\n");
    repo.commit_all("initial");
    repo.write("weird\n\tcaf\u{e9}.txt", b"one\ntwo\nthree\n");
    repo.commit_all("add weird file");
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let diff = backend.commit_diff(&oid).expect("commit diff");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(
        diff.files[0].new_path,
        Some(std::path::PathBuf::from("weird\n\tcaf\u{e9}.txt"))
    );
}

#[test]
fn commit_diff_of_unknown_revision_fails() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"one\n");
    repo.commit_all("initial");

    let backend = repo.repository();
    assert!(
        backend
            .commit_diff("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .is_err()
    );
}
