//! Change events of the data commands, published on the shared [`bmcp_core::EventBus`] so the
//! UI (bookmarks bar, omnibox star, library page) can re-read instead of polling.
//!
//! * `bookmarks.changed {op, id}` — `op` is `add`, `add_folder`, `update`, `remove`, `move` or
//!   `import`; `id` is the bookmark/folder that changed (`null` for `import`).
//! * `history.changed {op}` — `op` is `delete` or `import`.
//!
//! Only the commands emit them (an edit made by any caller: UI, REST, MCP, a script). Visits
//! recorded automatically by the [`crate::recorder`] do not.

use bmcp_core::Ctx;
use serde_json::json;

/// Topic of the bookmark change event.
pub const BOOKMARKS_CHANGED: &str = "bookmarks.changed";
/// Topic of the history change event.
pub const HISTORY_CHANGED: &str = "history.changed";

/// Emit `bookmarks.changed {op, id}`.
pub fn bookmarks_changed(ctx: &Ctx, op: &str, id: Option<i64>) {
    ctx.events
        .emit(BOOKMARKS_CHANGED, json!({ "op": op, "id": id }));
}

/// Emit `history.changed {op}`.
pub fn history_changed(ctx: &Ctx, op: &str) {
    ctx.events.emit(HISTORY_CHANGED, json!({ "op": op }));
}
