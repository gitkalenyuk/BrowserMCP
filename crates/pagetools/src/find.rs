//! Find in page: highlight every match of a text and scroll to the current one.
//!
//! The work is done by [`FIND_JS`] in an isolated world (`Page.createIsolatedWorld` +
//! `Runtime.evaluate {contextId}`), so page scripts cannot see or break it. Matches are
//! painted with the CSS Custom Highlight API (no `<mark>` elements inserted). Which match is
//! "current" is remembered here, per tab, so `next`/`prev` can step through them.
//!
//! Limits: a match must lie inside one text node ("foo <b>bar</b>" does not match "foo bar"),
//! at most 1000 matches, main frame only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bmcp_core::{CmdError, Ctx, TabId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::util::{cdp, main_frame};

/// The script; `(FIND_JS)(query, caseSensitive, wanted)` evaluates to the result object.
pub const FIND_JS: &str = include_str!("find.js");
/// Name of the isolated world (Chromium reuses the world for the same name).
pub const WORLD: &str = "bmcp-find";

/// The last search per tab.
#[derive(Default)]
pub struct FindState {
    tabs: Mutex<HashMap<TabId, Search>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Search {
    query: String,
    case_sensitive: bool,
    /// 0-based index of the current match.
    index: i64,
}

impl FindState {
    pub fn from_ctx(ctx: &Ctx) -> Arc<FindState> {
        ctx.extensions.get_or_insert_with(FindState::default)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<TabId, Search>> {
        self.tabs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Which match to show: the same search continues from its current match (`step` = +1
    /// next, -1 previous, 0 stay); a new query starts at the first match.
    fn wanted(&self, tab: TabId, query: &str, case_sensitive: bool, step: i64) -> i64 {
        match self.lock().get(&tab) {
            Some(last) if last.query == query && last.case_sensitive == case_sensitive => {
                last.index + step
            }
            // A new search: "previous" starts from the last match.
            _ if step < 0 => -1,
            _ => 0,
        }
    }

    fn remember(&self, tab: TabId, search: Search) {
        self.lock().insert(tab, search);
    }

    fn forget(&self, tab: TabId) {
        self.lock().remove(&tab);
    }
}

/// What the page script found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Found {
    /// Number of visible matches (at most 1000).
    pub count: u32,
    /// 0-based index of the current match.
    pub index: u32,
    /// Text around the current match.
    pub context: String,
    /// The page has no CSS highlight API (very old engine or a special page).
    #[serde(default)]
    pub unsupported: bool,
}

/// Search `query` on `tab` and move to match `step` (+1 / -1 / 0) relative to the current one.
pub async fn find_text(
    ctx: &Ctx,
    tab: TabId,
    query: &str,
    case_sensitive: bool,
    step: i64,
) -> Result<Found, CmdError> {
    let state = FindState::from_ctx(ctx);
    let wanted = state.wanted(tab, query, case_sensitive, step);
    let found = run(ctx, tab, json!([query, case_sensitive, wanted])).await?;
    if query.is_empty() {
        state.forget(tab);
    } else {
        state.remember(
            tab,
            Search {
                query: query.to_string(),
                case_sensitive,
                index: i64::from(found.index),
            },
        );
    }
    Ok(found)
}

/// Remove the highlights of `tab` and forget its search.
pub async fn clear(ctx: &Ctx, tab: TabId) -> Result<(), CmdError> {
    FindState::from_ctx(ctx).forget(tab);
    run(ctx, tab, json!(["", false, 0])).await?;
    Ok(())
}

/// Evaluate [`FIND_JS`] with `args` (a JSON array) in the isolated world.
async fn run(ctx: &Ctx, tab: TabId, args: Value) -> Result<Found, CmdError> {
    let (frame_id, _) = main_frame(ctx, tab).await?;
    let world = cdp(
        ctx,
        tab,
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "worldName": WORLD }),
    )
    .await?;
    // `args` is JSON, which is valid JavaScript: `(function…)(…args)` needs no escaping.
    let arguments = args.as_array().cloned().unwrap_or_default();
    let list: Vec<String> = arguments.iter().map(Value::to_string).collect();
    let expression = format!("({FIND_JS})({})", list.join(", "));
    let reply = cdp(
        ctx,
        tab,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "contextId": world["executionContextId"],
            "returnByValue": true,
        }),
    )
    .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("JavaScript exception");
        return Err(CmdError::internal(format!("find in page failed: {text}")));
    }
    serde_json::from_value(reply["result"]["value"].clone())
        .map_err(|e| CmdError::internal(format!("unexpected find result: {e}")))
}
