//! BrowserMCP daily-driver data: bookmarks, history, downloads, passwords and the Chrome import,
//! all in one SQLite file, `data/db/browsermcp.sqlite` (see [`store`] for the schema).
//!
//! * [`bookmarks`] — `data.bookmarks`, `bookmarks.add|add_folder|remove|move|search`;
//! * [`history`] — `history.search|delete`, `data.top_sites` (frecency); visits are recorded
//!   automatically by [`recorder`] from `tab.updated` / `nav.goto` events;
//! * [`downloads`] — `downloads.list` (filled from the CEF download events by [`downloads::spawn_download_recorder`]);
//! * [`bookmarks`] — `data.bookmarks`, `bookmarks.add|add_folder|update|remove|move|search`;
//! * [`history`] — `history.search|delete` (delete = visits in a window / of a day / of a URL),
//!   `data.top_sites` (frecency); visits are recorded automatically by [`recorder`] from
//!   `tab.updated` / `nav.goto` events;
//! * [`downloads`] — `downloads.list` (filled by a later CEF hook);
//! * [`passwords`] — `passwords.list|get|save|delete`. **Stored unencrypted** by the user's
//!   choice (see the module docs);
//! * [`import`] — `import.chrome`, `import.chrome_detect`.
//!
//! Wiring (done in `crates/app`):
//!
//! ```no_run
//! # fn demo(data: &bmcp_core::DataDir, events: &bmcp_core::EventBus) -> Result<(), bmcp_data::DataError> {
//! let mut registry = bmcp_core::Registry::new();
//! let store = bmcp_data::DataStore::open_in(data)?;
//! registry.extensions().insert(store.clone());
//! bmcp_data::register(&mut registry);
//! // Inside the tokio runtime, with the same EventBus the API layer uses:
//! bmcp_data::spawn_history_recorder(store, events, None);
//! # Ok(()) }
//! ```
//!
//! All commands are in [`bmcp_core::Toolset::Data`]. Times are Unix milliseconds. Edits emit
//! `bookmarks.changed {op, id}` / `history.changed {op}` on the event bus ([`events`]).

pub mod bookmarks;
pub mod downloads;
pub mod events;
pub mod history;
pub mod import;
pub mod passwords;
pub mod recorder;
pub mod store;
#[cfg(test)]
mod tests;

use bmcp_core::Registry;

pub use recorder::{HistoryRecorder, spawn_history_recorder};
pub use store::{BOOKMARKS_BAR, DataError, DataResult, DataStore, OTHER_BOOKMARKS};

/// Register every data command. The [`DataStore`] itself must be put into
/// `registry.extensions()` by the app (it needs the data folder); without it the commands fail
/// with `INTERNAL: the data store … is not open`.
pub fn register(registry: &mut Registry) {
    registry
        .add::<bookmarks::List>()
        .add::<bookmarks::Add>()
        .add::<bookmarks::AddFolder>()
        .add::<bookmarks::Update>()
        .add::<bookmarks::Remove>()
        .add::<bookmarks::Move>()
        .add::<bookmarks::Search>()
        .add::<history::Search>()
        .add::<history::Delete>()
        .add::<history::TopSites>()
        .add::<downloads::List>()
        .add::<passwords::List>()
        .add::<passwords::Get>()
        .add::<passwords::Save>()
        .add::<passwords::Delete>()
        .add::<import::Detect>()
        .add::<import::Import>();
}
