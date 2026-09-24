# Gitilante

A lightweight open source Git GUI, focused on the operations that are more
convenient in a graphical interface than in a terminal: reviewing the current
changes and manipulating single hunks.

Gitilante is a thin frontend over the `git` executable: it never reimplements
Git semantics and deliberately does not replace the CLI (no commit, push,
branch, merge, ...).

## Features

### Changes

- open a repository from the command line, from any of its subdirectories;
- Changes view with Staged / Unstaged / Untracked sections;
- per-file and per-hunk actions: Stage, Unstage, Discard (with confirmation);
- Revert a single hunk of a commit into the working tree, without creating
  any commit;
- binary files shown as such, with whole-file actions;
- conflicted files listed in their own read-only section;
- refresh after every operation, on window focus, and with `Ctrl+R`.

### Diff viewer

- monospace source text with line numbers and markers in a dedicated gutter;
- syntax highlighting powered by GtkSourceView 5, adapted to the light/dark
  theme;
- line colors for added/removed lines with an extra overlay on the parts that
  actually changed inside a line (intraline diff);
- commit diffs render every file with the same viewer.

### History

- incremental loading (200 commits at a time);
- commit graph with lanes, merges and branches, built from the commit DAG;
- badges for local and remote branches, with the current branch emphasized
  and an explicit `HEAD` badge on detached HEADs;
- the history covers unmerged branches and remote branches too.

### Search

- `Ctrl+F` searches what is on screen (diff, commit diff, file preview): all
  matches highlighted, `Enter` / `Shift+Enter` to move, match counter,
  automatic scrolling;
- `Ctrl+Shift+F` searches the repository with the scopes
  `All | Changes | Files | Contents | History | History Changes`:
  - **Changes** — only the added/removed lines of the diffs on screen, with an
    Added / Removed filter;
  - **Files** — fuzzy file name and path search (`hist rs` finds
    `src/git/history.rs`);
  - **Contents** — working tree contents, untracked non-ignored files
    included, binaries skipped;
  - **History** — commit subject, author, object names and branch/ref names;
  - **History Changes** — commits that added or removed a string
    (`git log -S` / `-G`);
- regular expression toggle (`.*`) and smart-case toggle (`Aa`);
- every result navigates to its match: the right line is revealed and
  highlighted;
- read-only file preview with line numbers and syntax highlighting.

### Keyboard

```text
Usage: gitilante [path]

Shortcuts:
  Ctrl+R         Refresh
  Ctrl+F         Search in the current view
  Ctrl+Shift+F   Search the repository
  Ctrl+Q         Quit
  S              Stage the focused hunk
  U              Unstage the focused hunk
  D              Discard the focused hunk (asks for confirmation)
  R              Revert the focused hunk of a commit
```

The single-key shortcuts act on the focused hunk and never fire while typing
in a search field.

## Requirements

- Linux
- Rust (1.85+), GTK 4, libadwaita and GtkSourceView 5 development packages
- `git` in `PATH`

## Build and run

```bash
cargo build --release
./target/release/gitilante ~/src/project
```

Or install it with `cargo install --path .`, or from the AUR in two flavors:

```bash
yay -S gitilante       # builds the release sources with Cargo
yay -S gitilante-bin   # installs the official prebuilt binary
```

Both install the same application (`gitilante`, its `gila` alias and the
desktop/icon/metainfo files) and conflict with each other.

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
