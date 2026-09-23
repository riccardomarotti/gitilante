//! Intraline analysis integration tests on real Git repositories.
//!
//! DIFF.md section 42: besides the pure function tests, run the whole flow
//! (create repository, commit, modify, parse, analyze) on real diffs of
//! different file kinds. The span metadata must never influence patch
//! reconstruction (DIFF.md sections 2 and 29).

mod common;

use common::{TestRepo, file};
use gitilante::git::patch::single_hunk_patch;
use gitilante::model::diff::{Diff, DiffLine, DiffLineKind};

/// Changed texts of one line, for readable expectations.
fn changed(line: &DiffLine) -> Vec<String> {
    let text = line.text();
    line.intraline
        .iter()
        .map(|span| {
            text.chars()
                .skip(span.start)
                .take(span.end - span.start)
                .collect()
        })
        .collect()
}

/// Changed texts of the first removed/added pair of `name`'s first hunk.
fn changed_pair(diff: &Diff, name: &str) -> (Vec<String>, Vec<String>) {
    let hunk = &file(diff, name).hunks[0];
    let removed = hunk
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Deletion)
        .expect("a removed line");
    let added = hunk
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Addition)
        .expect("an added line");
    (changed(removed), changed(added))
}

#[test]
fn rust_source_intraline() {
    let repo = TestRepo::new();
    repo.write(
        "f.rs",
        b"fn main() {\n    let timeout = Duration::from_secs(10);\n}\n",
    );
    repo.commit_all("initial");
    repo.write(
        "f.rs",
        b"fn main() {\n    let timeout = Duration::from_secs(30);\n}\n",
    );

    assert_eq!(
        changed_pair(&repo.working_tree_diff(), "f.rs"),
        (vec!["10".to_owned()], vec!["30".to_owned()])
    );
}

#[test]
fn json_intraline() {
    let repo = TestRepo::new();
    repo.write(
        "config.json",
        b"{\n  \"timeout\": 10,\n  \"name\": \"demo\"\n}\n",
    );
    repo.commit_all("initial");
    repo.write(
        "config.json",
        b"{\n  \"timeout\": 30,\n  \"name\": \"demo\"\n}\n",
    );

    assert_eq!(
        changed_pair(&repo.working_tree_diff(), "config.json"),
        (vec!["10".to_owned()], vec!["30".to_owned()])
    );
}

#[test]
fn markdown_intraline() {
    let repo = TestRepo::new();
    repo.write("README.md", b"# Title\n\n## Status: old\n");
    repo.commit_all("initial");
    repo.write("README.md", b"# Title\n\n## Status: new\n");

    assert_eq!(
        changed_pair(&repo.working_tree_diff(), "README.md"),
        (vec!["old".to_owned()], vec!["new".to_owned()])
    );
}

#[test]
fn plain_text_intraline() {
    let repo = TestRepo::new();
    repo.write("notes.txt", b"hello world\nsecond line\n");
    repo.commit_all("initial");
    repo.write("notes.txt", b"hello there\nsecond line\n");

    assert_eq!(
        changed_pair(&repo.working_tree_diff(), "notes.txt"),
        (vec!["world".to_owned()], vec!["there".to_owned()])
    );
}

#[test]
fn unpaired_added_lines_stay_plain() {
    let repo = TestRepo::new();
    repo.write("f.txt", b"alpha one\nbeta two\n");
    repo.commit_all("initial");
    repo.write("f.txt", b"alpha one CHANGED\nfresh line\nbeta two\n");

    let diff = repo.working_tree_diff();
    let hunk = &file(&diff, "f.txt").hunks[0];
    for line in &hunk.lines {
        if line.kind == DiffLineKind::Addition {
            let text = line.text().into_owned();
            if text == "fresh line" {
                assert!(
                    line.intraline.is_empty(),
                    "a wholly new line must stay plain"
                );
            } else {
                // The inserted part is literally " CHANGED" (leading space
                // included), like the "_async" of DIFF.md section 23.
                assert_eq!(changed(line), [" CHANGED"]);
            }
        }
    }
}

#[test]
fn patches_ignore_intraline_spans() {
    // DIFF.md sections 2 and 29: the analyzed model must rebuild exactly the
    // same patch as the raw Git output.
    let repo = TestRepo::new();
    repo.write(
        "f.rs",
        b"fn main() {\n    let timeout = Duration::from_secs(10);\n}\n",
    );
    repo.commit_all("initial");
    repo.write(
        "f.rs",
        b"fn main() {\n    let timeout = Duration::from_secs(30);\n}\n",
    );

    let diff = repo.working_tree_diff();
    let changed = file(&diff, "f.rs");
    let removed = changed.hunks[0]
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Deletion)
        .expect("a removed line");
    assert!(!removed.intraline.is_empty()); // spans are there
    assert_eq!(
        single_hunk_patch(changed, &changed.hunks[0]),
        repo.git_bytes(&["diff", "--no-color"])
    );
}
