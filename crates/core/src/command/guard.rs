//! Command guards: things outside a command that can stop it.
//!
//! Example: a JavaScript `alert()` freezes the page, so every `page.*` call would hang until
//! its CDP timeout. The page tools crate plugs a guard into the [`Registry`](super::Registry)
//! with `add_guard`; the guard watches for open dialogs and turns "hangs for 30 s" into an
//! immediate `DIALOG_OPEN` error that says which dialog is open.
//!
//! How it runs: for every call the registry asks each guard for a "watch" future. The command
//! and the watch futures run side by side; if a watch future finishes first, the command is
//! dropped (cancelled) and the watch's error is returned. Watch futures are polled before the
//! command, so a guard whose condition already holds stops the command before it does anything.

use serde_json::Value;

use super::{CmdError, Ctx};
use crate::backend::BoxFuture;

/// Watches one call. See the [module docs](self).
pub trait CommandGuard: Send + Sync + 'static {
    /// `None` = this guard does not care about the command (e.g. wrong name).
    /// `Some(future)` = run the command only as long as `future` is pending; when it
    /// finishes, the command is cancelled and fails with the returned error.
    fn watch(&self, ctx: &Ctx, name: &'static str, args: &Value)
    -> Option<BoxFuture<'static, CmdError>>;
}
