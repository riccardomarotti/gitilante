# Gitilante

A lightweight open source Git GUI, focused on the operations that are more
convenient in a graphical interface than in a terminal: reviewing the current
changes and manipulating single hunks.

Gitilante is a thin frontend over the `git` executable: it never reimplements
Git semantics and deliberately does not replace the CLI (no commit, push,
branch, merge, ...). See [SPEC.md](SPEC.md) for the full technical and
functional specification.

## Features (v0.1)

- open a repository from the command line, from any of its subdirectories;
- Changes view with Staged / Unstaged / Untracked sections;
- per-file and per-hunk actions: Stage, Unstage, Discard (with confirmation);
- monospace diff viewer with line numbers;
- History view with incremental loading (200 commits at a time) and the
  commit diff shown with the same renderer when a commit is selected;
- Revert a single hunk of a commit into the working tree, without creating
  any commit;
- binary files shown as such, with whole-file actions;
- refresh after every operation, on window focus, and with `Ctrl+R`.

## Requirements

- Linux
- Rust (1.85+), GTK 4 and libadwaita development packages
- `git` in `PATH`

## Build and run

```bash
cargo build --release
./target/release/gitilante ~/src/project
```

Or install it:

```bash
cargo install --path .
gitilante .
```

```text
Usage: gitilante [path]

Shortcuts:
  Ctrl+R         Refresh
  Ctrl+Q         Quit
```

Diagnostics are logged through `env_logger` (`RUST_LOG=debug` shows every Git
command with its duration and exit status).

## Development

The Git backend (`src/git`) is a thin, testable layer over the `git` binary and
is covered by extensive tests against real temporary repositories:

```bash
cargo test
cargo clippy --all-targets
cargo fmt
```
