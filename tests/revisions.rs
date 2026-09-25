mod common;

use common::{TestRepo, file};
use gitilante::git::Error;
use gitilante::model::diff::{DiffLineKind, FileStatus};
use gitilante::model::revisions::RevisionKind;
use std::path::Path;

fn oid(repo: &TestRepo, revision: &str) -> String {
    repo.git(&["rev-parse", "--verify", revision])
        .trim()
        .to_owned()
}

#[test]
fn resolves_commitish_forms_to_full_commit_oids() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"first\n");
    repo.commit_all("first");
    let first = oid(&repo, "HEAD");
    repo.git(&["branch", "feature"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &first]);
    repo.git(&["tag", "lightweight", &first]);
    repo.git(&["tag", "-a", "annotated", &first, "-m", "release"]);
    repo.write("file.txt", b"second\n");
    repo.commit_all("second");
    let second = oid(&repo, "HEAD");

    for (revision, expected) in [
        ("HEAD", second.as_str()),
        ("HEAD~1", first.as_str()),
        ("main", second.as_str()),
        ("feature", first.as_str()),
        ("origin/main", first.as_str()),
        ("lightweight", first.as_str()),
        ("annotated", first.as_str()),
        (&second[..12], second.as_str()),
        (second.as_str(), second.as_str()),
    ] {
        assert_eq!(
            repo.repository().resolve_revision(revision).unwrap(),
            expected
        );
    }
}

#[test]
fn rejects_unknown_malformed_and_non_commit_revisions() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"content\n");
    repo.commit_all("first");
    let tree = oid(&repo, "HEAD^{tree}");
    let blob = repo
        .git(&["hash-object", "-w", "file.txt"])
        .trim()
        .to_owned();

    for revision in ["unknown-revision", "HEAD^{tree}", blob.as_str(), "--help"] {
        assert!(
            matches!(
                repo.repository().resolve_revision(revision),
                Err(Error::InvalidRevision { .. })
            ),
            "expected invalid revision for {revision:?}"
        );
    }
    assert_eq!(tree.len(), 40);
}

#[test]
fn compare_direction_and_same_revision_match_git_two_commit_diff() {
    let repo = TestRepo::new();
    repo.write("value.txt", b"x = 1\n");
    repo.commit_all("first");
    let first = oid(&repo, "HEAD");
    repo.write("value.txt", b"x = 2\n");
    repo.commit_all("second");
    let second = oid(&repo, "HEAD");
    let repository = repo.repository();

    let forward = repository.compare_revisions("HEAD~1", "HEAD").unwrap();
    let forward_file = file(&forward.diff, "value.txt");
    assert!(
        forward_file.hunks[0]
            .lines
            .iter()
            .any(|line| line.content == b"x = 1")
    );
    assert!(
        forward_file.hunks[0]
            .lines
            .iter()
            .any(|line| line.content == b"x = 2")
    );
    assert_eq!(forward.from_oid, first);
    assert_eq!(forward.to_oid, second);
    assert_eq!(forward.from_input, "HEAD~1");
    assert_eq!(forward.to_input, "HEAD");

    let reverse = repository.compare_revisions("HEAD", "HEAD~1").unwrap();
    let reverse_file = file(&reverse.diff, "value.txt");
    assert!(
        reverse_file.hunks[0]
            .lines
            .iter()
            .any(|line| line.content == b"x = 2")
    );
    assert!(
        reverse_file.hunks[0]
            .lines
            .iter()
            .any(|line| line.content == b"x = 1")
    );

    let identical = repository.compare_revisions(&first, &first).unwrap();
    assert!(identical.diff.is_empty());

    repo.git(&["commit", "--allow-empty", "-q", "-m", "empty tree change"]);
    let third = oid(&repo, "HEAD");
    assert_ne!(second, third);
    assert!(
        repository
            .compare_revisions(&second, &third)
            .unwrap()
            .diff
            .is_empty()
    );
}

#[test]
fn branch_comparison_is_not_three_dot_merge_base_comparison() {
    let repo = TestRepo::new();
    repo.write("value.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["branch", "feature"]);
    repo.write("value.txt", b"main change\n");
    repo.commit_all("main change");
    repo.git(&["checkout", "-q", "feature"]);
    repo.write("value.txt", b"feature change\n");
    repo.commit_all("feature change");
    assert!(repo.repository().resolve_revision("main..feature").is_err());
    assert!(
        repo.repository()
            .resolve_revision("main...feature")
            .is_err()
    );

    let comparison = repo
        .repository()
        .compare_revisions("main", "feature")
        .unwrap();
    assert!(
        comparison
            .diff
            .files
            .iter()
            .any(|file| file.path() == Some(Path::new("value.txt")))
    );
    let file = file(&comparison.diff, "value.txt");
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Deletion && line.content == b"main change")
    );
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Addition && line.content == b"feature change")
    );
}

#[test]
fn detects_renames_binary_files_and_quoted_paths() {
    let repo = TestRepo::new();
    let source = (1..=24)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    repo.write("old.rs", source.as_bytes());
    for name in ["with space.rs", "città.rs", "tab\tname.rs", "-leading.rs"] {
        repo.write(name, b"before\n");
    }
    repo.write("image.bin", b"\0\x01\x02\x03");
    repo.commit_all("before rename");
    let from = oid(&repo, "HEAD");

    repo.git(&["mv", "old.rs", "new.rs"]);
    repo.write("new.rs", format!("{source}changed\n").as_bytes());
    for name in ["with space.rs", "città.rs", "tab\tname.rs", "-leading.rs"] {
        repo.write(name, b"after\n");
    }
    repo.write("image.bin", b"\0\x01\x02\x04");
    repo.commit_all("after rename");
    let comparison = repo.repository().compare_revisions(&from, "HEAD").unwrap();

    let renamed = comparison
        .diff
        .files
        .iter()
        .find(|file| file.new_path.as_deref() == Some(Path::new("new.rs")))
        .unwrap();
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some(Path::new("old.rs")));
    for name in ["with space.rs", "città.rs", "tab\tname.rs", "-leading.rs"] {
        assert!(
            comparison
                .diff
                .files
                .iter()
                .any(|file| file.path() == Some(Path::new(name))),
            "missing {name:?}"
        );
    }
    let binary = comparison
        .diff
        .files
        .iter()
        .find(|file| file.path() == Some(Path::new("image.bin")))
        .unwrap();
    assert!(binary.binary);
    assert!(binary.hunks.is_empty());
}

#[test]
fn revision_candidates_are_deterministic_and_do_not_change_history_refs() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"content\n");
    repo.commit_all("first");
    let head = oid(&repo, "HEAD");
    repo.git(&["branch", "zeta"]);
    repo.git(&["branch", "alpha"]);
    repo.git(&["branch", "v1.0"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &head]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repo.git(&["tag", "v1.0", &head]);
    repo.git(&["tag", "-a", "v2.0", &head, "-m", "release"]);

    let repository = repo.repository();
    let candidates = repository.revision_candidates().unwrap();
    for candidate in &candidates {
        assert_eq!(repository.resolve_revision(&candidate.spec).unwrap(), head);
    }
    assert_eq!(candidates[0].label, "HEAD");
    assert_eq!(candidates[0].kind, RevisionKind::Head);
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| (candidate.kind, candidate.label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (RevisionKind::Head, "HEAD"),
            (RevisionKind::LocalBranch, "alpha"),
            (RevisionKind::LocalBranch, "main"),
            (RevisionKind::LocalBranch, "v1.0"),
            (RevisionKind::LocalBranch, "zeta"),
            (RevisionKind::RemoteBranch, "origin/main"),
            (RevisionKind::Tag, "v1.0"),
            (RevisionKind::Tag, "v2.0"),
        ]
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.label != "origin/HEAD")
    );
    assert_eq!(
        candidates
            .iter()
            .filter(|candidate| candidate.label == "v1.0")
            .map(|candidate| candidate.spec.as_str())
            .collect::<Vec<_>>(),
        vec!["refs/heads/v1.0", "refs/tags/v1.0"]
    );
    let history_refs: Vec<_> = repo
        .repository()
        .commit_refs()
        .unwrap()
        .into_values()
        .flatten()
        .collect();
    assert_eq!(history_refs.len(), 5);
    assert!(history_refs.iter().all(|reference| matches!(
        reference.kind,
        gitilante::model::refs::RefKind::LocalBranch
            | gitilante::model::refs::RefKind::RemoteBranch
    )));
}

#[test]
fn unborn_head_is_not_a_candidate_and_is_an_inline_resolvable_error() {
    let repo = TestRepo::new();
    assert!(
        repo.repository()
            .revision_candidates()
            .unwrap()
            .iter()
            .all(|candidate| candidate.kind != RevisionKind::Head)
    );
    assert!(matches!(
        repo.repository().compare_revisions("", "HEAD"),
        Err(Error::InvalidRevision { .. })
    ));
    assert!(matches!(
        repo.repository().compare_revisions("HEAD", "HEAD"),
        Err(Error::InvalidRevision { .. })
    ));
}
