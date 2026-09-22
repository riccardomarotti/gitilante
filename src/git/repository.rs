//! Repository discovery and high-level operations.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::git::Result;
use crate::git::command;
use crate::git::error::Error;
use crate::model::status::Status;

/// A Git repository, rooted at its working tree root.
///
/// All operations run `git -C <root>`; the process working directory is never
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    root: PathBuf,
}

impl Repository {
    /// Opens the repository containing `path`.
    ///
    /// `path` may be the repository root or any subdirectory of it; the
    /// repository root is resolved with `git rev-parse --show-toplevel`.
    pub fn discover(path: &Path) -> Result<Self> {
        let args = [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")];
        let output = match command::run(path, &args) {
            Ok(output) => output,
            // Any `rev-parse` failure here means the path is not inside a work tree.
            Err(Error::Git(_)) => {
                return Err(Error::NotARepository {
                    path: path.to_path_buf(),
                });
            }
            Err(error) => return Err(error),
        };

        // `--show-toplevel` prints the root followed by a newline (or NUL with -z);
        // strip trailing separators to get the exact pathname.
        let root_bytes: Vec<u8> = output
            .stdout
            .iter()
            .copied()
            .take_while(|byte| *byte != 0 && *byte != b'\n')
            .collect();
        if root_bytes.is_empty() {
            return Err(Error::NotARepository {
                path: path.to_path_buf(),
            });
        }

        Ok(Self {
            root: PathBuf::from(OsString::from_vec(root_bytes)),
        })
    }

    /// Working tree root of the repository.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the current status of the working tree and index.
    pub fn status(&self) -> Result<Status> {
        crate::git::status::status(self)
    }
}
