//! Shared helpers for backend tests running against real `git` repositories.
//!
//! Each test builds a throwaway repository under the system temp directory,
//! configures a test identity and verifies results with Git itself.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gitilante::model::diff::{Diff, FileDiff};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Finds a file diff by path, panicking when it is missing.
pub fn file<'a>(diff: &'a Diff, name: &str) -> &'a FileDiff {
    diff.files
        .iter()
        .find(|file| file.path() == Some(Path::new(name)))
        .unwrap_or_else(|| panic!("no file diff for {name:?}, got {:#?}", diff.files))
}

/// Builds the content of a file with `count` numbered lines (`line1` ...).
pub fn numbered_lines(count: usize) -> Vec<u8> {
    (1..=count)
        .map(|number| format!("line{number}\n"))
        .collect::<String>()
        .into_bytes()
}

/// Builds numbered content with the given lines replaced.
pub fn changed_lines(count: usize, replacements: &[(&str, &str)]) -> Vec<u8> {
    let mut content = String::from_utf8(numbered_lines(count)).unwrap();
    for (from, to) in replacements {
        content = content.replace(from, to);
    }
    content.into_bytes()
}

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
        let (success, stdout, stderr) = self.git_with_stdin_raw(args, &[]);
        (
            success,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )
    }

    /// Runs `git` in the repository feeding `stdin`, reporting success, stdout
    /// and stderr as text (used for `git apply -`).
    pub fn git_with_stdin(&self, args: &[&str], stdin: &[u8]) -> (bool, String, String) {
        let (success, stdout, stderr) = self.git_with_stdin_raw(args, stdin);
        (
            success,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )
    }

    /// Runs `git` in the repository feeding `stdin`, with raw output bytes.
    pub fn git_with_stdin_raw(&self, args: &[&str], stdin: &[u8]) -> (bool, Vec<u8>, Vec<u8>) {
        use std::io::Write;
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .arg("--no-pager")
            .args(args);
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("run git");
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(stdin)
            .expect("write stdin");
        let output = child.wait_with_output().expect("wait for git");
        (output.status.success(), output.stdout, output.stderr)
    }

    /// Runs `git` in the repository and returns its raw stdout, panicking on failure.
    pub fn git_bytes(&self, args: &[&str]) -> Vec<u8> {
        let (success, stdout, _) = self.git_with_stdin_raw(args, &[]);
        assert!(success, "git {args:?} failed");
        stdout
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

    /// Reads a file from the working tree.
    pub fn read(&self, relative: impl AsRef<Path>) -> Vec<u8> {
        fs::read(self.root.join(relative)).expect("read file")
    }

    /// Creates a symbolic link pointing at `target`.
    pub fn symlink(&self, target: impl AsRef<Path>, relative: impl AsRef<Path>) {
        std::os::unix::fs::symlink(target, self.root.join(relative)).expect("create symlink");
    }

    /// Reads the current status through the Gitilante backend.
    pub fn status(&self) -> gitilante::model::status::Status {
        self.repository().status().expect("read status")
    }

    /// Opens the repository through the Gitilante backend.
    pub fn repository(&self) -> gitilante::git::Repository {
        gitilante::git::Repository::discover(self.root()).expect("discover repository")
    }

    /// Reads the working tree diff through the Gitilante backend.
    pub fn working_tree_diff(&self) -> gitilante::model::diff::Diff {
        self.repository()
            .working_tree_diff()
            .expect("read working tree diff")
    }

    /// Reads the staged diff through the Gitilante backend.
    pub fn staged_diff(&self) -> gitilante::model::diff::Diff {
        self.repository().staged_diff().expect("read staged diff")
    }

    /// Commits every current change with the given message.
    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Finds a status entry by path, panicking when it is missing.
    pub fn entry<'a>(
        &self,
        status: &'a gitilante::model::status::Status,
        name: &str,
    ) -> &'a gitilante::model::status::StatusEntry {
        status
            .entries
            .iter()
            .find(|entry| entry.path == Path::new(name))
            .unwrap_or_else(|| panic!("no status entry for {name:?}, got {:#?}", status.entries))
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
