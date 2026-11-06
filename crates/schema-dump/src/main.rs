//! `bmcp-schema [--hidden]`: print `{"commands":[CommandInfo…]}` for the full registry.
//!
//! Same shape as `GET /v1/commands`, but without starting a browser. `--hidden` also lists the
//! commands of the `hidden` toolset. `cargo xtask gen-sdk` turns this into TypeScript.

use bmcp_core::Toolset;
use serde_json::json;

fn main() {
    let hidden = std::env::args().skip(1).any(|a| a == "--hidden");
    let registry = bmcp_registry::build_registry();
    let commands = if hidden {
        registry.list(Some(&Toolset::ALL))
    } else {
        registry.list(None)
    };
    let dump = json!({ "commands": commands });
    // Pretty-printed so the dump is readable and diffs are small.
    println!(
        "{}",
        serde_json::to_string_pretty(&dump).expect("CommandInfo is always serializable")
    );
}
