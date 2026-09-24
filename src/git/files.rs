//! `git ls-files` for the Files search scope
//! (GITILANTE_SEARCH_SPEC.md sections 13 and 54).
//!
//! The listing covers tracked files plus untracked, non-ignored files and is
//! NUL-delimited, so paths with spaces, tabs or newlines survive intact.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::git::Result;
use crate::git::command;
use crate::git::repository::Repository;

/// Lists the working tree files relative to the repository root.
pub fn working_tree_files(repo: &Repository) -> Result<Vec<PathBuf>> {
    let output = command::run(
        repo.root(),
        &["ls-files", "-co", "--exclude-standard", "-z"],
    )?;
    Ok(parse(&output.stdout))
}

/// Parses the NUL-delimited file list (GITILANTE_SEARCH_SPEC.md section 54).
pub fn parse(input: &[u8]) -> Vec<PathBuf> {
    input
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| PathBuf::from(OsString::from_vec(entry.to_vec())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_delimited_paths() {
        // GITILANTE_SEARCH_SPEC.md section 54: spaces, tabs and newlines in
        // file names must survive.
        let input = b"src/app.rs\x00with space.txt\x00tab\ttab.txt\x00new\nline.txt\x00citta\0\x00";
        let files = parse(input);
        assert_eq!(
            files,
            vec![
                PathBuf::from("src/app.rs"),
                PathBuf::from("with space.txt"),
                PathBuf::from("tab\ttab.txt"),
                PathBuf::from("new\nline.txt"),
                PathBuf::from("citta"),
            ]
        );
    }

    #[test]
    fn empty_input_is_an_empty_list() {
        assert!(parse(b"").is_empty());
    }
}
