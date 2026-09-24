//! Conflict file model: plain segments, conflict blocks and presentations.

use crate::conflict::resolution::{BuildError, ConflictResolution};

/// One parsed chunk of a working tree file containing conflict markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictFilePart {
    /// Bytes outside any conflict block, preserved verbatim.
    Plain(Vec<u8>),
    /// A single conflict region between `<<<<<<<` and `>>>>>>>` markers.
    Conflict(ConflictBlock),
}

/// A single conflict region with its two (or three) alternatives.
///
/// Contents are raw bytes including their line terminators, exactly as found
/// in the working tree between the markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlock {
    /// 1-based position of this block inside the file.
    pub id: usize,
    /// Content under the `<<<<<<<` marker (stage 2 side).
    pub source_a: Vec<u8>,
    /// Content under the `|||||||` marker (diff3/zdiff3 only).
    pub base: Option<Vec<u8>>,
    /// Content above the `>>>>>>>` marker (stage 3 side).
    pub source_b: Vec<u8>,
    /// Label text found after `<<<<<<<`, when present.
    pub marker_a: Option<String>,
    /// Label text found after `|||||||`, when present.
    pub marker_base: Option<String>,
    /// Label text found after `>>>>>>>`, when present.
    pub marker_b: Option<String>,
}

/// A parsed file: alternating plain segments and conflict blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictFile {
    /// Parsed parts, in file order.
    pub parts: Vec<ConflictFilePart>,
}

impl ConflictFile {
    /// Wraps a parsed part list.
    pub fn from_parts(parts: Vec<ConflictFilePart>) -> Self {
        Self { parts }
    }

    /// All conflict blocks in file order.
    pub fn conflicts(&self) -> impl Iterator<Item = &ConflictBlock> {
        self.parts.iter().filter_map(|part| match part {
            ConflictFilePart::Conflict(block) => Some(block),
            ConflictFilePart::Plain(_) => None,
        })
    }

    /// Number of conflict blocks in the file.
    pub fn conflict_count(&self) -> usize {
        self.conflicts().count()
    }

    /// True when the file contains no conflict markers at all.
    pub fn has_no_markers(&self) -> bool {
        self.conflict_count() == 0
    }

    /// Builds the resolved file bytes from one resolution per conflict block.
    ///
    /// Plain segments are copied byte-for-byte; resolved blocks contribute
    /// their chosen content with no added separators or newlines. Fails when
    /// any block is still unresolved.
    pub fn build_resolved(
        &self,
        resolutions: &[ConflictResolution],
    ) -> Result<Vec<u8>, BuildError> {
        if resolutions.len() != self.conflict_count() {
            return Err(BuildError::ResolutionCountMismatch {
                expected: self.conflict_count(),
                found: resolutions.len(),
            });
        }
        let mut output = Vec::new();
        let mut next_resolution = 0;
        for part in &self.parts {
            match part {
                ConflictFilePart::Plain(bytes) => output.extend_from_slice(bytes),
                ConflictFilePart::Conflict(block) => {
                    let resolution = &resolutions[next_resolution];
                    next_resolution += 1;
                    output.extend_from_slice(
                        &block
                            .resolved_bytes(resolution)
                            .ok_or(BuildError::Unresolved { conflict: block.id })?,
                    );
                }
            }
        }
        Ok(output)
    }
}

impl ConflictBlock {
    /// Bytes this block contributes to the file for the given resolution, or
    /// `None` while the block is still unresolved.
    pub fn resolved_bytes(&self, resolution: &ConflictResolution) -> Option<Vec<u8>> {
        match resolution {
            ConflictResolution::Unresolved => None,
            ConflictResolution::SourceA => Some(self.source_a.clone()),
            ConflictResolution::SourceB => Some(self.source_b.clone()),
            ConflictResolution::SourceAThenB => {
                let mut bytes = self.source_a.clone();
                bytes.extend_from_slice(&self.source_b);
                Some(bytes)
            }
            ConflictResolution::SourceBThenA => {
                let mut bytes = self.source_b.clone();
                bytes.extend_from_slice(&self.source_a);
                Some(bytes)
            }
            ConflictResolution::Custom(bytes) => Some(bytes.clone()),
        }
    }
}
