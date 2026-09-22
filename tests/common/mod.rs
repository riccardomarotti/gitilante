//! Shared helpers for backend tests running against real `git` repositories.
//!
//! Each test builds a throwaway repository under the system temp directory,
//! configures a test identity and verifies results with Git itself.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary Git repository with a hermetic configuration.
pub struct TestRepo {
    root: PathBuf,
}

impl TestRepo {
    /// Creates a temporary repository with `main` as its branch and a test
    /// identity configured. Global and system Git config are ignored so that
    /// user settings cannot influence the tests.
    pub fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("gitilante-test-{}-{}", std::process::id(), unique));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp repository directory");

        let repo = Self { root };
        repo.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        repo.git(&["config", "user.name", "Gitilante Test"]);
        repo.git(&["config", "user.email", "test@gitilante.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.git(&["config", "status.renames", "true"]);
        repo
    }

    /// Working tree root of the temporary repository.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Runs `git` in the repository and returns its stdout, panicking on failure.
    pub fn git(&self, args: &[&str]) -> String {
        let (success, stdout, stderr) = self.try_git(args);
        assert!(success, "git {args:?} failed:\n{stderr}");
        stdout
    }

    /// Runs `git` in the repository, reporting success, stdout and stderr.
    pub fn try_git(&self, args: &[&str]) -> (bool, String, String) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .arg("--no-pager")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Writes a file (creating parent directories), replacing any content.
    pub fn write(&self, relative: impl AsRef<Path>, content: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(&path, content).expect("write file");
    }

    /// Appends content to a file.
    pub fn append(&self, relative: impl AsRef<Path>, content: &[u8]) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.root.join(relative))
            .expect("open file");
        file.write_all(content).expect("append file");
    }

    /// Removes a file or symlink from the working tree.
    pub fn remove(&self, relative: impl AsRef<Path>) {
        fs::remove_file(self.root.join(relative)).expect("remove file");
    }

    /// Creates a symbolic link pointing at `target`.
    pub fn symlink(&self, target: impl AsRef<Path>, relative: impl AsRef<Path>) {
        std::os::unix::fs::symlink(target, self.root.join(relative)).expect("create symlink");
    }

    /// Reads the current status through the Gitilante backend.
    pub fn status(&self) -> gitilante::model::status::Status {
        let repo = gitilante::git::Repository::discover(self.root()).expect("discover repository");
        repo.status().expect("read status")
    }

    /// Commits every current change with the given message.
    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
