//! Error type for everything in this crate.

/// What can go wrong when taking snapshots or resolving refs.
///
/// Every variant carries a human/AI-readable message that already says what to do next
/// (for example "take a new snapshot"), so callers can forward it to the model as-is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PerceptionError {
    /// The ref text is malformed, or was never handed out (typo, made-up number).
    #[error("bad ref `{input}`: {reason}")]
    BadRef { input: String, reason: String },

    /// The ref was valid once, but the page navigated (new document) since then,
    /// or the snapshot it came from is too old to remember.
    #[error("stale ref `{input}`: {reason}")]
    StaleRef { input: String, reason: String },

    /// The ref is fine, but the node is not in the accessibility tree we were given
    /// (it was removed from the page, or the tree is empty).
    #[error("not found: {0}")]
    NotFound(String),

    /// A `find` filter is empty or its regex does not compile.
    #[error("bad filter: {0}")]
    BadFilter(String),

    /// The CDP JSON does not look like an `Accessibility.getFullAXTree` result.
    #[error("bad CDP data: {0}")]
    BadTree(String),
}

impl PerceptionError {
    /// Stable machine-readable code, matching the API error codes in `docs/plan.md`.
    pub fn code(&self) -> &'static str {
        match self {
            PerceptionError::BadRef { .. } => "BAD_REF",
            PerceptionError::StaleRef { .. } => "STALE_REF",
            PerceptionError::NotFound(_) => "NOT_FOUND",
            PerceptionError::BadFilter(_) => "BAD_FILTER",
            PerceptionError::BadTree(_) => "BAD_TREE",
        }
    }
}

/// Short alias used throughout the crate.
pub type Result<T> = std::result::Result<T, PerceptionError>;
