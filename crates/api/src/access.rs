//! Restricted WebSocket connections for plugins.
//!
//! Normal API clients are trusted (local only, no tokens; see the crate docs). A plugin is
//! different: it is third-party code that the user allowed to do *some* things. Its script
//! host connects to
//!
//! ```text
//! ws://127.0.0.1:<port>/v1/ws?plugin=<name>&token=<per-plugin token>
//! ```
//!
//! and the WebSocket server asks the [`PluginAuth`] service (installed by `bmcp-plugins` into
//! the registry's extensions as a [`PluginAuthHandle`]) whether the token is the current one of
//! that plugin. A wrong token is refused with 403 before the upgrade. A valid one gives a
//! [`PluginGrant`], and that connection then
//!
//! * runs commands as `Caller::Plugin { name }` and only those matching the grant's command
//!   globs (others fail with `NEEDS_APPROVAL`);
//! * starts with NO event subscriptions, and only receives events whose topic matches the
//!   grant's event globs, plus `plugin.command` events addressed to this plugin.
//!
//! The token is not what keeps a plugin in its box: plugin code runs in the script host's V8
//! isolate, which has no network, file or process access at all, only the one connection the
//! host opened for it. The token makes that connection's identity unforgeable (audit events
//! name the right plugin) and lets the browser revoke it when the plugin is disabled.

use std::sync::Arc;

use serde_json::Value;

/// Topic of palette commands sent to plugins: `{plugin, id}`.
pub const PLUGIN_COMMAND_TOPIC: &str = "plugin.command";

/// What one plugin connection may do. Built from the plugin's `plugin.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGrant {
    /// Plugin name (also the `Caller::Plugin` name).
    pub name: String,
    /// Command globs, e.g. `["page.*", "net.rule_add"]`.
    pub commands: Vec<String>,
    /// Event topic globs, e.g. `["tab.updated", "net.*"]`.
    pub events: Vec<String>,
}

impl PluginGrant {
    /// May this plugin run command `name` (dotted)?
    pub fn allows_command(&self, name: &str) -> bool {
        self.commands.iter().any(|g| glob_match(g, name))
    }

    /// Should this event reach the plugin? Its own palette commands always do.
    pub fn allows_event(&self, topic: &str, data: &Value) -> bool {
        if topic == PLUGIN_COMMAND_TOPIC {
            return data.get("plugin").and_then(Value::as_str) == Some(self.name.as_str());
        }
        self.events.iter().any(|g| glob_match(g, topic))
    }
}

/// Checks plugin tokens. Implemented by the plugin manager (`bmcp-plugins`).
pub trait PluginAuth: Send + Sync + 'static {
    /// The grant of `plugin` when `token` is its current token, else `None`.
    fn authenticate(&self, plugin: &str, token: &str) -> Option<PluginGrant>;
}

/// How the API finds the [`PluginAuth`] service: `registry.extensions().insert(PluginAuthHandle(..))`.
#[derive(Clone)]
pub struct PluginAuthHandle(pub Arc<dyn PluginAuth>);

/// `*` matches any run of characters (dots included); everything else is literal.
/// `"page.*"` matches `page.snapshot` and `page.a.b`, `"*"` matches everything,
/// `"net.rule_add"` only itself.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    // First part must be a prefix, last part a suffix, the middle ones appear in order.
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for middle in &parts[1..parts.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn globs() {
        assert!(glob_match("*", "anything.at.all"));
        assert!(glob_match("page.*", "page.snapshot"));
        assert!(glob_match("page.*", "page.a.b"));
        assert!(!glob_match("page.*", "pages.list"));
        assert!(!glob_match("page.*", "page"));
        assert!(glob_match("net.rule_add", "net.rule_add"));
        assert!(!glob_match("net.rule_add", "net.rule_add2"));
        assert!(glob_match("*.list", "tabs.list"));
        assert!(glob_match("a*b*c", "a__b__c"));
        assert!(!glob_match("a*b*c", "a__c__b"));
        assert!(!glob_match("ab*ba", "aba")); // prefix and suffix may not overlap
    }

    #[test]
    fn grants() {
        let grant = PluginGrant {
            name: "hello".into(),
            commands: vec!["page.*".into(), "tabs.list".into()],
            events: vec!["tab.*".into()],
        };
        assert!(grant.allows_command("page.find"));
        assert!(grant.allows_command("tabs.list"));
        assert!(!grant.allows_command("tabs.close"));
        assert!(!grant.allows_command("script.run"));
        assert!(grant.allows_event("tab.updated", &json!({})));
        assert!(!grant.allows_event("net.request", &json!({})));
        assert!(grant.allows_event("plugin.command", &json!({ "plugin": "hello", "id": "x" })));
        assert!(!grant.allows_event("plugin.command", &json!({ "plugin": "other", "id": "x" })));
    }
}
