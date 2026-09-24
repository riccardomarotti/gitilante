//! Open the current working-tree file in a configured editor or the desktop default.
//! No Git commands, shell execution, waiting, or working-directory changes.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use gtk4::prelude::*;
use gtk4::{gio, glib};

#[derive(Debug)]
pub enum OpenError {
    MissingFile,
    InvalidPath,
    InvalidEditor(String),
    Spawn(std::io::Error),
    DefaultApp(glib::Error),
}

impl OpenError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::MissingFile => "File does not exist in the working tree",
            Self::InvalidPath | Self::InvalidEditor(_) | Self::Spawn(_) => {
                "Could not open external editor"
            }
            Self::DefaultApp(_) => "Could not open this file with the default application",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditorChoice {
    Configured(Vec<OsString>),
    DesktopDefault,
}

/// A blank variable is treated as unset; a malformed nonblank value is an
/// error, not a reason to silently fall back to a different editor.
pub fn resolve_editor(
    gitilante: Option<&OsStr>,
    visual: Option<&OsStr>,
    editor: Option<&OsStr>,
) -> Result<EditorChoice, OpenError> {
    let chosen = [gitilante, visual, editor]
        .into_iter()
        .flatten()
        .find(|value| !value.to_string_lossy().trim().is_empty());
    let Some(chosen) = chosen else {
        return Ok(EditorChoice::DesktopDefault);
    };
    let argv = glib::shell_parse_argv(chosen)
        .map_err(|error| OpenError::InvalidEditor(error.to_string()))?;
    if argv.is_empty() || argv[0].is_empty() {
        return Err(OpenError::InvalidEditor("Empty editor command".to_owned()));
    }
    Ok(EditorChoice::Configured(argv))
}

pub fn absolute_path(root: &Path, relative: &Path) -> Result<PathBuf, OpenError> {
    if !root.is_absolute() || relative.is_absolute() {
        return Err(OpenError::InvalidPath);
    }
    Ok(root.join(relative))
}

/// Constructs arguments without interpreting the filename as shell syntax.
pub fn invocation(argv: &[OsString], absolute: &Path) -> (OsString, Vec<OsString>) {
    let mut args = argv[1..].to_vec();
    args.push(absolute.as_os_str().to_owned());
    (argv[0].clone(), args)
}

pub fn open(root: &Path, relative: &Path) -> Result<(), OpenError> {
    let absolute = absolute_path(root, relative)?;
    if !std::fs::metadata(&absolute).is_ok_and(|metadata| metadata.is_file()) {
        return Err(OpenError::MissingFile);
    }
    let gitilante = std::env::var_os("GITILANTE_EDITOR");
    let visual = std::env::var_os("VISUAL");
    let editor = std::env::var_os("EDITOR");
    match resolve_editor(gitilante.as_deref(), visual.as_deref(), editor.as_deref())? {
        EditorChoice::Configured(argv) => {
            let (program, args) = invocation(&argv, &absolute);
            Command::new(program)
                .args(args)
                .spawn()
                .map_err(OpenError::Spawn)?;
            Ok(())
        }
        EditorChoice::DesktopDefault => {
            let file = gio::File::for_path(&absolute);
            gio::AppInfo::launch_default_for_uri(&file.uri(), None::<&gio::AppLaunchContext>)
                .map_err(OpenError::DefaultApp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn var(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    #[test]
    fn editor_precedence_and_quoting() {
        assert_eq!(
            resolve_editor(var("gitilante --flag"), var("visual"), var("editor")).unwrap(),
            EditorChoice::Configured(vec!["gitilante".into(), "--flag".into()])
        );
        assert_eq!(
            resolve_editor(None, var("visual"), var("editor")).unwrap(),
            EditorChoice::Configured(vec!["visual".into()])
        );
        assert_eq!(
            resolve_editor(None, None, var("editor")).unwrap(),
            EditorChoice::Configured(vec!["editor".into()])
        );
        assert_eq!(
            resolve_editor(var("  "), None, None).unwrap(),
            EditorChoice::DesktopDefault
        );
        assert_eq!(
            resolve_editor(None, None, None).unwrap(),
            EditorChoice::DesktopDefault
        );
        assert_eq!(
            resolve_editor(var("my-editor \"--profile=Work Files\""), None, None).unwrap(),
            EditorChoice::Configured(vec!["my-editor".into(), "--profile=Work Files".into()])
        );
    }

    #[test]
    fn malformed_command_never_falls_back() {
        assert!(matches!(
            resolve_editor(var("'unterminated"), var("visual"), None),
            Err(OpenError::InvalidEditor(_))
        ));
    }

    #[test]
    fn paths_are_single_raw_arguments_without_shell() {
        let argv = vec![OsString::from("code"), OsString::from("--reuse-window")];
        for relative in [
            "src/a file.rs",
            "città.rs",
            "-leading.rs",
            "semi;colon.rs",
            "dollar$name.rs",
            "quote\"name.rs",
        ] {
            let path = absolute_path(Path::new("/tmp/repo"), Path::new(relative)).unwrap();
            let (program, args) = invocation(&argv, &path);
            assert_eq!(program, "code");
            assert_eq!(
                args,
                [OsString::from("--reuse-window"), path.into_os_string()]
            );
        }
        let raw = std::path::PathBuf::from(OsString::from_vec(b"raw-\xff.txt".to_vec()));
        let path = absolute_path(Path::new("/tmp/repo"), &raw).unwrap();
        assert_eq!(invocation(&argv, &path).1[1], path.as_os_str());
    }

    #[test]
    fn missing_file_fails_before_editor_resolution() {
        let path =
            std::env::temp_dir().join(format!("gitilante-editor-missing-{}", std::process::id()));
        assert!(matches!(
            open(&path, Path::new("no-file")),
            Err(OpenError::MissingFile)
        ));
    }
}
