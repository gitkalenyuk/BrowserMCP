//! Identity for worker targets (dedicated, shared and service workers).
//!
//! The overrides of [`crate::stealth::apply`] belong to the *tab's* DevTools session and its
//! new-document script never runs in workers, so a worker shows the real navigator — CreepJS
//! compares the two. The fix lives in the `cef` crate (it owns the CDP sessions); this module
//! only says *what* to send:
//!
//! 1. On every tab session: `Target.setAutoAttach {autoAttach: true, waitForDebuggerOnStart:
//!    true, flatten: true}` (workers then start paused). Service workers belong to the browser
//!    target: also `Target.setAutoAttach` on the browser session (or `Target.setDiscoverTargets`
//!    + `Target.attachToTarget {flatten: true}` for `service_worker` targets).
//! 2. On `Target.attachedToTarget {sessionId, targetInfo: {type}, waitingForDebugger}`:
//!    look up the identity of the tab's profile (`Services::identity_for_profile`) and send
//!    each command of [`worker_commands`]`(&identity, &targetInfo.type, engine_version)` with
//!    that `sessionId`, **in order**. A failing command is never fatal: log it (or ignore it
//!    silently when [`WorkerCommand::optional`]) and continue.
//! 3. The last command is always `Runtime.runIfWaitingForDebugger`, which lets the worker
//!    start. Send it even when everything before it failed — otherwise the worker hangs. For
//!    targets this module does not handle (`page`, `iframe`, ...) the list holds only that
//!    command, so step 2 can run unconditionally for every auto-attached target.
//!
//! What reaches workers: the user agent, client hints and languages through
//! `Network.setUserAgentOverride` on the worker session; timezone/locale/CPU through
//! `Emulation.*` where the engine supports it on workers (optional); and the patch script as
//! a fallback for `hardwareConcurrency`, `deviceMemory`, `userAgent`, `appVersion`,
//! `platform`, `language(s)` and the WebGL strings of OffscreenCanvas. Not covered by the
//! fallback: the timezone (no clean JS patch for `Intl`) and `navigator.userAgentData`.

use serde_json::{Value, json};

use crate::identity::Identity;
use crate::stealth::{ua_override_params, worker_patch_script};

/// One CDP command for a worker session.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerCommand {
    pub method: &'static str,
    pub params: Value,
    /// Expected to fail on some engines or worker kinds; a failure needs no warning.
    pub optional: bool,
}

/// CDP target types that are workers.
pub fn is_worker(target_type: &str) -> bool {
    matches!(target_type, "worker" | "shared_worker" | "service_worker")
}

/// The commands that give a newly attached target `identity` (see the module docs). Pass
/// the engine's full version (`Browser.getVersion` → "156.0.7632.45") so the client hints
/// carry it like on the tab; `None` keeps the identity's stored version.
pub fn worker_commands(
    identity: &Identity,
    target_type: &str,
    engine_version: Option<&str>,
) -> Vec<WorkerCommand> {
    let resume = WorkerCommand {
        method: "Runtime.runIfWaitingForDebugger",
        params: json!({}),
        optional: false,
    };
    if !is_worker(target_type) {
        return vec![resume];
    }
    let mut identity = identity.clone();
    if let Some(engine) = engine_version {
        // A mismatch is already reported when the tab gets the identity.
        let _ = identity.adapt_to_engine(engine);
    }
    let cmd = |method, params, optional| WorkerCommand {
        method,
        params,
        optional,
    };
    vec![
        cmd(
            "Network.setUserAgentOverride",
            ua_override_params(&identity, true),
            false,
        ),
        cmd(
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": identity.timezone }),
            true,
        ),
        cmd(
            "Emulation.setLocaleOverride",
            json!({ "locale": identity.locale }),
            true,
        ),
        cmd(
            "Emulation.setHardwareConcurrencyOverride",
            json!({ "hardwareConcurrency": identity.hardware_concurrency }),
            true,
        ),
        // No Runtime.enable (detectable); evaluate works without it, also while paused.
        cmd(
            "Runtime.evaluate",
            json!({ "expression": worker_patch_script(&identity), "silent": true, "returnByValue": true }),
            false,
        ),
        resume,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Overrides, generate};

    #[test]
    fn workers_get_ua_emulation_patch_then_resume() {
        let id = generate(Some(crate::Os::Windows), 11, &Overrides::default());
        for kind in ["worker", "shared_worker", "service_worker"] {
            let cmds = worker_commands(&id, kind, Some("156.0.7632.45"));
            let methods: Vec<&str> = cmds.iter().map(|c| c.method).collect();
            assert_eq!(
                methods,
                [
                    "Network.setUserAgentOverride",
                    "Emulation.setTimezoneOverride",
                    "Emulation.setLocaleOverride",
                    "Emulation.setHardwareConcurrencyOverride",
                    "Runtime.evaluate",
                    "Runtime.runIfWaitingForDebugger",
                ],
                "{kind}"
            );
            let ua = &cmds[0].params;
            assert_eq!(ua["userAgent"], json!(id.user_agent));
            assert_eq!(ua["acceptLanguage"], json!(id.accept_language));
            assert_eq!(ua["platform"], "Win32");
            assert!(
                ua["userAgentMetadata"]["fullVersionList"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b["version"] == "156.0.7632.45"),
                "engine version in the client hints"
            );
            assert_eq!(cmds[1].params["timezoneId"], json!(id.timezone));
            assert!(cmds[1].optional && !cmds[0].optional && !cmds[5].optional);
            let src = cmds[4].params["expression"].as_str().unwrap();
            assert!(src.contains("WorkerNavigator"));
            assert!(src.contains(&format!(
                "\"hardwareConcurrency\":{}",
                id.hardware_concurrency
            )));
            assert!(src.contains(&format!("\"platform\":\"{}\"", id.platform)));
            // The worker has no screen: no avail* patch.
            assert!(src.contains("\"availHeight\":null"), "{src}");
        }
    }

    #[test]
    fn other_targets_are_only_resumed() {
        let id = generate(None, 1, &Overrides::default());
        for kind in ["page", "iframe", "browser", "other"] {
            let cmds = worker_commands(&id, kind, None);
            assert_eq!(cmds.len(), 1);
            assert_eq!(cmds[0].method, "Runtime.runIfWaitingForDebugger");
        }
        assert!(is_worker("service_worker") && !is_worker("page"));
    }
}
