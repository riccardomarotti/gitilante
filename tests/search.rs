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
