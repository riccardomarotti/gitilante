//! CLI open-target resolution (GITILANTE_CLI_FILE_HISTORY_SPEC.md).
//!
//! The meaning of the command line `path` depends on what it is on the
//! filesystem (§1):
//!
//! - a real directory opens the containing repository normally;
//! - anything else that exists opens the File History of that path.
//!
//! Classification happens before [`Repository::discover`] because discovery
//! runs `git -C <directory>` and Git requires a directory there (§4). The
//! resolver is pure CLI logic: it never touches the GUI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::git::Error;
use crate::git::repository::Repository;

/// What the UI must show after its first successful refresh (§11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialView {
    /// The normal repository view.
    Repository,
    /// File History of a repository-relative path.
    FileHistory(PathBuf),
}

/// Resolves the command line `path` against the process working directory.
pub fn resolve_open_target(path: &Path) -> Result<(Repository, InitialView), Error> {
    let cwd = std::env::current_dir().map_err(|source| Error::Io {
        command: "resolve current directory".to_owned(),
        source,
    })?;
    resolve_open_target_in(&cwd, path)
}

/// Resolves `path` against an explicit working directory (§28-§35).
pub fn resolve_open_target_in(cwd: &Path, path: &Path) -> Result<(Repository, InitialView), Error> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    // `symlink_metadata` does not follow links (§5): only real directories
    // are traversed; a versioned symlink is a File History target even when
    // its final target is a directory.
    let metadata =
        fs::symlink_metadata(&absolute).map_err(|source| lookup_error(&absolute, source))?;
    if metadata.file_type().is_dir() {
        let repo = Repository::discover(&absolute)?;
        return Ok((repo, InitialView::Repository));
    }

    // Everything else that exists is a file-like path (§5, §19).
    let parent = absolute
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let repo = Repository::discover(parent)?;
    // §10: normalize the parent directory only; the final component keeps the
    // name Git sees in the working tree, even when it is a symlink.
    let real_parent = fs::canonicalize(parent).map_err(|source| lookup_error(parent, source))?;
    let name = absolute.file_name().ok_or_else(|| Error::InvalidPath {
        path: path.to_path_buf(),
    })?;
    let absolute_file = real_parent.join(name);
    // `repo.root()` is the source of truth for the repository-relative path (§9).
    let display_path =
        strip_repo_prefix(&absolute_file, repo.root()).ok_or_else(|| Error::InvalidPath {
            path: absolute_file.clone(),
        })?;
    Ok((repo, InitialView::FileHistory(display_path)))
}

/// Strips the repository root from an absolute file path (§9).
///
/// Falls back to the physical root when Git reported a root with a different
/// symlink resolution than the canonicalized parent.
fn strip_repo_prefix(file: &Path, root: &Path) -> Option<PathBuf> {
    file.strip_prefix(root)
        .ok()
        .or_else(|| {
            fs::canonicalize(root)
                .ok()
                .and_then(|real_root| file.strip_prefix(real_root).ok())
        })
        .map(Path::to_path_buf)
}

/// Distinguishes a missing path (§6) from other filesystem failures.
fn lookup_error(path: &Path, source: io::Error) -> Error {
    if source.kind() == io::ErrorKind::NotFound {
        Error::PathNotFound {
            path: path.to_path_buf(),
        }
    } else {
        Error::Io {
            command: format!("stat {}", path.display()),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Creates a temporary Git repository with the given files (§28).
    fn temp_repo(files: &[&Path]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gitilante-cli-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Test"]);
        for file in files {
            let absolute = root.join(file);
            fs::create_dir_all(absolute.parent().unwrap()).unwrap();
            fs::write(&absolute, "content\n").unwrap();
        }
        root
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed in {}", cwd.display());
    }

    /// Opens `path` and asserts the expected File History target (§29).
    fn expect_history(cwd: &Path, path: &Path, expected: &str) {
        let (repo, view) = resolve_open_target_in(cwd, path).unwrap();
        assert_eq!(
            view,
            InitialView::FileHistory(PathBuf::from(expected)),
            "input {} from {}",
            path.display(),
            cwd.display()
        );
        assert!(repo.root().is_absolute());
    }

    #[test]
    fn directories_open_the_repository_view() {
        let root = temp_repo(&[Path::new("src/foo.rs")]);
        for path in [root.clone(), root.join("src")] {
            let (repo, view) = resolve_open_target_in(&root, &path).unwrap();
            assert_eq!(view, InitialView::Repository);
            assert_eq!(repo.root(), &root);
        }
    }

    #[test]
    fn relative_file_opens_its_file_history() {
        let root = temp_repo(&[Path::new("src/foo.rs")]);
        expect_history(&root, Path::new("src/foo.rs"), "src/foo.rs");
    }

    #[test]
    fn absolute_file_opens_its_file_history() {
        let root = temp_repo(&[Path::new("src/foo.rs")]);
        expect_history(&root, &root.join("src/foo.rs"), "src/foo.rs");
    }

    #[test]
    fn root_level_file_opens_its_file_history() {
        let root = temp_repo(&[Path::new("README.md")]);
        expect_history(&root, Path::new("README.md"), "README.md");
    }

    #[test]
    fn nested_working_directory_keeps_the_full_relative_path() {
        let root = temp_repo(&[Path::new("src/ui/main_window.rs")]);
        expect_history(
            &root.join("src/ui"),
            Path::new("main_window.rs"),
            "src/ui/main_window.rs",
        );
    }

    #[test]
    fn special_paths_are_preserved_as_paths() {
        let names = ["with space.rs", "città.rs", "-leading.rs", "tab\tname.rs"];
        let files: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
        let file_refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
        let root = temp_repo(&file_refs);
        for name in names {
            expect_history(&root, Path::new(name), name);
        }
    }

    #[cfg(unix)]
    #[test]
    fn versioned_symlink_is_a_file_history_target() {
        use std::os::unix::fs::symlink;
        let root = temp_repo(&[Path::new("foo.rs")]);
        symlink("foo.rs", root.join("link.rs")).unwrap();
        git(&root, &["add", "link.rs"]);
        // The target must not change the identity of the link (§34).
        expect_history(&root, Path::new("link.rs"), "link.rs");
    }

    #[test]
    fn file_outside_any_repository_fails() {
        let dir = std::env::temp_dir().join(format!(
            "gitilante-cli-outside-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plain-file.txt");
        fs::write(&file, "content\n").unwrap();
        let error = resolve_open_target_in(&dir, Path::new("plain-file.txt")).unwrap_err();
        assert!(
            matches!(error, Error::NotARepository { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn missing_path_fails_with_a_clear_error() {
        let root = temp_repo(&[]);
        let error = resolve_open_target_in(&root, Path::new("does-not-exist.rs")).unwrap_err();
        assert!(
            matches!(error, Error::PathNotFound { .. }),
            "unexpected error: {error:?}"
        );
    }
}
