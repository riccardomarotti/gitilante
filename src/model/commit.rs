//! Commit model.

/// A commit as reported by `git log` (SPEC section 16).
///
/// Text fields are display-oriented and decoded lossily, since commit metadata
/// is never fed back to Git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full object name of the commit.
    pub oid: String,
    /// Parent object names; empty for a root commit.
    pub parents: Vec<String>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Author time as a Unix timestamp in seconds.
    pub author_time: i64,
    /// First line of the commit message.
    pub subject: String,
}

impl Commit {
    /// Short object name for lists: the first 7 characters.
    pub fn short_oid(&self) -> &str {
        &self.oid[..self.oid.len().min(7)]
    }
}
