//! Pure presentation-time splitting of ordinary Git hunks.
//!
//! Synthetic hunks retain the original raw diff lines and can be passed
//! directly to the existing patch backend. The original [`FileDiff`] is never
//! modified (GITILANTE_SPLIT_HUNK_SPEC.md sections 1, 9-15, 25).

use crate::model::diff::{DiffLineKind, FileDiff, FileStatus, Hunk};

/// Number of unchanged lines retained around each synthetic hunk.
pub const SPLIT_CONTEXT: usize = 3;

/// One synthetic child hunk and its relation to the raw Git hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPart {
    /// Hunk accepted by the existing patch backend.
    pub hunk: Hunk,
    /// Half-open range in the original hunk represented by this child.
    pub source_start: usize,
    pub source_end: usize,
    /// One-based position, suitable for the `split N/M` label.
    pub index: usize,
    /// Total number of children.
    pub total: usize,
}

/// Whether this ordinary modified-file hunk can be safely split.
pub fn can_split(file: &FileDiff, hunk: &Hunk) -> bool {
    split_ranges(file, hunk).is_some()
}

/// Splits the hunk into one synthetic hunk per maximal contiguous change block.
pub fn split(file: &FileDiff, hunk: &Hunk) -> Option<Vec<SplitPart>> {
    let segments = split_ranges(file, hunk)?;
    let total = segments.len();
    let suffix = header_suffix(&hunk.header)?;
    let mut parts = Vec::with_capacity(total);

    for (offset, (source_start, source_end)) in segments.into_iter().enumerate() {
        let mut old_start = hunk.old_start;
        let mut new_start = hunk.new_start;
        for line in &hunk.lines[..source_start] {
            advance(line.kind, &mut old_start, &mut new_start);
        }

        let mut old_count = 0;
        let mut new_count = 0;
        for line in &hunk.lines[source_start..source_end] {
            match line.kind {
                DiffLineKind::Context => {
                    old_count += 1;
                    new_count += 1;
                }
                DiffLineKind::Deletion => old_count += 1,
                DiffLineKind::Addition => new_count += 1,
                DiffLineKind::NoNewlineMarker => return None,
            }
        }
        if old_count == 0 && new_count == 0 {
            return None;
        }
        if !hunk.lines[source_start..source_end]
            .iter()
            .any(|line| line.kind == DiffLineKind::Context)
        {
            return None;
        }

        let mut header =
            format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@").into_bytes();
        header.extend_from_slice(suffix);
        let child = Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            header,
            lines: hunk.lines[source_start..source_end].to_vec(),
        };
        parts.push(SplitPart {
            hunk: child,
            source_start,
            source_end,
            index: offset + 1,
            total,
        });
    }
    Some(parts)
}

fn split_ranges(file: &FileDiff, hunk: &Hunk) -> Option<Vec<(usize, usize)>> {
    if file.status != FileStatus::Modified
        || file.binary
        || file.metadata.iter().any(|line| {
            line.starts_with(b"old mode ")
                || line.starts_with(b"new mode ")
                || line.starts_with(b"new file mode ")
                || line.starts_with(b"deleted file mode ")
        })
        || !hunk.header.starts_with(b"@@ ")
        || hunk
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::NoNewlineMarker)
    {
        return None;
    }
    header_suffix(&hunk.header)?;

    let mut blocks = Vec::new();
    let mut index = 0;
    while index < hunk.lines.len() {
        if !is_change(hunk.lines[index].kind) {
            index += 1;
            continue;
        }
        let start = index;
        while index < hunk.lines.len() && is_change(hunk.lines[index].kind) {
            index += 1;
        }
        blocks.push((start, index));
    }
    if blocks.len() < 2 {
        return None;
    }

    let mut segments = Vec::with_capacity(blocks.len());
    for (part_index, &(block_start, block_end)) in blocks.iter().enumerate() {
        let lower_bound = if part_index == 0 {
            0
        } else {
            blocks[part_index - 1].1
        };
        let upper_bound = blocks
            .get(part_index + 1)
            .map_or(hunk.lines.len(), |next| next.0);

        let mut source_start = block_start;
        let mut preceding = 0;
        while source_start > lower_bound
            && preceding < SPLIT_CONTEXT
            && hunk.lines[source_start - 1].kind == DiffLineKind::Context
        {
            source_start -= 1;
            preceding += 1;
        }
        let mut source_end = block_end;
        let mut following = 0;
        while source_end < upper_bound
            && following < SPLIT_CONTEXT
            && hunk.lines[source_end].kind == DiffLineKind::Context
        {
            source_end += 1;
            following += 1;
        }
        if !hunk.lines[source_start..source_end]
            .iter()
            .any(|line| line.kind == DiffLineKind::Context)
        {
            return None;
        }
        segments.push((source_start, source_end));
    }
    Some(segments)
}

fn is_change(kind: DiffLineKind) -> bool {
    matches!(kind, DiffLineKind::Addition | DiffLineKind::Deletion)
}

fn advance(kind: DiffLineKind, old: &mut usize, new: &mut usize) {
    match kind {
        DiffLineKind::Context => {
            *old += 1;
            *new += 1;
        }
        DiffLineKind::Deletion => *old += 1,
        DiffLineKind::Addition => *new += 1,
        DiffLineKind::NoNewlineMarker => {}
    }
}

/// Returns the raw suffix after the second `@@` in a standard hunk header.
fn header_suffix(header: &[u8]) -> Option<&[u8]> {
    let rest = header.strip_prefix(b"@@")?;
    let end = rest.windows(2).position(|window| window == b"@@")? + 2;
    Some(&rest[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::patch::{file_patch, single_hunk_patch};
    use crate::model::diff::{DiffLine, IntralineSpan};
    use std::path::PathBuf;

    fn line(kind: DiffLineKind, content: &[u8]) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_vec(),
            intraline: vec![IntralineSpan { start: 0, end: 1 }],
        }
    }

    fn file(status: FileStatus, binary: bool, metadata: Vec<Vec<u8>>) -> FileDiff {
        FileDiff {
            header: b"diff --git a/file.rs b/file.rs".to_vec(),
            metadata,
            old_path: Some(PathBuf::from("file.rs")),
            new_path: Some(PathBuf::from("file.rs")),
            status,
            binary,
            hunks: Vec::new(),
        }
    }

    fn hunk(lines: Vec<DiffLine>) -> Hunk {
        let old_count = lines
            .iter()
            .filter(|line| matches!(line.kind, DiffLineKind::Context | DiffLineKind::Deletion))
            .count();
        let new_count = lines
            .iter()
            .filter(|line| matches!(line.kind, DiffLineKind::Context | DiffLineKind::Addition))
            .count();
        Hunk {
            old_start: 10,
            old_count,
            new_start: 10,
            new_count,
            header: format!("@@ -10,{old_count} +10,{new_count} @@ fn work()\n").into_bytes(),
            lines,
        }
    }

    fn two_blocks() -> Hunk {
        hunk(vec![
            line(DiffLineKind::Context, b"before\n"),
            line(DiffLineKind::Deletion, b"old A\n"),
            line(DiffLineKind::Addition, b"new A\n"),
            line(DiffLineKind::Context, b"between\n"),
            line(DiffLineKind::Deletion, b"old B\n"),
            line(DiffLineKind::Addition, b"new B\n"),
            line(DiffLineKind::Context, b"after\n"),
        ])
    }

    #[test]
    fn one_change_block_and_adjacent_replacements_are_not_splittable() {
        let file = file(FileStatus::Modified, false, Vec::new());
        let one = hunk(vec![
            line(DiffLineKind::Deletion, b"old\n"),
            line(DiffLineKind::Addition, b"new\n"),
        ]);
        let adjacent = hunk(vec![
            line(DiffLineKind::Deletion, b"old A\n"),
            line(DiffLineKind::Addition, b"new A\n"),
            line(DiffLineKind::Deletion, b"old B\n"),
            line(DiffLineKind::Addition, b"new B\n"),
        ]);
        assert!(!can_split(&file, &one));
        assert!(!can_split(&file, &adjacent));
    }

    #[test]
    fn context_separated_change_blocks_split_with_valid_coordinates_and_heading() {
        let file = file(FileStatus::Modified, false, Vec::new());
        let original = two_blocks();
        let parts = split(&file, &original).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].index, 1);
        assert_eq!(parts[0].total, 2);
        assert_eq!(parts[1].index, 2);
        assert_eq!(parts[1].total, 2);
        assert_eq!((parts[0].hunk.old_start, parts[0].hunk.old_count), (10, 3));
        assert_eq!((parts[0].hunk.new_start, parts[0].hunk.new_count), (10, 3));
        assert_eq!((parts[1].hunk.old_start, parts[1].hunk.old_count), (12, 3));
        assert_eq!((parts[1].hunk.new_start, parts[1].hunk.new_count), (12, 3));
        for part in &parts {
            assert!(part.hunk.header.ends_with(b" fn work()\n"));
            assert!(
                part.hunk
                    .lines
                    .iter()
                    .any(|line| line.kind == DiffLineKind::Context)
            );
            assert!(part.hunk.header.starts_with(b"@@ -"));
            let patch = single_hunk_patch(&file, &part.hunk);
            assert!(
                patch
                    .windows(part.hunk.header.len())
                    .any(|window| window == part.hunk.header)
            );
            let patch_text = String::from_utf8(patch).unwrap();
            if part.index == 1 {
                assert!(patch_text.contains("+new A"));
                assert!(!patch_text.contains("+new B"));
            } else {
                assert!(patch_text.contains("+new B"));
                assert!(!patch_text.contains("+new A"));
            }
        }
        let mut whole_file = file.clone();
        whole_file.hunks.push(original.clone());
        let whole_patch = String::from_utf8(file_patch(&whole_file)).unwrap();
        assert!(whole_patch.contains("+new A"));
        assert!(whole_patch.contains("+new B"));
        assert_eq!(original, two_blocks(), "the original hunk is not mutated");
    }

    #[test]
    fn three_blocks_keep_raw_bytes_and_additions_deletions() {
        let file = file(FileStatus::Modified, false, Vec::new());
        let original = hunk(vec![
            line(DiffLineKind::Deletion, b"old\xff A\n"),
            line(DiffLineKind::Addition, b"new\xfe A\n"),
            line(DiffLineKind::Context, b"gap one\n"),
            line(DiffLineKind::Addition, b"insert B\n"),
            line(DiffLineKind::Context, b"gap two\n"),
            line(DiffLineKind::Deletion, b"remove C\n"),
        ]);
        let parts = split(&file, &original).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.hunk.stats().additions)
                .sum::<usize>(),
            2
        );
        assert_eq!(
            parts
                .iter()
                .map(|part| part.hunk.stats().deletions)
                .sum::<usize>(),
            2
        );
        assert_eq!(parts[0].hunk.lines[0].content, b"old\xff A\n");
        assert_eq!(
            parts[0].hunk.lines[0].intraline,
            original.lines[0].intraline
        );
        assert_eq!(parts[1].hunk.lines[1].content, b"insert B\n");
        assert_eq!(parts[2].hunk.lines[1].content, b"remove C\n");
    }

    #[test]
    fn rejects_unsupported_files_and_no_newline_markers() {
        let base_hunk = two_blocks();
        for status in [
            FileStatus::Added,
            FileStatus::Deleted,
            FileStatus::Renamed,
            FileStatus::Copied,
            FileStatus::ModeChanged,
        ] {
            assert!(!can_split(&file(status, false, Vec::new()), &base_hunk));
        }
        assert!(!can_split(
            &file(FileStatus::Modified, true, Vec::new()),
            &base_hunk
        ));
        assert!(!can_split(
            &file(
                FileStatus::Modified,
                false,
                vec![b"old mode 100644".to_vec()]
            ),
            &base_hunk
        ));
        let no_newline = hunk(vec![
            line(DiffLineKind::Deletion, b"old\n"),
            line(
                DiffLineKind::NoNewlineMarker,
                b"\\ No newline at end of file",
            ),
            line(DiffLineKind::Context, b"gap\n"),
            line(DiffLineKind::Addition, b"new\n"),
        ]);
        assert!(!can_split(
            &file(FileStatus::Modified, false, Vec::new()),
            &no_newline
        ));
        let combined = Hunk {
            header: b"@@@ -1,2 -1,2 +1,2 @@@".to_vec(),
            ..base_hunk
        };
        assert!(!can_split(
            &file(FileStatus::Modified, false, Vec::new()),
            &combined
        ));
    }
}
