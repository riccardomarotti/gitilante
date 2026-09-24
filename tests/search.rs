//! Integration tests for the Contents search on real repositories
//! (GITILANTE_SEARCH_SPEC.md sections 63-65).

mod common;

use common::TestRepo;
use gitilante::search::contents;
use gitilante::search::query::SearchQuery;
use gitilante::search::result::SearchResult;

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        ..SearchQuery::default()
    }
}

fn lines(results: &[SearchResult]) -> Vec<(String, usize)> {
    results
        .iter()
        .map(|result| match result {
            SearchResult::Content(content) => {
                (content.path.display().to_string(), content.line_number)
            }
            _ => panic!("expected a content result"),
        })
        .collect()
}

#[test]
fn finds_matches_in_tracked_files() {
    // GITILANTE_SEARCH_SPEC.md section 63.
    let repo = TestRepo::new();
    repo.write("a.txt", b"one needle\ntwo\nthree needle\n");
    repo.write("b.txt", b"needle here\n");
    repo.commit_all("content");

    let backend = repo.repository();
    let untracked = backend.untracked_files().expect("untracked");
    let results = contents::search(&query("needle"), &backend, &untracked);
    assert_eq!(
        lines(&results),
        vec![
            ("a.txt".to_owned(), 1),
            ("a.txt".to_owned(), 3),
            ("b.txt".to_owned(), 1),
        ]
    );
}

#[test]
fn untracked_found_and_ignored_skipped() {
    // GITILANTE_SEARCH_SPEC.md section 64.
    let repo = TestRepo::new();
    repo.write("tracked.txt", b"nothing here\n");
    repo.commit_all("base");
    repo.write("untracked.txt", b"needle untracked\n");
    repo.write(".gitignore", b"ignored.txt\n");
    repo.write("ignored.txt", b"needle ignored\n");

    let backend = repo.repository();
    let untracked = backend.untracked_files().expect("untracked");
    assert!(untracked.iter().any(|path| path.ends_with("untracked.txt")));
    assert!(!untracked.iter().any(|path| path.ends_with("ignored.txt")));

    let results = contents::search(&query("needle"), &backend, &untracked);
    assert_eq!(lines(&results), vec![("untracked.txt".to_owned(), 1)]);
}

#[test]
fn binary_files_are_silent() {
    // GITILANTE_SEARCH_SPEC.md section 65.
    let repo = TestRepo::new();
    repo.write("blob.bin", b"needle\x00needle");
    repo.write("blob2.bin", b"\x00needle");
    repo.commit_all("binary");

    let backend = repo.repository();
    let untracked = backend.untracked_files().expect("untracked");
    let results = contents::search(&query("needle"), &backend, &untracked);
    assert!(results.is_empty());
}

#[test]
fn short_queries_find_nothing() {
    // GITILANTE_SEARCH_SPEC.md section 34.
    let repo = TestRepo::new();
    repo.write("a.txt", b"n\n");
    repo.commit_all("content");
    let backend = repo.repository();
    assert!(contents::search(&query("n"), &backend, &[]).is_empty());
}

#[test]
fn finds_commits_by_subject_author_and_object_name() {
    // GITILANTE_SEARCH_SPEC.md sections 23 and 66.
    let repo = TestRepo::new();
    repo.write("a.txt", b"one\n");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-m",
        "Fix timeout handling",
        "--author=Ada Lovelace <ada@example.com>",
    ]);
    let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    let backend = repo.repository();
    let refs = backend.commit_refs().expect("refs");

    let results = gitilante::search::history::search(&query("timeout"), &backend, &refs);
    assert_eq!(results.len(), 1, "subject match: {results:#?}");

    let results = gitilante::search::history::search(&query("Ada"), &backend, &refs);
    assert_eq!(results.len(), 1, "author match: {results:#?}");

    // An object name prefix resolves to its commit (section 25).
    let results = gitilante::search::history::search(&query(&oid[..8]), &backend, &refs);
    let SearchResult::Commit(commit) = &results[0] else {
        panic!("expected a commit result");
    };
    assert_eq!(commit.oid, oid);

    // An older commit must resolve too: passing `-- <oid>` to git log would
    // interpret the object name as a path and silently return HEAD instead.
    repo.write("later.txt", b"later\n");
    repo.commit_all("Later commit");
    let results = gitilante::search::history::search(&query(&oid[..7]), &backend, &refs);
    assert_eq!(results.len(), 1, "older commit by prefix: {results:#?}");
    let SearchResult::Commit(commit) = &results[0] else {
        panic!("expected a commit result");
    };
    assert_eq!(commit.oid, oid);
}

#[test]
fn finds_commits_by_ref_name() {
    // GITILANTE_SEARCH_SPEC.md section 23.
    let repo = TestRepo::new();
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.git(&["branch", "feature/parser"]);

    let backend = repo.repository();
    let refs = backend.commit_refs().expect("refs");
    let results = gitilante::search::history::search(&query("feature"), &backend, &refs);
    assert_eq!(results.len(), 1, "ref match: {results:#?}");
    let SearchResult::Commit(commit) = &results[0] else {
        panic!("expected a commit result");
    };
    assert!(commit.refs.iter().any(|name| name == "feature/parser"));
}

#[test]
fn history_changes_follows_pickaxe_semantics() {
    // GITILANTE_SEARCH_SPEC.md section 67: -S finds the commits whose number of
    // occurrences changes.
    let repo = TestRepo::new();
    repo.write("config.rs", b"DEFAULT_TIMEOUT = 30\n");
    repo.commit_all("add default timeout");
    repo.write("other.txt", b"unrelated\n");
    repo.commit_all("touch something");
    repo.write("config.rs", b"// gone\n");
    repo.commit_all("remove default timeout");

    let backend = repo.repository();
    let results =
        gitilante::search::history_changes::search(&query("DEFAULT_TIMEOUT"), &backend).unwrap();
    let subjects: Vec<&str> = results
        .iter()
        .map(|result| match result {
            SearchResult::HistoryChange(change) => change.subject.as_str(),
            _ => panic!("expected a history change result"),
        })
        .collect();
    assert_eq!(
        subjects,
        vec!["remove default timeout", "add default timeout"]
    );
}

#[test]
fn history_changes_regex_mode_uses_log_g() {
    // GITILANTE_SEARCH_SPEC.md section 68.
    let repo = TestRepo::new();
    repo.write("v.txt", b"value = 42\n");
    repo.commit_all("add value");
    repo.write("v.txt", b"value = 43\n");
    repo.commit_all("change value");

    let backend = repo.repository();
    let regex_query = SearchQuery {
        text: "value = 4[23]".to_owned(),
        regex: true,
        ..SearchQuery::default()
    };
    let results = gitilante::search::history_changes::search(&regex_query, &backend).unwrap();
    assert_eq!(results.len(), 2, "both commits: {results:#?}");

    let invalid = SearchQuery {
        text: "[invalid(".to_owned(),
        regex: true,
        ..SearchQuery::default()
    };
    assert!(gitilante::search::history_changes::search(&invalid, &backend).is_err());
}
