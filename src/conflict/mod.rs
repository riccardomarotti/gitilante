//! Conflict model: marker parsing, per-block resolutions and result building.
//!
//! This module is deliberately free of Git and GTK concerns: it works on raw
//! bytes taken from the working tree and never normalizes whitespace, line
//! endings or encodings. See docs/GITILANTE_CONFLICT_SOLVER_SPEC.md.

pub mod context;
pub mod markers;
pub mod model;
pub mod resolution;

pub use context::{CommitIdentity, ConflictContext, ConflictOperation, ConflictSource};
pub use markers::{MarkerParseError, parse};
pub use model::{ConflictBlock, ConflictFile, ConflictFilePart};
pub use resolution::{BuildError, ConflictResolution};
