//! Tab locks: which caller may change which tab.
//!
//! The real policy (agent groups, human takeover) lives elsewhere and is plugged into the
//! [`Registry`](super::Registry) with `set_lock_policy`. Until then [`AllowAll`] is used.

use super::{Caller, CmdError};
use crate::backend::TabId;

/// Decides whether `caller` may run a tab-mutating command on `tab`.
///
/// `tab` is `None` when the command did not name a tab (for example `tabs.open`). Return
/// `Err(CmdError::tab_locked(..))` to refuse.
pub trait LockPolicy: Send + Sync + 'static {
    fn check(&self, caller: &Caller, tab: Option<TabId>) -> Result<(), CmdError>;
}

/// Lets everything through.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl LockPolicy for AllowAll {
    fn check(&self, _caller: &Caller, _tab: Option<TabId>) -> Result<(), CmdError> {
        Ok(())
    }
}
