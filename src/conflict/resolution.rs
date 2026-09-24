//! Per-conflict-block resolution state and result building errors.

/// How a single conflict block is going to end up in the resolved file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConflictResolution {
    /// No choice made yet.
    #[default]
    Unresolved,
    /// Keep the content under the `<<<<<<<` marker.
    SourceA,
    /// Keep the content above the `>>>>>>>` marker.
    SourceB,
    /// Keep both contents, A first.
    SourceAThenB,
    /// Keep both contents, B first.
    SourceBThenA,
    /// Hand-edited result bytes for this block.
    Custom(Vec<u8>),
}

impl ConflictResolution {
    /// True once a concrete choice exists for the block.
    pub fn is_resolved(&self) -> bool {
        !matches!(self, Self::Unresolved)
    }
}

/// Why the resolved file could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A conflict block is still unresolved.
    Unresolved {
        /// 1-based id of the unresolved block.
        conflict: usize,
    },
    /// The resolutions list does not match the number of conflict blocks.
    ResolutionCountMismatch {
        /// Number of conflict blocks in the file.
        expected: usize,
        /// Number of resolutions provided.
        found: usize,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unresolved { conflict } => {
                write!(formatter, "conflict {conflict} is still unresolved")
            }
            Self::ResolutionCountMismatch { expected, found } => write!(
                formatter,
                "expected {expected} resolutions for the conflict blocks, found {found}"
            ),
        }
    }
}

impl std::error::Error for BuildError {}
