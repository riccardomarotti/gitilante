//! Error types shared by the Git backend.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Failure of a single `git` invocation that ran but reported an error.
///
/// The full command line is kept for diagnostics: it is meant for logs and
/// error details, not for the primary user-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    /// The failed command, including its arguments.
    pub command: String,
    /// Exit code of the process, if it terminated normally.
    pub exit_code: Option<i32>,
    /// Standard error of the process, trailing whitespace trimmed.
    pub stderr: String,
}

impl GitError {
    /// Full diagnostic description: command line, exit code and stderr.
    pub fn details(&self) -> String {
        let exit_code = self
            .exit_code
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
        format!(
            "command: {}\nexit code: {}\nstderr: {}",
            self.command, exit_code, self.stderr
        )
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Git command failed.\n\n{}", self.stderr)
    }
}

impl std::error::Error for GitError {}

/// Errors produced by the Git backend.
#[derive(Debug)]
pub enum Error {
    /// `git` ran but reported an error.
    Git(GitError),
    /// The `git` executable could not be run.
    Io { command: String, source: io::Error },
    /// The given path does not belong to a Git repository.
    NotARepository { path: PathBuf },
    /// `git` produced output that could not be parsed.
    MalformedOutput { command: String, detail: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => write!(f, "{error}"),
            Self::Io { command, source } => {
                write!(f, "Could not run Git.\n\n{command}\n\n{source}")
            }
            Self::NotARepository { path } => write!(f, "Not a Git repository: {}", path.display()),
            Self::MalformedOutput { command, detail } => {
                write!(f, "Unexpected output from `git {command}`.\n\n{detail}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<GitError> for Error {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}
