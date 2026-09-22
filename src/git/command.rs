//! Thin wrapper around the system `git` executable.
//!
//! Every invocation runs `git -C <root> --no-pager <args...>` without a shell,
//! captures stdout, stderr and the exit status separately, and logs command,
//! duration and output sizes at debug level.

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::git::Result;
use crate::git::error::GitError;

/// Captured output of a successful `git` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    /// Raw standard output, unparsed.
    pub stdout: Vec<u8>,
    /// Raw standard error, unparsed.
    pub stderr: Vec<u8>,
}

/// Runs `git -C <root> --no-pager <args...>`.
///
/// `root` must be the repository root (or any directory inside it). Arguments
/// are passed verbatim, without a shell, so pathnames never need quoting.
pub fn run<S: AsRef<OsStr>>(root: &Path, args: &[S]) -> Result<GitOutput> {
    run_with_stdin(root, args, None)
}

/// Like [`run`], but feeds `stdin` to the command (e.g. a patch for `git apply`).
pub fn run_with_stdin<S: AsRef<OsStr>>(
    root: &Path,
    args: &[S],
    stdin: Option<&[u8]>,
) -> Result<GitOutput> {
    let command_display = format_command(root, args);

    let mut command = Command::new("git");
    command.arg("-C").arg(root).arg("--no-pager").args(args);
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = command.spawn().map_err(|source| crate::git::Error::Io {
        command: command_display.clone(),
        source,
    })?;

    // `git apply` consumes the patch as it parses it, so a synchronous write
    // cannot fill the pipe while the child waits on us.
    if let Some(data) = stdin {
        let mut child_stdin = child.stdin.take().expect("stdin was piped");
        child_stdin
            .write_all(data)
            .map_err(|source| crate::git::Error::Io {
                command: command_display.clone(),
                source,
            })?;
        drop(child_stdin);
    }

    let output = child
        .wait_with_output()
        .map_err(|source| crate::git::Error::Io {
            command: command_display.clone(),
            source,
        })?;

    log::debug!(
        "{} | exit={:?} stdout={}B stderr={}B | {:?}",
        command_display,
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
        started.elapsed()
    );

    if !output.status.success() {
        return Err(GitError {
            command: command_display,
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned(),
        }
        .into());
    }

    Ok(GitOutput {
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Renders the full command line for logs and error details.
fn format_command<S: AsRef<OsStr>>(root: &Path, args: &[S]) -> String {
    let mut parts = vec![
        "git".to_owned(),
        "-C".to_owned(),
        root.display().to_string(),
        "--no-pager".to_owned(),
    ];
    parts.extend(
        args.iter()
            .map(|arg| arg.as_ref().to_string_lossy().into_owned()),
    );
    parts.join(" ")
}
