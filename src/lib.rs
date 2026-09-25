//! Gitilante library: Git backend and data model.
//!
//! The UI layer must go through this crate and never run `git` directly, so
//! that every Git operation stays testable without GTK.

pub mod cli;
pub mod conflict;
pub mod git;
pub mod graph;
pub mod hunk_split;
pub mod intraline;
pub mod model;
pub mod search;
pub mod syntax;
pub mod ui;
