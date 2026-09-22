//! Repository discovery tests against real Git repositories.

mod common;

use common::TestRepo;
use gitilante::git::Repository;
use gitilante::git::error::Error;

#[test]
fn discovers_the_working_tree_root() {
    let repo = TestRepo::new();
    let discovered = Repository::discover(repo.root()).expect("discover");
    assert_eq!(discovered.root(), repo.root());
}

#[test]
fn discovers_the_repository_from_a_subdirectory() {
    let repo = TestRepo::new();
    repo.write("src/deep/nested/file.rs", b"fn main() {}\n");

    let discovered = Repository::discover(&repo.root().join("src/deep/nested"))
        .expect("discover from subdirectory");
    assert_eq!(discovered.root(), repo.root());
}

#[test]
fn reports_a_clear_error_outside_any_repository() {
    let outside = std::env::temp_dir();
    // The system temp dir must not itself be inside a repository for this test.
    if Repository::discover(&outside).is_ok() {
        eprintln!("skipping: temp dir is inside a Git repository");
        return;
    }

    match Repository::discover(&outside) {
        Err(Error::NotARepository { path }) => assert_eq!(path, outside),
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[test]
fn reports_a_clear_error_for_a_missing_path() {
    let repo = TestRepo::new();
    let missing = repo.root().join("does-not-exist");
    match Repository::discover(&missing) {
        Err(Error::NotARepository { path }) => assert_eq!(path, missing),
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[test]
fn discovered_root_is_an_absolute_path() {
    let repo = TestRepo::new();
    let discovered = Repository::discover(repo.root()).expect("discover");
    assert!(discovered.root().is_absolute());
}
