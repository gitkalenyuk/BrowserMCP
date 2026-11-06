//! Role tables: which nodes get refs, which are kept as structure, which are flattened,
//! and how node states are printed.
//!
//! Role strings are the ones CDP reports: ARIA roles in lower case (`button`, `main`)
//! and Chrome-internal roles in PascalCase (`StaticText`, `RootWebArea`, `LabelText`).

use crate::ax::{AxNode, AxTree};
use crate::text::{MAX_VALUE_CHARS, clean, quote, truncate};

/// Roles that always get a ref: things an agent clicks, types into or toggles.
const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "option",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "treeitem",
    // Chrome-internal roles that are interactive too.
    "DisclosureTriangle", // <summary>
    "PopUpButton",
    "ToggleButton",
];

/// Roles that become interactive only when the page made them focusable
/// (`<div tabindex=0 onclick=...>`, focusable grid cells, clickable images...).
const CLICKISH_WHEN_FOCUSABLE: &[&str] = &[
    "generic",
    "image",
    "img",
    "listitem",
    "row",
    "cell",
    "gridcell",
    "columnheader",
    "rowheader",
    "figure",
    "LabelText",
    "paragraph",
];

/// Structure kept in interactive mode when it contains something kept:
/// landmarks and containers that tell the agent *where* a control is.
const STRUCTURE: &[&str] = &[
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "search",
    "dialog",
    "alertdialog",
    "list",
    "table",
    "grid",
    "treegrid",
    "row",
    "menu",
    "menubar",
    "tablist",
    "tabpanel",
    "tree",
    "toolbar",
    "radiogroup",
    "article",
    "feed",
    "MenuListPopup",
    // An <iframe> whose document was attached (`AxTree::attach_frame`).
    "Iframe",
];

/// Containers kept only when they have a name (`<form aria-label>`, `<fieldset>` with
/// a legend, named `<section>`). Unnamed ones are flattened.
const STRUCTURE_IF_NAMED: &[&str] = &["form", "group", "region", "Section"];

/// Kept even when nothing interactive is inside: headings and messages the agent must see.
const ALWAYS_KEEP: &[&str] = &["heading", "alert", "alertdialog", "status", "dialog"];

/// Kept as a wrapper only when it groups two or more kept children
/// (a product card: link + "Add to cart" button).
const GROUPING: &[&str] = &["listitem"];

/// Pure wrappers, flattened even in full mode unless they have a name.
const WRAPPERS: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "GenericContainer",
    "Section",
    "LabelText",
    "rowgroup",
];

/// Never printed and never walked into.
const SKIPPED: &[&str] = &["InlineTextBox", "LineBreak"];

/// Long lists of these roles collapse into a `→ @S:gN` group.
const COLLAPSIBLE: &[&str] = &[
    "list", "listbox", "menu", "tree", "table", "grid", "treegrid", "feed", "tablist",
];

/// Shorter or friendlier names for some CDP roles. `find` accepts both spellings.
const ALIASES: &[(&str, &str)] = &[
    ("contentinfo", "footer"),
    ("complementary", "aside"),
    ("navigation", "nav"),
    ("StaticText", "text"),
    ("image", "img"),
    ("RootWebArea", "document"),
    ("LabelText", "label"),
    ("MenuListPopup", "listbox"),
    ("DisclosureTriangle", "summary"),
    ("Iframe", "iframe"),
];

pub fn is_skipped(role: &str) -> bool {
    SKIPPED.contains(&role)
}

/// Does this node always deserve a ref because of its role?
pub fn has_interactive_role(node: &AxNode) -> bool {
    INTERACTIVE.contains(&node.role())
}

/// A focusable node with a "click-ish" role (custom buttons made of divs).
pub fn is_clickish_focusable(node: &AxNode) -> bool {
    CLICKISH_WHEN_FOCUSABLE.contains(&node.role()) && node.prop_true("focusable")
}

pub fn is_structure(role: &str, name: &str) -> bool {
    STRUCTURE.contains(&role) || (STRUCTURE_IF_NAMED.contains(&role) && !name.is_empty())
}

pub fn is_always_kept(role: &str) -> bool {
    ALWAYS_KEEP.contains(&role)
}

pub fn is_grouping(role: &str) -> bool {
    GROUPING.contains(&role)
}

pub fn is_wrapper(role: &str) -> bool {
    WRAPPERS.contains(&role)
}

/// `display` is the printed role (after aliasing).
pub fn is_collapsible(display: &str) -> bool {
    COLLAPSIBLE.contains(&display)
}

/// The role as printed: `contentinfo` -> `footer`, everything else unchanged.
pub fn display_role(role: &str) -> String {
    for (cdp, short) in ALIASES {
        if *cdp == role {
            return short.to_string();
        }
    }
    role.to_string()
}

/// Does a user-supplied role filter (`"footer"`, `"Button"`) match this CDP role?
pub fn role_matches(filter: &str, cdp_role: &str) -> bool {
    let filter = filter.to_lowercase();
    filter == cdp_role.to_lowercase() || filter == display_role(cdp_role).to_lowercase()
}

/// Roles whose current value is worth printing (`="jane@acme.com"`).
fn shows_value(role: &str) -> bool {
    matches!(
        role,
        "textbox" | "searchbox" | "combobox" | "spinbutton" | "slider"
    )
}

/// Words that mark a field as a password in several languages.
const PASSWORD_WORDS: &[&str] = &[
    "password",
    "passwort",
    "passcode",
    "пароль",
    "contraseña",
    "mot de passe",
    "senha",
    "hasło",
    "wachtwoord",
];

/// Is this a password field? We never print or search its value.
///
/// The AX tree has no "password" flag, so we combine hints:
/// 1. DOM info from the caller (`input_type == "password"`), the only reliable one;
/// 2. a `protected` property, in case a Chrome version exposes it;
/// 3. a value made only of mask characters (Chrome shows passwords as `•••`);
/// 4. an accessible name containing "password" (in several languages).
pub fn is_password(tree: &AxTree, node: &AxNode) -> bool {
    if !matches!(node.role(), "textbox" | "searchbox") {
        return false;
    }
    if let Some(backend) = node.backend_dom_node_id
        && let Some(layout) = tree.layout(backend)
        && layout.input_type.as_deref() == Some("password")
    {
        return true;
    }
    if node.prop_true("protected") {
        return true;
    }
    let value = node.value_text();
    if !value.is_empty() && value.chars().all(|c| c == '•' || c == '●' || c == '*') {
        return true;
    }
    let name = node.name().to_lowercase();
    PASSWORD_WORDS.iter().any(|w| name.contains(w))
}

/// The host part of a URL (`https://ads.example.com:8080/x` → `ads.example.com:8080`), or
/// the start of the URL when it has none (`about:blank`, `data:...`).
fn url_host(url: &str) -> String {
    match url.split_once("://") {
        Some((_, rest)) => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            // Drop `user:password@`.
            let host = authority.rsplit('@').next().unwrap_or_default();
            truncate(host, MAX_VALUE_CHARS)
        }
        None if url.is_empty() => "no src".to_string(),
        None => truncate(url, 20),
    }
}

/// Printed states for a node, in a fixed order, e.g. `["=\"jane\"", "req", "focused"]`.
pub fn states(tree: &AxTree, node: &AxNode, interactive: bool) -> Vec<String> {
    let role = node.role();
    let mut out = Vec::new();

    // Iframes: where the document inside comes from, `(ads.example.com)`.
    if let Some(url) = &node.frame_url {
        out.push(format!("({})", url_host(url)));
    }

    // Heading level: h1..h6.
    if role == "heading" {
        let level = node.prop_text("level");
        if !level.is_empty() {
            out.push(format!("h{level}"));
        }
    }

    // Value, or `pwd` for password fields (whose value is never printed).
    if is_password(tree, node) {
        out.push("pwd".to_string());
    } else if shows_value(role) {
        let value = clean(&node.value_text());
        if !value.is_empty() {
            out.push(format!("={}", quote(&truncate(&value, MAX_VALUE_CHARS))));
        }
    }

    // Checked: on / off / mixed.
    if let Some(checked) = node.prop("checked") {
        match checked.as_text().as_str() {
            "true" => out.push("on".to_string()),
            "false" => out.push("off".to_string()),
            "mixed" => out.push("mixed".to_string()),
            _ => {}
        }
    }
    if node.prop_true("pressed") {
        out.push("pressed".to_string());
    }
    if let Some(expanded) = node.prop("expanded") {
        out.push(
            if expanded.is_true() {
                "expanded"
            } else {
                "collapsed"
            }
            .to_string(),
        );
    }
    if node.prop_true("selected") {
        out.push("selected".to_string());
    }
    if node.prop_true("required") {
        out.push("req".to_string());
    }
    if node.prop_true("disabled") {
        out.push("disabled".to_string());
    }
    if node.prop_true("readonly") {
        out.push("readonly".to_string());
    }
    // `invalid` is a token: "false", "true", "spelling", "grammar".
    let invalid = node.prop_text("invalid");
    if !invalid.is_empty() && invalid != "false" {
        out.push("invalid".to_string());
    }
    if node.prop_true("focused") && role != "RootWebArea" {
        out.push("focused".to_string());
    }
    if interactive
        && let Some(backend) = node.backend_dom_node_id
        && let Some(layout) = tree.layout(backend)
        && layout.in_viewport == Some(false)
    {
        out.push("offscreen".to_string());
    }
    out
}
