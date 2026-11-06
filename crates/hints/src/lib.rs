//! BrowserMCP Vim-style link hints (plan Phase 4, M4.2).
//!
//! The page is a native CEF view and the chrome a separate Svelte view, so labels are drawn
//! *inside the page*: `hints.js` runs in the isolated world `bmcp-hints`, finds the
//! interactive elements in the viewport and puts small yellow labels next to them, in a
//! closed shadow root of a top-layer popover (page CSS cannot break them, no layout shift).
//!
//! * `hints.show {tab?, mode: click|newtab|focus|copy}` — label the elements; then typing a
//!   label in the page picks it (letters filter, Backspace edits, Esc cancels);
//! * `hints.hide {tab?}`;
//! * `hints.config {enabled, keys, disabled_sites, disable_site, enable_site, alphabet}` —
//!   with hints enabled, `f` (click) and `F` (new tab) typed in a page show them too, unless
//!   a text field has focus, a modifier is held or the site is switched off;
//! * `clipboard.write {text}` — used by copy mode, usable on its own.
//!
//! Who does what: the page script finds elements, draws labels and reports keys through a CDP
//! binding; Rust picks the labels ([`labels`]), interprets typing ([`filter`]) and acts
//! ([`service`]): clicks and focus with trusted CDP input through the same code as
//! `page.act` (`bmcp_browser::perform_on_node`), new tabs with `open_tab`.
//!
//! The app calls [`start`] once so every tab gets the page script (for the in-page `f` key).
//! No CEF dependency; tested with `bmcp_core::testing::FakeBackend`.

pub mod commands;
pub mod config;
pub mod filter;
pub mod labels;
pub mod service;
#[cfg(test)]
mod tests;

use bmcp_core::Registry;

pub use config::{HintsConfig, Keys, Mode};
pub use service::{HintsService, start};

/// Register the `hints.*` and `clipboard.write` commands.
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::Show>()
        .add::<commands::Hide>()
        .add::<commands::Config>()
        .add::<commands::ClipboardWrite>();
}
