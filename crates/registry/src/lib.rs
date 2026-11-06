//! The one place where every command crate is registered.
//!
//! Both the browser (`crates/app`) and the schema dump (`bmcp-schema`, used by
//! `cargo xtask gen-sdk`) call [`build_registry`], so the generated TypeScript SDK always
//! matches what the running browser exposes.
//!
//! Adding a command crate: add it to this crate's `Cargo.toml` and add ONE `register` line
//! below. Nothing else (app, API surfaces, SDK generator) needs to change.

use bmcp_core::Registry;

/// A registry with every command of the browser.
pub fn build_registry() -> Registry {
    let mut registry = Registry::new();
    bmcp_core::commands::register_builtin(&mut registry);
    bmcp_api::commands::register(&mut registry);
    bmcp_browser::register(&mut registry);
    bmcp_data::register(&mut registry);
    bmcp_net::register(&mut registry);
    bmcp_identity::register(&mut registry);
    bmcp_ai::register(&mut registry);
    bmcp_record::register(&mut registry);
    bmcp_adblock::register(&mut registry);
    bmcp_jobs::register(&mut registry);
    bmcp_profiles::register(&mut registry);
    bmcp_plugins::register(&mut registry);
    bmcp_governor::register(&mut registry);
    bmcp_pagetools::register(&mut registry);
    bmcp_heal::register(&mut registry);
    bmcp_notify::register(&mut registry);
    bmcp_hints::register(&mut registry);
    // New command crates go here, one line each (e.g. `bmcp_net::register(&mut registry);`).
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_the_builtin_and_page_commands() {
        let registry = build_registry();
        for name in [
            "tabs.list",
            "nav.goto",
            "page.snapshot",
            "page.ax_tree",
            "bookmarks.add",
            "page.dialog",
            "page.upload",
            "page.pdf",
            "page.find_text",
            "hints.show",
        ] {
            assert!(registry.get(name).is_some(), "{name} missing");
        }
    }
}
