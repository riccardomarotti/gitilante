# Gitilante

![demo](demo.gif)

A lightweight open source Git GUI, focused on the operations that are more
convenient in a graphical interface than in a terminal: reviewing changes,
resolving conflicts, and manipulating hunks or selected lines.

Gitilante is a thin frontend over the `git` executable: it never reimplements
Git semantics and deliberately does not replace the CLI (no commit, push,
branch, merge, ...).

## Features

### Changes

- open a repository from the command line, from any of its subdirectories;
- Changes view with Staged / Unstaged / Untracked sections;
- per-file and per-hunk actions: Stage, Unstage, Discard (with confirmation);
- select changed lines within one hunk to Stage, Unstage or Discard just those
  lines; partial replacements are supported;
- Revert a single hunk of a commit into the working tree, without creating
  any commit;
- binary files shown as such, with whole-file actions;
- conflicted files open a solver with operation-aware labels, side-by-side
  choices, editable results and whole-file choices for non-text conflicts;
  applying a resolution checks for external changes before writing and staging;
- refresh after every operation, on window focus, and with `Ctrl+R`.

### Diff viewer

- monospace source text with line numbers and markers in a dedicated gutter;
- syntax highlighting powered by GtkSourceView 5, adapted to the light/dark
  theme;
- line colors for added/removed lines with an extra overlay on the parts that
  actually changed inside a line (intraline diff);
- commit diffs render every file with the same viewer;
- collapsible files and hunks with `+N −M` summaries: fold what you are not
  reviewing, with `Collapse/Expand all` actions from the header menu; the
  fold state survives refreshes and selection switches, and searches
  automatically expand what they need to show.

### History

- incremental loading (200 commits at a time);
- commit graph with lanes, merges and branches, built from the commit DAG;
- badges for local and remote branches, with the current branch emphasized
  and an explicit `HEAD` badge on detached HEADs;
- the repository history covers unmerged branches and remote branches too;
- **File History** from a staged/unstaged file or a tracked search preview
  follows renames from `HEAD` and shows only that file's diff for each commit;
  `All history` returns to the repository view.

### Search

- `Ctrl+F` searches what is on screen (diff, commit diff, file preview): all
  matches highlighted, `Enter` / `Shift+Enter` to move, match counter,
  automatic scrolling;
- `Ctrl+Shift+F` searches the repository with the scopes
  `All | Changes | Files | Contents | History | History Changes`:
  - **Changes** — added/removed lines in the current staged and unstaged
    diffs, with an Added / Removed filter;
  - **Files** — fuzzy file name and path search (`hist rs` finds
    `src/git/history.rs`);
  - **Contents** — working tree contents, untracked non-ignored files
    included, binaries skipped;
  - **History** — commit subject, author, full/abbreviated commit ID and
    branch/ref names;
  - **History Changes** — commits where a string's occurrence count changed
    (`git log -S`), or whose diffs match a regex (`git log -G`);
- regular expression toggle (`.*`) and smart-case toggle (`Aa`);
- every result navigates to its match: the right line is revealed and
  highlighted;
- read-only working-tree file preview with line numbers and syntax highlighting;
- stale asynchronous results are discarded when the query changes or the
  dialog closes.

### Context actions

Right-click a file, hunk or commit to copy its repository-relative or absolute
path, full file/hunk patch, commit SHA or commit info as appropriate. Right-click
working-tree files in Changes, the diff viewer, a search preview or the conflict
solver to **Open in Editor**; historical diffs are not opened as current files.
The editor is chosen from `GITILANTE_EDITOR`, `VISUAL`, `EDITOR`, then the
desktop default. For example, `GITILANTE_EDITOR="code --reuse-window"` opens
files in a graphical editor without changing your Git commit-message editor.

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
- `git-lfs` only for repositories that use Git LFS (optional for other repos)

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
