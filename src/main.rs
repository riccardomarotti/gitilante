//! `gitilante` command line entry point.
//!
//! Usage: `gitilante [path]` opens the repository containing `path` (default:
//! the current directory) in the GUI.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use gitilante::git::Error;
use gitilante::git::repository::Repository;
use gitilante::ui;

const USAGE: &str = "\
Usage: gitilante [path]

Opens the Git repository containing <path> (any subdirectory is accepted).
When <path> is omitted, the current directory is used.

Options:
  -h, --help     Show this help
  -V, --version  Show the version

The Arch package also installs the short name `gila`.

Shortcuts:
  Ctrl+R         Refresh
  Ctrl+Q         Quit
  S              Stage the focused hunk
  U              Unstage the focused hunk
  D              Discard the focused hunk (asks for confirmation)
  R              Revert the focused hunk of a commit
";

/// What the command line asked for.
enum Args {
    ShowHelp,
    ShowVersion,
    Open(PathBuf),
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let path = match parse_args(env::args_os().skip(1)) {
        Ok(Args::ShowHelp) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Args::ShowVersion) => {
            println!("gitilante {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Args::Open(path)) => path,
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let repo = match Repository::discover(&path) {
        Ok(repo) => repo,
        Err(error) => return report(&error),
    };

    ExitCode::from(ui::app::run(repo) as u8)
}

/// Parses the command line.
fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<Args, String> {
    let mut positional: Vec<OsString> = Vec::new();
    let mut only_paths = false;

    for arg in args {
        if !only_paths {
            match arg.to_str() {
                Some("--") => {
                    only_paths = true;
                    continue;
                }
                Some("-h") | Some("--help") => return Ok(Args::ShowHelp),
                Some("-V") | Some("--version") => return Ok(Args::ShowVersion),
                Some(other) if other.starts_with('-') => {
                    return Err(format!("unknown option: {other}"));
                }
                _ => {}
            }
        }
        positional.push(arg);
    }

    match positional.len() {
        0 => Ok(Args::Open(PathBuf::from("."))),
        1 => Ok(Args::Open(PathBuf::from(&positional[0]))),
        _ => Err("too many paths given".to_owned()),
    }
}

/// Prints a clear error and returns a failure exit code.
fn report(error: &Error) -> ExitCode {
    eprintln!("{error}");
    if let Error::Git(git_error) = error {
        log::debug!("{}", git_error.details());
    }
    ExitCode::FAILURE
}
