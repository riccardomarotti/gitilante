//! Git backend: runs the system `git` executable and translates its output
//! into model types.
//!
//! Nothing in this module knows about GTK; it must stay testable on its own.

pub mod command;
pub mod diff;
pub mod error;
pub mod files;
pub mod grep;
pub mod history;
pub mod patch;
pub mod refs;
pub mod repository;
pub mod status;

pub use error::{Error, GitError};
pub use repository::Repository;

/// Result type used across the Git backend.
pub type Result<T> = std::result::Result<T, Error>;
