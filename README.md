# Gitilante

A lightweight open source Git GUI, focused on the operations that are more
convenient in a graphical interface than in a terminal: reviewing the current
changes and manipulating single hunks.

Gitilante is a thin frontend over the `git` executable: it never reimplements
Git semantics and deliberately does not replace the CLI (no commit, push,
branch, merge, ...).

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
- conflicted files listed in their own read-only section;
- refresh after every operation, on window focus, and with `Ctrl+R`;
- keyboard shortcuts acting on the focused hunk (`S`, `U`, `D`, `R`).

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

The Arch package also installs the short command name `gila` as an alias.

```text
Usage: gitilante [path]

Shortcuts:
  Ctrl+R         Refresh
  Ctrl+Q         Quit
  S              Stage the focused hunk
  U              Unstage the focused hunk
  D              Discard the focused hunk (asks for confirmation)
  R              Revert the focused hunk of a commit
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

## Packaging

Gitilante is available on the AUR in two flavors:

```bash
yay -S gitilante       # builds the release sources with Cargo
yay -S gitilante-bin   # installs the official prebuilt binary
```

Both install the same application (`gitilante`, its `gila` alias and the
desktop/icon/metainfo files) and conflict with each other.

Release packaging lives in `packaging/aur/` and `packaging/aur-bin/`
(`PKGBUILD` templates) and `scripts/`. To generate the AUR package metadata
locally:

```bash
./scripts/generate-aur-package.sh 0.1.0        # from the GitLab tag archive
./scripts/generate-aur-package.sh 0.1.0 --local  # testing only, pre-tag
cd dist/aur && makepkg
```

For the binary package (the bundle must already exist):

```bash
./scripts/build-binary-bundle.sh 0.1.0
./scripts/generate-aur-bin-package.sh 0.1.0 --archive dist/bin/gitilante-0.1.0-linux-x86_64.tar.gz
cd dist/aur-bin && makepkg
```
