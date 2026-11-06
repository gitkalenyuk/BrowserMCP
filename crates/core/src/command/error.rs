//! `CmdError`: the one error type every command returns.
//!
//! It is designed to be read by an AI agent: besides a human message it says *what to do next*
//! (`retry`), may carry a `hint` ("call tabs_list first") and, for ambiguous targets, a list of
//! `candidates` to choose from. On the wire it looks like:
//!
//! ```json
//! {"code":"TAB_NOT_FOUND","message":"tab 7 not found","retry":"no","hint":"Call tabs_list to see open tabs."}
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::BackendError;

/// Machine-readable error code. Serialized as `SCREAMING_SNAKE_CASE`, e.g. `TAB_LOCKED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// An element ref (`@7:e3`) is from an old snapshot.
    StaleRef,
    /// A target description matched more than one element; see `candidates`.
    AmbiguousTarget,
    /// Waiting for something took too long.
    Timeout,
    /// The element exists but cannot be clicked/typed into (hidden, disabled, covered).
    NotInteractable,
    /// Something (command, element, file, …) does not exist.
    NotFound,
    /// Another caller (agent or human) holds the lock on this tab.
    TabLocked,
    /// No tab with that id.
    TabNotFound,
    /// The command needs a tab but none is open.
    NoTabs,
    /// A navigation started while the command was running.
    NavigationInterrupted,
    /// The renderer process of the tab crashed.
    PageCrashed,
    /// The tab was closed while the command was running.
    TabClosed,
    /// The user must approve this action first.
    NeedsApproval,
    /// Not allowed while the profile is in stealth mode.
    BlockedInStealth,
    /// Arguments did not match the command's schema.
    InvalidParams,
    /// The browser is shutting down.
    ShuttingDown,
    /// The command exists but this option is not built yet (see `hint`).
    NotImplemented,
    /// A JavaScript dialog (alert/confirm/prompt/beforeunload) is open and blocks the page;
    /// `candidates[0]` describes it. Answer it with `page_dialog`, then retry.
    DialogOpen,
    /// A bug or an unexpected failure.
    Internal,
}

impl ErrorCode {
    /// The wire spelling, e.g. `"TAB_LOCKED"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::StaleRef => "STALE_REF",
            ErrorCode::AmbiguousTarget => "AMBIGUOUS_TARGET",
            ErrorCode::Timeout => "TIMEOUT",
            ErrorCode::NotInteractable => "NOT_INTERACTABLE",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::TabLocked => "TAB_LOCKED",
            ErrorCode::TabNotFound => "TAB_NOT_FOUND",
            ErrorCode::NoTabs => "NO_TABS",
            ErrorCode::NavigationInterrupted => "NAVIGATION_INTERRUPTED",
            ErrorCode::PageCrashed => "PAGE_CRASHED",
            ErrorCode::TabClosed => "TAB_CLOSED",
            ErrorCode::NeedsApproval => "NEEDS_APPROVAL",
            ErrorCode::BlockedInStealth => "BLOCKED_IN_STEALTH",
            ErrorCode::InvalidParams => "INVALID_PARAMS",
            ErrorCode::ShuttingDown => "SHUTTING_DOWN",
            ErrorCode::NotImplemented => "NOT_IMPLEMENTED",
            ErrorCode::DialogOpen => "DIALOG_OPEN",
            ErrorCode::Internal => "INTERNAL",
        }
    }

    /// The retry advice used when a command does not pick one explicitly.
    pub fn default_retry(self) -> Retry {
        match self {
            ErrorCode::StaleRef | ErrorCode::AmbiguousTarget | ErrorCode::NotFound => {
                Retry::AfterSnapshot
            }
            ErrorCode::Timeout
            | ErrorCode::NotInteractable
            | ErrorCode::TabLocked
            | ErrorCode::NeedsApproval => Retry::AfterWait,
            ErrorCode::NavigationInterrupted => Retry::Now,
            ErrorCode::TabNotFound
            | ErrorCode::NoTabs
            | ErrorCode::PageCrashed
            | ErrorCode::TabClosed
            | ErrorCode::BlockedInStealth
            | ErrorCode::InvalidParams
            | ErrorCode::ShuttingDown
            | ErrorCode::NotImplemented
            // The dialog stays until someone answers it (page_dialog); retrying does not help.
            | ErrorCode::DialogOpen
            | ErrorCode::Internal => Retry::No,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the caller should do about an error. Serialized as `now`, `after_wait`, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retry {
    /// Retrying the same call right away will probably work.
    Now,
    /// Retry after waiting (page still loading, tab locked by someone else, …).
    AfterWait,
    /// Take a fresh snapshot first; the old refs/targets are no longer valid.
    AfterSnapshot,
    /// Retrying the same call will fail again; change the arguments or give up.
    No,
}

/// Error returned by every command.
///
/// Build one with [`CmdError::new`] and the `with_*` helpers:
///
/// ```
/// use bmcp_core::command::{CmdError, ErrorCode};
/// let err = CmdError::new(ErrorCode::NotFound, "no button 'Save'").with_hint("Take a snapshot.");
/// assert_eq!(err.code, ErrorCode::NotFound);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct CmdError {
    pub code: ErrorCode,
    pub message: String,
    pub retry: Retry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Possible intended targets (for `AMBIGUOUS_TARGET`, unknown command names, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Value>,
    /// `TAB_LOCKED`: who holds the tab — an agent id, or `"user"` after a human took over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

impl CmdError {
    /// New error with the code's [default retry advice](ErrorCode::default_retry).
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        CmdError {
            code,
            message: message.into(),
            retry: code.default_retry(),
            hint: None,
            candidates: Vec::new(),
            owner: None,
        }
    }

    pub fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_candidates(mut self, candidates: Vec<Value>) -> Self {
        self.candidates = candidates;
        self
    }

    /// Set [`CmdError::owner`] (who holds a locked tab).
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Shortcut for `INVALID_PARAMS`.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, message)
    }

    /// Shortcut for `INTERNAL`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// Shortcut for `TAB_LOCKED`; `holder` is who owns the lock (an agent id, or "user").
    pub fn tab_locked(tab: crate::TabId, holder: impl fmt::Display) -> Self {
        Self::new(
            ErrorCode::TabLocked,
            format!("tab {tab} is locked by {holder}"),
        )
        .with_owner(holder.to_string())
        .with_hint("Wait until the lock is released, or use another tab (tabs_open).")
    }
}

/// Backend failures become command errors automatically, so commands can use `?` on
/// `ctx.backend.*().await`.
impl From<BackendError> for CmdError {
    fn from(err: BackendError) -> Self {
        let message = err.to_string();
        match err {
            BackendError::TabNotFound(_) => CmdError::new(ErrorCode::TabNotFound, message)
                .with_hint("Call tabs_list to see the open tabs."),
            BackendError::NoTabs => CmdError::new(ErrorCode::NoTabs, message)
                .with_hint("Open a tab first with tabs_open."),
            BackendError::Timeout(_) => CmdError::new(ErrorCode::Timeout, message)
                .with_hint("The page may still be loading; wait and try again."),
            BackendError::ShuttingDown => CmdError::new(ErrorCode::ShuttingDown, message),
            // A raw CDP failure is unexpected for a well-formed command: report it as-is.
            BackendError::Cdp(_) => CmdError::new(ErrorCode::Internal, message),
        }
    }
}
