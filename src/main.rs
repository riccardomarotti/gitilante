//! `gitilante` command line entry point.
//!
//! Usage: `gitilante [path]` opens the repository containing `path` (default:
//! the current directory). Until the GTK UI lands (Fase 4) this binary only
//! resolves the repository and prints its status.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use gitilante::git::{Error, Repository};
use gitilante::model::status::{Status, StatusEntry};

const USAGE: &str = "\
Usage: gitilante [path]

Opens the Git repository containing <path> (any subdirectory is accepted).
When <path> is omitted, the current directory is used.

Options:
  -h, --help     Show this help
  -V, --version  Show the version
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

    match repo.status() {
        Ok(status) => {
            println!("Repository: {}", repo.root().display());
            print_status(&status);
            ExitCode::SUCCESS
        }
        Err(error) => report(&error),
    }
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

/// Prints a status summary (temporary scaffolding until the GTK UI lands).
fn print_status(status: &Status) {
    let head = match &status.branch.head {
        gitilante::model::status::Head::Branch(name) => name.clone(),
        gitilante::model::status::Head::Detached => "(detached)".to_owned(),
        gitilante::model::status::Head::Unknown => "(unknown)".to_owned(),
    };
    println!("Branch: {head}");

    print_entries("Staged:", status.staged_entries().collect());
    print_entries("Unstaged:", status.unstaged_entries().collect());
    print_entries("Untracked:", status.untracked_entries().collect());
}

fn print_entries(title: &str, entries: Vec<&StatusEntry>) {
    if entries.is_empty() {
        return;
    }
    println!("{title}");
    for entry in entries {
        let orig = entry
            .orig_path
            .as_ref()
            .map(|path| format!(" (from {})", path.display()))
            .unwrap_or_default();
        println!("  {}{}", entry.path.display(), orig);
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
