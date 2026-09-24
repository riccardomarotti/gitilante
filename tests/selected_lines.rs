mod common;

use common::{TestRepo, file};
use gitilante::git::Error;
use gitilante::git::patch::selected_lines_patch;
use gitilante::model::diff::{DiffLineKind, Hunk};

fn line_index(hunk: &Hunk, kind: DiffLineKind, text: &str) -> usize {
    hunk.lines
        .iter()
        .position(|line| line.kind == kind && line.content == text.as_bytes())
        .unwrap()
}

fn fixture() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("file with spaces.txt", b"one\nold A\nthree\nold B\nfive\n");
    repo.commit_all("initial");
    repo.write("file with spaces.txt", b"one\nnew A\nthree\nnew B\nfive\n");
    repo
}

#[test]
fn stage_only_one_replacement_in_one_hunk() {
    let repo = fixture();
    let diff = repo.working_tree_diff();
    let file = file(&diff, "file with spaces.txt");
    assert_eq!(file.hunks.len(), 1);
    let hunk = &file.hunks[0];
    let selected = [
        line_index(hunk, DiffLineKind::Deletion, "old B"),
        line_index(hunk, DiffLineKind::Addition, "new B"),
    ];
    repo.repository()
        .stage_lines(file, hunk, &selected)
        .unwrap();
    assert_eq!(
        repo.git_bytes(&["show", ":file with spaces.txt"]),
        b"one\nold A\nthree\nnew B\nfive\n"
    );
    assert!(
        String::from_utf8(repo.git_bytes(&["diff"]))
            .unwrap()
            .contains("new A")
    );
}

#[test]
fn unstage_and_discard_selected_lines() {
    let repo = fixture();
    repo.git(&["add", "file with spaces.txt"]);
    let diff = repo.staged_diff();
    let file = file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let selected = [
        line_index(hunk, DiffLineKind::Deletion, "old B"),
        line_index(hunk, DiffLineKind::Addition, "new B"),
    ];
    repo.repository()
        .unstage_lines(file, hunk, &selected)
        .unwrap();
    assert_eq!(
        repo.git_bytes(&["show", ":file with spaces.txt"]),
        b"one\nnew A\nthree\nold B\nfive\n"
    );
    let diff = repo.working_tree_diff();
    let file = common::file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let selected = [
        line_index(hunk, DiffLineKind::Deletion, "old B"),
        line_index(hunk, DiffLineKind::Addition, "new B"),
    ];
    repo.repository()
        .discard_lines(file, hunk, &selected)
        .unwrap();
    assert_eq!(
        repo.read("file with spaces.txt"),
        b"one\nnew A\nthree\nold B\nfive\n"
    );
}

#[test]
fn partial_replacement_and_invalid_selection() {
    let repo = fixture();
    let diff = repo.working_tree_diff();
    let file = file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let addition = line_index(hunk, DiffLineKind::Addition, "new B");
    repo.repository()
        .stage_lines(file, hunk, &[addition])
        .unwrap();
    assert_eq!(
        repo.git_bytes(&["show", ":file with spaces.txt"]),
        b"one\nold A\nthree\nold B\nnew B\nfive\n"
    );
    for invalid in [
        vec![],
        vec![hunk.lines.len()],
        vec![addition, addition],
        vec![0],
    ] {
        assert!(selected_lines_patch(file, hunk, &invalid).is_err());
    }
}

#[test]
fn stale_partial_patch_does_not_modify_index() {
    let repo = fixture();
    let diff = repo.working_tree_diff();
    let file = file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let selected = [
        line_index(hunk, DiffLineKind::Deletion, "old B"),
        line_index(hunk, DiffLineKind::Addition, "new B"),
    ];
    repo.write("file with spaces.txt", b"completely changed\n");
    repo.git(&["add", "file with spaces.txt"]);
    let before = repo.git_bytes(&["show", ":file with spaces.txt"]);
    assert!(matches!(
        repo.repository().stage_lines(file, hunk, &selected),
        Err(Error::PatchDoesNotApply { .. })
    ));
    assert_eq!(repo.git_bytes(&["show", ":file with spaces.txt"]), before);
}

#[test]
fn stage_only_deletion_and_discard_only_addition() {
    let repo = fixture();
    let diff = repo.working_tree_diff();
    let file = file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let deletion = line_index(hunk, DiffLineKind::Deletion, "old B");
    repo.repository()
        .stage_lines(file, hunk, &[deletion])
        .unwrap();
    assert_eq!(
        repo.git_bytes(&["show", ":file with spaces.txt"]),
        b"one\nold A\nthree\nfive\n"
    );

    let diff = repo.working_tree_diff();
    let file = common::file(&diff, "file with spaces.txt");
    let hunk = &file.hunks[0];
    let addition = line_index(hunk, DiffLineKind::Addition, "new B");
    repo.repository()
        .discard_lines(file, hunk, &[addition])
        .unwrap();
    assert_eq!(
        repo.read("file with spaces.txt"),
        b"one\nnew A\nthree\nfive\n"
    );
}

#[test]
fn newline_marker_and_added_file_are_rejected() {
    let repo = TestRepo::new();
    repo.write("no-newline.txt", b"old");
    repo.commit_all("initial");
    repo.write("no-newline.txt", b"new");
    let diff = repo.working_tree_diff();
    let file = file(&diff, "no-newline.txt");
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::NoNewlineMarker)
    );
    assert!(selected_lines_patch(file, &file.hunks[0], &[0]).is_err());
    repo.write("new.txt", b"added\n");
    repo.git(&["add", "new.txt"]);
    let diff = repo.staged_diff();
    let added = common::file(&diff, "new.txt");
    assert!(selected_lines_patch(added, &added.hunks[0], &[0]).is_err());
}

#[test]
fn selected_lines_support_unicode_and_dash_paths() {
    for path in ["città.txt", "-leading.txt"] {
        let repo = TestRepo::new();
        repo.write(path, "uno\nvecchio\ntré\n".as_bytes());
        repo.commit_all("initial");
        repo.write(path, "uno\nnuovo\ntré\n".as_bytes());
        let diff = repo.working_tree_diff();
        let file = file(&diff, path);
        let hunk = &file.hunks[0];
        let selected = [
            line_index(hunk, DiffLineKind::Deletion, "vecchio"),
            line_index(hunk, DiffLineKind::Addition, "nuovo"),
        ];
        repo.repository()
            .stage_lines(file, hunk, &selected)
            .unwrap();
        assert!(
            repo.staged_diff()
                .files
                .iter()
                .any(|file| file.path() == Some(std::path::Path::new(path)))
        );
    }
}
