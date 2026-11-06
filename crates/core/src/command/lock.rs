//! Tab locks: which caller may change which tab.
//!
//! The real policy (agent groups, human takeover) is [`crate::agents::Agents`]; the built-in
//! commands install it (`register_builtin`). A bare [`Registry`](super::Registry) uses
//! [`AllowAll`].
//!
//! The registry talks to the policy at these points of every call:
//!
//! 1. [`LockPolicy::started`] — a known command is about to be validated and run (activity);
//! 2. [`LockPolicy::acquire`] — the command mutates a tab: may `caller` change `tab`? A policy
//!    may also *claim* the tab here (first agent to touch a tab owns it);
//! 3. [`LockPolicy::finished`] — the call is over (also after a refusal).
//!
//! Commands call [`LockPolicy::opened`] through `Ctx::tab_opened` after opening a tab, and
//! `Ctx::resolve_tab` asks [`LockPolicy::default_tab`] which tab to use when none was given.

use super::{Caller, CmdError, Ctx};
use crate::backend::TabId;

/// Decides whether `caller` may run a tab-mutating command on `tab`.
///
/// `tab` is `None` when the command did not name a tab (for example `tabs.open`). Return
/// `Err(CmdError::tab_locked(..))` to refuse. Every method except `check` has a do-nothing
/// default, so a simple policy only implements `check`.
pub trait LockPolicy: Send + Sync + 'static {
    /// Pure decision, no side effects.
    fn check(&self, caller: &Caller, tab: Option<TabId>) -> Result<(), CmdError>;

    /// Decision + bookkeeping (claiming the tab, human takeover). Default: [`Self::check`].
    fn acquire(&self, ctx: &Ctx, tab: Option<TabId>) -> Result<(), CmdError> {
        self.check(&ctx.caller, tab)
    }

    /// `ctx.caller` just opened `tab` (`tabs.open`).
    fn opened(&self, _ctx: &Ctx, _tab: TabId) {}

    /// The tab a command should use when the caller gave none (e.g. the agent's own tab).
    /// `None` = the registry default (most recently opened tab).
    fn default_tab(&self, _caller: &Caller) -> Option<TabId> {
        None
    }

    /// Command `name` is about to run for `ctx.caller`.
    fn started(&self, _ctx: &Ctx, _name: &str) {}

    /// Command `name` is over (`ok` = it returned a result).
    fn finished(&self, _ctx: &Ctx, _name: &str, _ok: bool) {}
}

/// Lets everything through.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl LockPolicy for AllowAll {
    fn check(&self, _caller: &Caller, _tab: Option<TabId>) -> Result<(), CmdError> {
        Ok(())
    }
}
