//! Split synthetic hunks reuse the ordinary Git patch backend.

mod common;

use common::{TestRepo, file};
use gitilante::hunk_split::split;
use gitilante::model::diff::DiffLineKind;

fn baseline() -> Vec<u8> {
    (1..=16)
        .map(|line| format!("line{line}\n"))
        .collect::<String>()
        .into_bytes()
}

fn two_changes() -> Vec<u8> {
    String::from_utf8(baseline())
        .unwrap()
        .replace("line4\n", "change_A\n")
        .replace("line8\n", "change_B\n")
        .into_bytes()
}

fn changed_file(repo: &TestRepo, staged: bool) -> gitilante::model::diff::Diff {
    let diff = if staged {
        repo.staged_diff()
    } else {
        repo.working_tree_diff()
    };
    assert_eq!(diff.files.len(), 1);
    assert_eq!(
        diff.files[0].hunks.len(),
        1,
        "fixture must have one raw Git hunk"
    );
    diff
}

fn has_changed_line(diff: &str, content: &str) -> bool {
    diff.lines()
        .any(|line| (line.starts_with('+') || line.starts_with('-')) && line[1..].contains(content))
}

#[test]
fn stage_only_one_synthetic_child() {
    let repo = TestRepo::new();
    repo.write("file.rs", &baseline());
    repo.commit_all("base");
    repo.write("file.rs", &two_changes());

    let diff = changed_file(&repo, false);
    let original = file(&diff, "file.rs");
    let parts = split(original, &original.hunks[0]).expect("two change blocks split");
    assert_eq!(parts.len(), 2);
    repo.repository()
        .stage_hunk(original, &parts[0].hunk)
        .expect("stage first child");

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(has_changed_line(&staged, "change_A"));
    assert!(!has_changed_line(&staged, "change_B"));
    assert!(has_changed_line(&unstaged, "change_B"));
    assert!(!has_changed_line(&unstaged, "change_A"));
    assert_eq!(repo.read("file.rs"), two_changes());
}

#[test]
fn unstage_only_one_synthetic_child() {
    let repo = TestRepo::new();
    repo.write("file.rs", &baseline());
    repo.commit_all("base");
    repo.write("file.rs", &two_changes());
    repo.git(&["add", "file.rs"]);

    let diff = changed_file(&repo, true);
    let original = file(&diff, "file.rs");
    let parts = split(original, &original.hunks[0]).expect("two change blocks split");
    repo.repository()
        .unstage_hunk(original, &parts[1].hunk)
        .expect("unstage second child");

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(has_changed_line(&staged, "change_A"));
    assert!(!has_changed_line(&staged, "change_B"));
    assert!(has_changed_line(&unstaged, "change_B"));
    assert!(!has_changed_line(&unstaged, "change_A"));
}

#[test]
fn discard_only_one_synthetic_child() {
    let repo = TestRepo::new();
    let base = baseline();
    repo.write("file.rs", &base);
    repo.commit_all("base");
    repo.write("file.rs", &two_changes());

    let diff = changed_file(&repo, false);
    let original = file(&diff, "file.rs");
    let parts = split(original, &original.hunks[0]).expect("two change blocks split");
    repo.repository()
        .discard_hunk(original, &parts[1].hunk)
        .expect("discard second child");

    let expected = String::from_utf8(base)
        .unwrap()
        .replace("line4\n", "change_A\n")
        .into_bytes();
    assert_eq!(repo.read("file.rs"), expected);
}

#[test]
fn revert_only_one_historical_synthetic_child() {
    let repo = TestRepo::new();
    let base = baseline();
    repo.write("file.rs", &base);
    repo.commit_all("base");
    repo.write("file.rs", &two_changes());
    repo.commit_all("two changes");

    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let diff = repo.repository().commit_diff(&oid).expect("commit diff");
    let historical = file(&diff, "file.rs");
    let parts = split(historical, &historical.hunks[0]).expect("two change blocks split");
    repo.repository()
        .revert_commit_hunk(historical, &parts[1].hunk)
        .expect("revert second child");

    let expected = String::from_utf8(two_changes())
        .unwrap()
        .replace("change_B\n", "line8\n")
        .into_bytes();
    assert_eq!(repo.read("file.rs"), expected);
    let diff = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(!has_changed_line(&diff, "change_A"));
    assert!(has_changed_line(&diff, "change_B"));
}

#[test]
fn stale_synthetic_child_is_rejected_by_the_existing_apply_check() {
    let repo = TestRepo::new();
    repo.write("file.rs", &baseline());
    repo.commit_all("base");
    repo.write("file.rs", &two_changes());

    let diff = changed_file(&repo, false);
    let original = file(&diff, "file.rs");
    let parts = split(original, &original.hunks[0]).expect("two change blocks split");
    repo.write("file.rs", b"external change\n");
    repo.git(&["add", "file.rs"]);
    let before = repo.read("file.rs");
    let index_before = repo.git_bytes(&["diff", "--cached"]);
    assert!(
        repo.repository()
            .stage_hunk(original, &parts[0].hunk)
            .is_err()
    );
    assert_eq!(repo.read("file.rs"), before);
    assert_eq!(repo.git_bytes(&["diff", "--cached"]), index_before);
}

#[test]
fn selected_lines_inside_a_child_use_child_line_indexes() {
    let repo = TestRepo::new();
    repo.write("file.rs", &baseline());
    repo.commit_all("base");
    let modified = String::from_utf8(baseline())
        .unwrap()
        .replace("line4\n", "replacement_one\nreplacement_two\n")
        .replace("line8\n", "change_B\n")
        .into_bytes();
    repo.write("file.rs", &modified);

    let diff = changed_file(&repo, false);
    let original = file(&diff, "file.rs");
    let parts = split(original, &original.hunks[0]).expect("two change blocks split");
    let first = &parts[0].hunk;
    let selected = first
        .lines
        .iter()
        .position(|line| line.kind == DiffLineKind::Addition && line.content == b"replacement_one")
        .expect("selected addition");
    repo.repository()
        .stage_lines(original, first, &[selected])
        .expect("stage selected line in child");

    let staged = String::from_utf8(repo.git_bytes(&["diff", "--cached"])).unwrap();
    let unstaged = String::from_utf8(repo.git_bytes(&["diff"])).unwrap();
    assert!(has_changed_line(&staged, "replacement_one"));
    assert!(!has_changed_line(&staged, "replacement_two"));
    assert!(!has_changed_line(&staged, "change_B"));
    assert!(has_changed_line(&unstaged, "replacement_two"));
    assert!(has_changed_line(&unstaged, "change_B"));
}
