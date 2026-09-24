//! Real-repository tests for the commit graph (BRANCH.md sections 59-63).
//!
//! Every test builds a real Git history and compares the commit parents, the
//! graph layout and the ref mapping against Git itself.

mod common;

use common::TestRepo;
use gitilante::graph::{GraphNodeKind, GraphPoint, layout_commits};
use gitilante::model::refs::RefKind;

#[test]
fn graph_of_a_real_branch_and_merge_history() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"one\n");
    repo.commit_all("root");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("file.txt", b"one\ntwo\n");
    repo.commit_all("feature commit");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("other.txt", b"x\n");
    repo.commit_all("main commit");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);

    let backend = repo.repository();
    let commits = backend.history(0, 100).expect("history");
    assert_eq!(commits.len(), 4);

    // The layout covers exactly the loaded commits.
    let graph = layout_commits(&commits);
    assert_eq!(graph.rows.len(), commits.len());
    for (row, commit) in graph.rows.iter().zip(&commits) {
        assert_eq!(row.oid, commit.oid);
        assert!(row.node.lane < row.lane_count);
    }

    // The merge commit keeps both ancestry lanes open below it.
    let merge_index = commits
        .iter()
        .position(|commit| commit.subject == "merge feature")
        .expect("merge commit");
    let merge = &graph.rows[merge_index];
    assert_eq!(merge.node.kind, GraphNodeKind::Merge);
    let outgoing: Vec<usize> = merge
        .edges
        .iter()
        .filter_map(|edge| match (edge.from, edge.to) {
            (GraphPoint::Node, GraphPoint::Bottom(bottom)) => Some(bottom),
            _ => None,
        })
        .collect();
    assert_eq!(outgoing.len(), 2);
    assert_ne!(outgoing[0], outgoing[1]);

    // The root commit terminates its lane: no edge towards a parent.
    let root_index = commits
        .iter()
        .position(|commit| commit.subject == "root")
        .expect("root commit");
    assert!(
        !graph.rows[root_index]
            .edges
            .iter()
            .any(|edge| matches!(edge.to, GraphPoint::Bottom(_)))
    );
}

#[test]
fn history_includes_unmerged_branches() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"1\n");
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("b.txt", b"2\n");
    repo.commit_all("feature commit");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("c.txt", b"3\n");
    repo.commit_all("main commit");

    // The feature branch is not merged into main and must still be listed
    // (BRANCH.md sections 4 and 75-D).
    let commits = repo.repository().history(0, 100).expect("history");
    let subjects: Vec<&str> = commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects.len(), 3);
    assert!(subjects.contains(&"feature commit"));
    assert!(subjects.contains(&"main commit"));

    // Both tips open their own lane while both lines are still open.
    let graph = layout_commits(&commits);
    assert!(graph.rows.iter().any(|row| row.lane_count >= 2));
}

#[test]
fn refs_map_to_their_commits() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"1\n");
    repo.commit_all("first");
    let first_oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    // Two refs on the same commit (BRANCH.md section 60).
    repo.git(&["branch", "feature"]);
    repo.write("a.txt", b"2\n");
    repo.commit_all("second");
    let second_oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    // A remote branch without a real remote, and its symbolic HEAD.
    repo.git(&["update-ref", "refs/remotes/origin/main", &second_oid]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let refs = repo.repository().commit_refs().expect("refs");

    let first: Vec<(&str, RefKind)> = refs[&first_oid]
        .iter()
        .map(|commit_ref| (commit_ref.name.as_str(), commit_ref.kind))
        .collect();
    assert_eq!(first, vec![("feature", RefKind::LocalBranch)]);

    let second: Vec<(&str, RefKind)> = refs[&second_oid]
        .iter()
        .map(|commit_ref| (commit_ref.name.as_str(), commit_ref.kind))
        .collect();
    assert_eq!(
        second,
        vec![
            ("main", RefKind::LocalBranch),
            ("origin/main", RefKind::RemoteBranch),
        ]
    );

    // Symbolic remote HEADs add no information (BRANCH.md section 12).
    assert!(
        refs.values()
            .flatten()
            .all(|commit_ref| commit_ref.name != "origin/HEAD")
    );
}

#[test]
fn detached_head_stays_in_the_graph() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"1\n");
    repo.commit_all("first");
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);

    let backend = repo.repository();
    let head = backend.head().expect("head").expect("detached head");
    assert_eq!(head.oid, oid);
    assert_eq!(head.branch, None);

    // The detached commit is listed even without any branch (BRANCH.md section 8).
    let commits = backend.history(0, 10).expect("history");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].oid, oid);

    let graph = layout_commits(&commits);
    assert_eq!(graph.rows.len(), 1);
}

#[test]
fn attached_head_reports_its_branch() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"1\n");
    repo.commit_all("first");

    let head = repo.repository().head().expect("head").expect("head");
    assert_eq!(head.branch.as_deref(), Some("main"));
}

#[test]
fn unborn_repository_has_no_history() {
    let repo = TestRepo::new();
    let backend = repo.repository();
    assert!(backend.history(0, 10).expect("history").is_empty());
    assert!(backend.head().expect("head").is_none());
    assert!(backend.commit_refs().expect("refs").is_empty());
}

#[test]
fn pagination_keeps_the_layout_of_loaded_commits() {
    let repo = TestRepo::new();
    for number in 1..=5 {
        repo.write(format!("f{number}.txt"), b"x\n");
        repo.commit_all(&format!("commit {number}"));
    }
    // A side branch from the second commit, kept open past the page boundary.
    repo.git(&["checkout", "-q", "-b", "feature", "HEAD~3"]);
    repo.write("side.txt", b"side\n");
    repo.commit_all("side commit");

    let backend = repo.repository();
    let all = backend.history(0, 100).expect("all commits");
    let first_page = backend.history(0, 3).expect("first page");
    let rest = backend.history(3, 100).expect("second page");

    // The pages are a stable partition of the whole history (BRANCH.md section 59).
    let mut joined = first_page.clone();
    joined.extend(rest);
    assert_eq!(joined, all);

    // Laying out more commits does not move the rows already laid out
    // (BRANCH.md section 48).
    let full_graph = layout_commits(&all);
    let page_graph = layout_commits(&first_page);
    assert_eq!(page_graph.rows, full_graph.rows[..first_page.len()]);
}
