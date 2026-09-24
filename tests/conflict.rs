//! Conflict marker parsing and resolution building tests.
//!
//! The scenario numbers follow docs/GITILANTE_CONFLICT_SOLVER_SPEC.md.

mod common;

use common::TestRepo;
use gitilante::conflict::{
    BuildError, CommitIdentity, ConflictContext, ConflictFilePart, ConflictOperation,
    ConflictPresentation, ConflictResolution, ConflictSide, parse,
};
use gitilante::git::Repository as Backend;
use gitilante::git::conflict::conflict_context;
use std::path::Path;

/// Plain segment bytes at the given position.
fn plain(file: &gitilante::conflict::ConflictFile, index: usize) -> &[u8] {
    match &file.parts[index] {
        ConflictFilePart::Plain(bytes) => bytes,
        other => panic!("expected a plain part, got {other:?}"),
    }
}

/// §71 — basic markers with labels, reconstruction on source B.
#[test]
fn marker_parser_basic() {
    let input =
        b"before\n<<<<<<< HEAD\ntimeout = 10\n=======\ntimeout = 30\n>>>>>>> feature\nafter\n";
    let file = parse(input).unwrap();
    assert_eq!(file.conflict_count(), 1);
    assert_eq!(plain(&file, 0), b"before\n".as_slice());
    assert_eq!(plain(&file, 2), b"after\n".as_slice());

    let block = file.conflicts().next().unwrap();
    assert_eq!(block.id, 1);
    assert_eq!(block.source_a, b"timeout = 10\n".to_vec());
    assert_eq!(block.source_b, b"timeout = 30\n".to_vec());
    assert_eq!(block.base, None);
    assert_eq!(block.marker_a.as_deref(), Some("HEAD"));
    assert_eq!(block.marker_base, None);
    assert_eq!(block.marker_b.as_deref(), Some("feature"));

    let resolved = file.build_resolved(&[ConflictResolution::SourceB]).unwrap();
    assert_eq!(resolved, b"before\ntimeout = 30\nafter\n".to_vec());
}

/// §72 — diff3 markers expose the common base.
#[test]
fn marker_parser_diff3() {
    let input =
        b"<<<<<<< HEAD\ntimeout = 10\n||||||| base\ntimeout = 20\n=======\ntimeout = 30\n>>>>>>> feature\n";
    let file = parse(input).unwrap();
    let block = file.conflicts().next().unwrap();
    assert_eq!(block.source_a, b"timeout = 10\n".to_vec());
    assert_eq!(block.base, Some(b"timeout = 20\n".to_vec()));
    assert_eq!(block.source_b, b"timeout = 30\n".to_vec());
    assert_eq!(block.marker_base.as_deref(), Some("base"));
}

/// §73 — multiple conflicts keep every plain segment byte-for-byte.
#[test]
fn marker_parser_multiple_conflicts() {
    let input = b"plain1\n<<<<<<<\na1\n=======\nb1\n>>>>>>>\nplain2\n<<<<<<<\na2\n=======\nb2\n>>>>>>>\nplain3";
    let file = parse(input).unwrap();
    assert_eq!(file.conflict_count(), 2);
    assert_eq!(plain(&file, 0), b"plain1\n".as_slice());
    assert_eq!(plain(&file, 2), b"plain2\n".as_slice());
    assert_eq!(plain(&file, 4), b"plain3".as_slice());

    let resolved = file
        .build_resolved(&[
            ConflictResolution::SourceA,
            ConflictResolution::SourceAThenB,
        ])
        .unwrap();
    assert_eq!(resolved, b"plain1\na1\nplain2\na2\nb2\nplain3".to_vec());
}

/// §74 — trailing whitespace, CRLF and a missing EOF newline survive untouched.
#[test]
fn marker_parser_preserves_bytes() {
    let input = b"a  \r\n<<<<<<< HEAD\r\nx \r\n=======\r\ny\r\n>>>>>>> tail\r\nend";
    let file = parse(input).unwrap();
    let block = file.conflicts().next().unwrap();
    assert_eq!(block.source_a, b"x \r\n".to_vec());
    assert_eq!(block.source_b, b"y\r\n".to_vec());
    assert_eq!(block.marker_b.as_deref(), Some("tail"));

    let resolved = file
        .build_resolved(&[ConflictResolution::Custom(Vec::new())])
        .unwrap();
    assert_eq!(resolved, b"a  \r\nend".to_vec());
}

/// §75 — incomplete or inconsistent markers fail instead of guessing.
#[test]
fn marker_parser_rejects_malformed_structures() {
    // Unterminated block.
    assert!(parse(b"<<<<<<< HEAD\nx\n=======\ny\n").is_err());
    // Split marker without a closing marker.
    assert!(parse(b"a\n<<<<<<< HEAD\nx\n").is_err());
    // Nested opener inside a block.
    assert!(parse(b"<<<<<<<\nx\n<<<<<<<\ny\n=======\nz\n>>>>>>>\n").is_err());
    // Second base marker inside the base section.
    assert!(parse(b"<<<<<<<\nx\n||||||| b\ny\n||||||| c\nz\n=======\nm\n>>>>>>>\n").is_err());
    // Split marker repeated inside source B.
    assert!(parse(b"<<<<<<<\nx\n=======\ny\n=======\nz\n>>>>>>>\n").is_err());
}

/// Marker-looking lines outside a conflict are ordinary content.
#[test]
fn marker_looking_lines_outside_blocks_are_content() {
    let input = b"Title\n=======\n\n>>>>>>> quote\nbody\n";
    let file = parse(input).unwrap();
    assert_eq!(file.conflict_count(), 0);
    assert_eq!(file.parts.len(), 1);
    assert_eq!(plain(&file, 0), input);
}

/// Labels are opaque text: spaces and Unicode survive, absence maps to `None`.
#[test]
fn marker_labels_are_opaque() {
    let input = "<<<<<<< HEAD con spazi\nx\n=======\ny\n>>>>>>> fix ünicode\n".as_bytes();
    let file = parse(input).unwrap();
    let block = file.conflicts().next().unwrap();
    assert_eq!(block.marker_a.as_deref(), Some("HEAD con spazi"));
    assert_eq!(block.marker_b.as_deref(), Some("fix ünicode"));

    let unlabeled = parse(b"<<<<<<<\nx\n=======\ny\n>>>>>>>\n").unwrap();
    let block = unlabeled.conflicts().next().unwrap();
    assert_eq!(block.marker_a, None);
    assert_eq!(block.marker_b, None);
}

/// §80 — every resolution choice rebuilds the file exactly.
#[test]
fn resolution_choices_rebuild_exactly() {
    let file = parse(b"pre\n<<<<<<<\na1\na2\n=======\nb\n>>>>>>>\npost").unwrap();
    for (resolution, expected) in [
        (ConflictResolution::SourceA, b"pre\na1\na2\npost".to_vec()),
        (ConflictResolution::SourceB, b"pre\nb\npost".to_vec()),
        (
            ConflictResolution::SourceAThenB,
            b"pre\na1\na2\nb\npost".to_vec(),
        ),
        (
            ConflictResolution::SourceBThenA,
            b"pre\nb\na1\na2\npost".to_vec(),
        ),
        (
            ConflictResolution::Custom(b"mix\n".to_vec()),
            b"pre\nmix\npost".to_vec(),
        ),
    ] {
        assert_eq!(file.build_resolved(&[resolution]).unwrap(), expected);
    }
}

/// Unresolved blocks and wrong resolution counts fail the build.
#[test]
fn build_requires_every_conflict_resolved() {
    let file = parse(b"<<<<<<<\nx\n=======\ny\n>>>>>>>\n").unwrap();
    assert_eq!(
        file.build_resolved(&[ConflictResolution::Unresolved]),
        Err(BuildError::Unresolved { conflict: 1 })
    );
    assert_eq!(
        file.build_resolved(&[]),
        Err(BuildError::ResolutionCountMismatch {
            expected: 1,
            found: 0
        })
    );
}

/// Builds a repository whose `main` and `feature` branches conflict on one line.
fn conflicting_branches() -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.write("file.txt", b"base\n");
    repo.commit_all("initial");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("file.txt", b"from feature\n");
    repo.commit_all("feature change");
    let feature_oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["checkout", "main"]);
    repo.write("file.txt", b"from main\n");
    repo.commit_all("main change");
    (repo, feature_oid)
}

/// §77/§78 — a merge conflict labels the sides with the branch names.
#[test]
fn merge_conflict_labels_the_branches() {
    let (repo, _) = conflicting_branches();
    let (merged, ..) = repo.try_git(&["merge", "feature"]);
    assert!(!merged);

    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let context = conflict_context(&backend).unwrap();
    assert_eq!(context.operation, ConflictOperation::Merge);
    assert_eq!(context.source_a.label, "CURRENT · main");
    assert_eq!(context.source_b.label, "INCOMING · feature");
    assert_eq!(
        context.description().as_deref(),
        Some("Merge feature into main")
    );
    // §B: no ours/theirs wording anywhere in the UI actions.
    for action in [
        context.use_a_action(),
        context.use_b_action(),
        context.a_then_b_action(),
        context.b_then_a_action(),
    ] {
        assert!(!action.contains("ours"));
        assert!(!action.contains("theirs"));
    }
    assert_eq!(context.use_a_action(), "Use current");
    assert_eq!(context.use_b_action(), "Use incoming");
}

/// §6 — cherry-pick labels identify the commit by OID and subject.
#[test]
fn cherry_pick_conflict_labels_the_commit() {
    let (repo, feature_oid) = conflicting_branches();
    let (picked, ..) = repo.try_git(&["cherry-pick", &feature_oid]);
    assert!(!picked);

    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let context = conflict_context(&backend).unwrap();
    assert_eq!(context.operation, ConflictOperation::CherryPick);
    assert_eq!(context.source_a.label, "CURRENT RESULT");
    let short = &feature_oid[..7];
    assert_eq!(
        context.source_b.label,
        format!("CHERRY-PICKED COMMIT · {short} \"feature change\"")
    );
    assert_eq!(
        context.description().as_deref(),
        Some(format!("Cherry-pick of {short} \"feature change\"").as_str())
    );
}

/// §7/§79 — during a rebase the result side is not called "current branch".
#[test]
fn rebase_conflict_labels_the_replayed_commit() {
    let (repo, feature_oid) = conflicting_branches();
    repo.git(&["checkout", "feature"]);
    let (rebased, ..) = repo.try_git(&["rebase", "main"]);
    assert!(!rebased);

    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let context = conflict_context(&backend).unwrap();
    assert_eq!(context.operation, ConflictOperation::Rebase);
    assert_eq!(context.source_a.label, "REBASED RESULT");
    assert!(
        context
            .source_b
            .label
            .starts_with("COMMIT BEING REPLAYED · ")
    );
    assert!(context.source_b.label.contains("feature change"));
    assert_eq!(context.use_a_action(), "Use rebased result");
    assert_eq!(context.use_b_action(), "Use replayed commit");
    assert!(!context.source_a.label.contains("current branch"));
    let _ = feature_oid;
}

/// §9 — unknown operations fall back to neutral labels.
#[test]
fn unknown_operation_uses_neutral_labels() {
    let context = ConflictContext::new(
        ConflictOperation::Unknown,
        Some("main".to_owned()),
        CommitIdentity::default(),
        CommitIdentity::default(),
    );
    assert_eq!(context.source_a.label, "VERSION A");
    assert_eq!(context.source_b.label, "VERSION B");
    assert_eq!(context.description(), None);
    assert_eq!(context.use_a_action(), "Use version A");
    assert_eq!(context.use_b_action(), "Use version B");
}

/// Builds a repository where both branches add the same path with different
/// content: a text add/add conflict (§47, §84).
fn add_add_conflict(file_name: &str, ours: &[u8], theirs: &[u8]) -> TestRepo {
    let repo = TestRepo::new();
    repo.write("base.txt", b"base\n");
    repo.commit_all("initial");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write(file_name, theirs);
    repo.commit_all("feature adds");
    repo.git(&["checkout", "main"]);
    repo.write(file_name, ours);
    repo.commit_all("main adds");
    let (merged, ..) = repo.try_git(&["merge", "feature"]);
    assert!(!merged);
    repo
}

/// §84 — a text add/add conflict opens as blocks and resolves into the file.
#[test]
fn add_add_text_conflict_resolves() {
    let repo = add_add_conflict("added.txt", b"our content\n", b"their content\n");
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("added.txt");

    let load = backend.conflict(path).unwrap().unwrap();
    let ConflictPresentation::TextBlocks(file) = &load.presentation else {
        panic!("expected text blocks, got {:?}", load.presentation);
    };
    assert_eq!(file.conflict_count(), 1);

    let resolved = file.build_resolved(&[ConflictResolution::SourceB]).unwrap();
    backend.apply_conflict_resolution(path, &resolved).unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
    assert_eq!(
        std::fs::read(repo.root().join("added.txt")).unwrap(),
        b"their content\n"
    );
}

/// §85 — binary add/add is a whole-file choice with no text decoding.
#[test]
fn binary_add_add_conflict_uses_whole_file_choice() {
    let repo = add_add_conflict(
        "logo.bin",
        b"\x00\x01our binary\n",
        b"\x00\x02their binary\n",
    );
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("logo.bin");

    let load = backend.conflict(path).unwrap().unwrap();
    let ConflictPresentation::WholeFileChoice {
        version_a,
        version_b,
    } = &load.presentation
    else {
        panic!("expected whole-file choice, got {:?}", load.presentation);
    };
    assert!(version_a.binary && version_b.binary);
    assert_eq!(version_a.bytes, b"\x00\x01our binary\n");
    assert_eq!(version_b.bytes, b"\x00\x02their binary\n");

    backend
        .apply_conflict_resolution(path, &version_a.bytes)
        .unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
    assert_eq!(
        std::fs::read(repo.root().join("logo.bin")).unwrap(),
        b"\x00\x01our binary\n"
    );
}

/// §83 — modify/delete offers keep/delete and can resolve either way.
#[test]
fn modify_delete_conflict_can_keep_or_delete() {
    // Deleted by them: our modified version survives.
    let repo = TestRepo::new();
    repo.write("victim.txt", b"original\n");
    repo.commit_all("initial");
    repo.git(&["checkout", "-b", "feature"]);
    repo.remove("victim.txt");
    repo.commit_all("feature deletes");
    repo.git(&["checkout", "main"]);
    repo.write("victim.txt", b"original\nmodified by main\n");
    repo.commit_all("main modifies");
    let (merged, ..) = repo.try_git(&["merge", "feature"]);
    assert!(!merged);

    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("victim.txt");
    let load = backend.conflict(path).unwrap().unwrap();
    let ConflictPresentation::KeepOrDelete {
        content,
        content_side,
    } = &load.presentation
    else {
        panic!("expected keep-or-delete, got {:?}", load.presentation);
    };
    assert_eq!(*content_side, ConflictSide::SourceA);
    assert_eq!(content, b"original\nmodified by main\n");

    backend.mark_deleted(path).unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
    assert!(!repo.root().join("victim.txt").exists());
}

/// §83 — deleted by us: the other side's version can be kept as staged.
#[test]
fn deleted_by_us_conflict_can_keep_file() {
    let repo = TestRepo::new();
    repo.write("victim.txt", b"original\n");
    repo.commit_all("initial");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("victim.txt", b"original\nmodified by feature\n");
    repo.commit_all("feature modifies");
    repo.git(&["checkout", "main"]);
    repo.remove("victim.txt");
    repo.commit_all("main deletes");
    let (merged, ..) = repo.try_git(&["merge", "feature"]);
    assert!(!merged);

    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("victim.txt");
    let load = backend.conflict(path).unwrap().unwrap();
    let ConflictPresentation::KeepOrDelete { content_side, .. } = &load.presentation else {
        panic!("expected keep-or-delete, got {:?}", load.presentation);
    };
    assert_eq!(*content_side, ConflictSide::SourceB);

    backend.mark_resolved(path).unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
    assert!(repo.root().join("victim.txt").exists());
}

/// §82 — markers removed by hand only need staging, content stays untouched.
#[test]
fn manually_resolved_file_is_only_staged() {
    let repo = add_add_conflict("added.txt", b"our content\n", b"their content\n");
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("added.txt");
    repo.write("added.txt", b"fixed by hand\n");

    let load = backend.conflict(path).unwrap().unwrap();
    assert_eq!(
        load.presentation,
        ConflictPresentation::AlreadyManuallyResolved
    );

    backend.mark_resolved(path).unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
    assert_eq!(
        std::fs::read(repo.root().join("added.txt")).unwrap(),
        b"fixed by hand\n"
    );
}

/// §81 — external edits invalidate the solver's snapshot.
#[test]
fn external_edit_invalidates_the_snapshot() {
    let repo = add_add_conflict("added.txt", b"our content\n", b"their content\n");
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("added.txt");
    let load = backend.conflict(path).unwrap().unwrap();
    assert!(
        backend
            .conflict_file_unchanged(path, load.working_tree.as_deref())
            .unwrap()
    );

    repo.write("added.txt", b"changed outside\n");
    assert!(
        !backend
            .conflict_file_unchanged(path, load.working_tree.as_deref())
            .unwrap()
    );
}

/// §86 — paths with spaces and a leading dash work through `--`.
#[test]
fn conflicted_paths_with_spaces_and_dashes_work() {
    let repo = add_add_conflict("wei rd -name.txt", b"ours\n", b"theirs\n");
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    let path = Path::new("wei rd -name.txt");

    let load = backend.conflict(path).unwrap().unwrap();
    let ConflictPresentation::TextBlocks(file) = &load.presentation else {
        panic!("expected text blocks, got {:?}", load.presentation);
    };
    let resolved = file.build_resolved(&[ConflictResolution::SourceA]).unwrap();
    backend.apply_conflict_resolution(path, &resolved).unwrap();
    assert_eq!(backend.status().unwrap().unmerged_entries().count(), 0);
}

/// §56 — a non-unmerged path reports no conflict.
#[test]
fn clean_path_reports_no_conflict() {
    let repo = TestRepo::new();
    repo.write("file.txt", b"content\n");
    repo.commit_all("initial");
    let backend = Backend::discover(Path::new(repo.root())).unwrap();
    assert_eq!(backend.conflict(Path::new("file.txt")).unwrap(), None);
}

/// §31 — near-identical lines get intraline marks on both sides.
#[test]
fn intraline_marks_highlight_the_changed_parts() {
    use gitilante::conflict::presentation::intraline_marks;

    let (marks_a, marks_b) = intraline_marks(b"timeout = 10\n", b"timeout = 30\n");
    assert_eq!(marks_a.len(), 1);
    assert_eq!(marks_b.len(), 1);
    for marks in [&marks_a, &marks_b] {
        assert!(!marks[0].is_empty());
        // The changed digits live inside "10" / "30" (columns 10..12).
        assert!(
            marks[0]
                .iter()
                .all(|(start, end)| *start >= 10 && *end <= 12)
        );
    }

    // Completely different lines get no pairing at all (§31).
    let (marks_a, marks_b) = intraline_marks(b"alpha\n", b"1234567890 qwerty asdfgh zxcv\n");
    assert!(marks_a.iter().all(|spans| spans.is_empty()));
    assert!(marks_b.iter().all(|spans| spans.is_empty()));
}
