mod common;

use common::TestRepo;
use gitilante::git::history::FileHistoryEntry;
use std::path::Path;

#[test]
fn file_history_follows_renames_and_paginates() {
    let repo = TestRepo::new();
    repo.write("old name.txt", b"first\n");
    repo.commit_all("create");
    repo.write("old name.txt", b"second\n");
    repo.commit_all("edit old");
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    repo.commit_all("rename");
    repo.write("new name.txt", b"third\n");
    repo.commit_all("edit new");
    let backend = repo.repository();
    let page = backend
        .file_history(Path::new("new name.txt"), 0, 2)
        .unwrap();
    let next = backend
        .file_history(Path::new("new name.txt"), 2, 2)
        .unwrap();
    let entries: Vec<FileHistoryEntry> = page.into_iter().chain(next).collect();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.commit.subject.as_str())
            .collect::<Vec<_>>(),
        ["edit new", "rename", "edit old", "create"]
    );
    assert_eq!(
        entries[1].paths,
        [Path::new("old name.txt"), Path::new("new name.txt")]
    );
    assert!(
        backend
            .file_history(Path::new("new name.txt"), 4, 2)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn file_history_excludes_other_files_and_handles_weird_paths() {
    for path in ["-leading.txt", "città.txt", "tab\tname.txt"] {
        let repo = TestRepo::new();
        repo.write(path, b"first\n");
        repo.commit_all("create target");
        repo.write("other.txt", b"unrelated\n");
        repo.commit_all("other commit");
        repo.write(path, b"second\n");
        repo.commit_all("edit target");
        let entries = repo
            .repository()
            .file_history(Path::new(path), 0, 10)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].paths, [Path::new(path)]);
        assert_eq!(entries[1].commit.subject, "create target");
    }
}

#[test]
fn rename_entry_paths_match_only_its_file_diff() {
    let repo = TestRepo::new();
    repo.write("before.txt", b"first\nsecond\n");
    repo.write("other.txt", b"unrelated\n");
    repo.commit_all("initial");
    repo.git(&["mv", "before.txt", "after.txt"]);
    repo.write("other.txt", b"another change\n");
    repo.commit_all("rename plus other file");
    let backend = repo.repository();
    let entries = backend.file_history(Path::new("after.txt"), 0, 10).unwrap();
    assert_eq!(
        entries[0].paths,
        [Path::new("before.txt"), Path::new("after.txt")]
    );
    let diff = backend.commit_diff(&entries[0].commit.oid).unwrap();
    let matching: Vec<_> = diff
        .files
        .iter()
        .filter(|file| {
            entries[0].paths.iter().any(|path| {
                file.old_path.as_ref() == Some(path) || file.new_path.as_ref() == Some(path)
            })
        })
        .collect();
    assert_eq!(diff.files.len(), 2);
    assert_eq!(matching.len(), 1);
    assert_eq!(
        matching[0].old_path.as_deref(),
        Some(Path::new("before.txt"))
    );
    assert_eq!(
        matching[0].new_path.as_deref(),
        Some(Path::new("after.txt"))
    );
}

#[test]
fn non_utf8_path_is_preserved_as_raw_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"raw-\xff.txt".to_vec()));
    let repo = TestRepo::new();
    repo.write(&path, b"first\n");
    repo.commit_all("initial");
    let entries = repo.repository().file_history(&path, 0, 10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].paths, [path]);
}

#[test]
fn file_history_follows_multiple_renames() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"hello\n");
    repo.commit_all("create a");
    repo.git(&["mv", "a.txt", "b.txt"]);
    repo.commit_all("rename to b");
    repo.git(&["mv", "b.txt", "c.txt"]);
    repo.commit_all("rename to c");
    let entries = repo
        .repository()
        .file_history(Path::new("c.txt"), 0, 10)
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.commit.subject.as_str())
            .collect::<Vec<_>>(),
        ["rename to c", "rename to b", "create a"]
    );
    assert_eq!(entries[0].paths, [Path::new("b.txt"), Path::new("c.txt")]);
    assert_eq!(entries[1].paths, [Path::new("a.txt"), Path::new("b.txt")]);
}
