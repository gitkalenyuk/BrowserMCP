//! Webhook triggers: `POST /v1/hooks/{id}` starts a saved script.
//!
//! ```text
//! trigger.webhook_set {script: "notify", secret?: "…", inputs_from_body?: true, inputs?: {...}}
//!   -> {id: "hook-3f2a…", path: "/v1/hooks/hook-3f2a…", secret: "…", …}
//!
//! curl -X POST http://127.0.0.1:47800/v1/hooks/hook-3f2a… \
//!      -H "X-BMCP-Signature: sha256=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')" \
//!      -d "$BODY"
//!   -> {"run_id": "run-…", "script": "notify"}
//! ```
//!
//! * The signature is HMAC-SHA256 of the exact body bytes with the hook's secret, as lowercase
//!   (or uppercase) hex, prefixed `sha256=` — the GitHub webhook format, so most senders (n8n,
//!   Zapier "custom webhook", GitHub itself) can produce it. Missing or wrong → 403.
//! * `inputs_from_body` (default true): a JSON object body becomes the script's `inputs`
//!   (values that are not strings arrive as their JSON text), on top of the hook's fixed
//!   `inputs`. Otherwise the raw body is passed as `inputs.body`.
//! * The script runs as a one-row mass run (`run.start`), so `run.status` / `run.results`
//!   show its progress and output; the HTTP answer comes right away with the `run_id`.
//! * Hooks live in `data/config/webhooks.json` (secrets in plain text, like the rest of the
//!   data folder by the user's choice).
//!
//! **Reaching it from outside.** The API listens on 127.0.0.1 only and checks that `Host` is a
//! loopback name, so a service on another machine (n8n cloud, GitHub, …) needs a tunnel that
//! forwards to `127.0.0.1:<api port>` and rewrites the Host header, e.g.
//! `cloudflared tunnel --url http://127.0.0.1:47800 --http-host-header 127.0.0.1` or
//! `ngrok http 47800 --host-header=rewrite`. Anyone who finds the tunnel URL reaches only what
//! the guard lets through; every hook call still needs the secret. Requests are not
//! protected against replay (a captured request can be sent again): use a fresh secret per
//! sender and rotate it if it leaks.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bmcp_core::{CmdError, Ctx, ErrorCode, Retry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{Inner, io_err, lock, now_ms, topics};

/// One webhook trigger as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub id: String,
    /// Saved script (`data/scripts/<name>.ts`).
    pub script: String,
    pub secret: String,
    pub inputs_from_body: bool,
    #[serde(default)]
    pub inputs: Map<String, Value>,
    pub created_ms: u64,
    #[serde(default)]
    pub last_fired_ms: Option<u64>,
    #[serde(default)]
    pub last_run_id: Option<String>,
}

/// A hook as listed (never with its secret).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HookInfo {
    pub id: String,
    /// `POST` here: `/v1/hooks/<id>`.
    pub path: String,
    pub script: String,
    pub inputs_from_body: bool,
    pub inputs: Map<String, Value>,
    pub created_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

impl Hook {
    pub fn info(&self) -> HookInfo {
        HookInfo {
            id: self.id.clone(),
            path: format!("/v1/hooks/{}", self.id),
            script: self.script.clone(),
            inputs_from_body: self.inputs_from_body,
            inputs: self.inputs.clone(),
            created_ms: self.created_ms,
            last_fired_ms: self.last_fired_ms,
            last_run_id: self.last_run_id.clone(),
        }
    }
}

/// The hooks, kept in memory and written to `webhooks.json` on every change.
pub struct HookStore {
    path: PathBuf,
    hooks: Mutex<Vec<Hook>>,
}

impl HookStore {
    pub fn open(path: &Path) -> HookStore {
        let hooks = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| serde_json::from_value(v["hooks"].clone()).ok())
            .unwrap_or_default();
        HookStore {
            path: path.to_path_buf(),
            hooks: Mutex::new(hooks),
        }
    }

    fn save(&self, hooks: &[Hook]) -> Result<(), CmdError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
        }
        let text = serde_json::to_string_pretty(&json!({ "hooks": hooks }))
            .map_err(|e| CmdError::internal(e.to_string()))?;
        // Write a temp file and rename it, so a crash never leaves half a file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| io_err(&tmp, e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| io_err(&self.path, e))
    }

    pub fn list(&self) -> Vec<HookInfo> {
        lock(&self.hooks).iter().map(Hook::info).collect()
    }

    /// Insert or replace (by id).
    pub fn put(&self, hook: Hook) -> Result<(), CmdError> {
        let mut hooks = lock(&self.hooks);
        hooks.retain(|h| h.id != hook.id);
        hooks.push(hook);
        self.save(&hooks)
    }

    pub fn remove(&self, id: &str) -> Result<bool, CmdError> {
        let mut hooks = lock(&self.hooks);
        let before = hooks.len();
        hooks.retain(|h| h.id != id);
        let removed = hooks.len() != before;
        if removed {
            self.save(&hooks)?;
        }
        Ok(removed)
    }

    pub fn get(&self, id: &str) -> Option<Hook> {
        lock(&self.hooks).iter().find(|h| h.id == id).cloned()
    }

    fn record_fired(&self, id: &str, run_id: &str) {
        let mut hooks = lock(&self.hooks);
        if let Some(hook) = hooks.iter_mut().find(|h| h.id == id) {
            hook.last_fired_ms = Some(now_ms());
            hook.last_run_id = Some(run_id.to_string());
        }
        if let Err(err) = self.save(&hooks) {
            tracing::warn!(%err, "cannot record webhook firing");
        }
    }
}

// ------------------------------------------------------------------ HMAC-SHA256 (RFC 2104)

/// HMAC-SHA256 of `message` with `key`. Small enough to write out instead of adding a crate:
/// `H((K ^ opad) || H((K ^ ipad) || message))` with the key padded (or hashed) to 64 bytes.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner_hash);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// The header value a sender must send for `body`: `sha256=<lowercase hex>`.
pub fn signature_for(secret: &str, body: &[u8]) -> String {
    format!(
        "sha256={}",
        crate::hex(&hmac_sha256(secret.as_bytes(), body))
    )
}

/// Does `header` (`sha256=<hex>`) sign `body` with `secret`? Constant-time comparison.
pub fn verify_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(given) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let expected = crate::hex(&hmac_sha256(secret.as_bytes(), body));
    crate::constant_time_eq(given.to_ascii_lowercase().as_bytes(), expected.as_bytes())
}

// ------------------------------------------------------------------ set / fire

/// Hook ids end up in a URL path: keep them simple.
fn validate_id(id: &str) -> Result<(), CmdError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "bad hook id {id:?}: use 1-64 chars of A-Z a-z 0-9 _ -"
        )))
    }
}

/// What `trigger.webhook_set` returns: the hook plus its secret (shown only here).
#[derive(Debug, Serialize, JsonSchema)]
pub struct HookCreated {
    #[serde(flatten)]
    pub hook: HookInfo,
    /// Give this to the sender; it signs every request with it. Not shown by webhook_list.
    pub secret: String,
}

impl Inner {
    pub fn set_hook(
        &self,
        id: Option<String>,
        script: String,
        secret: Option<String>,
        inputs_from_body: bool,
        inputs: Map<String, Value>,
    ) -> Result<HookCreated, CmdError> {
        let id = match id {
            Some(id) => id,
            None => format!("hook-{}", crate::random_hex(8)?),
        };
        validate_id(&id)?;
        // A saved script: same name rules as script.save, and the file must exist.
        validate_id(&script)?;
        let file = self.config.scripts_dir.join(format!("{script}.ts"));
        if !file.is_file() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("saved script {script} not found"),
            )
            .with_hint("Save it first with script.save {name, code}."));
        }
        let secret = match secret {
            Some(s) if s.len() >= 16 => s,
            Some(_) => {
                return Err(CmdError::invalid_params(
                    "secret must be at least 16 characters (or leave it out to get a random one)",
                ));
            }
            None => crate::random_hex(32)?,
        };
        let hook = Hook {
            id,
            script,
            secret: secret.clone(),
            inputs_from_body,
            inputs,
            created_ms: now_ms(),
            last_fired_ms: None,
            last_run_id: None,
        };
        let info = hook.info();
        self.hooks.put(hook)?;
        Ok(HookCreated { hook: info, secret })
    }

    /// Check the signature and start the run. `body` is the raw request body.
    pub async fn fire_hook(
        &self,
        ctx: &Ctx,
        id: &str,
        signature: &str,
        body: &str,
    ) -> Result<(String, String), CmdError> {
        let hook = self.hooks.get(id).ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("no webhook {id:?}")).with_retry(Retry::No)
        })?;
        if !verify_signature(&hook.secret, body.as_bytes(), signature) {
            tracing::warn!(hook = %id, "webhook refused: bad signature");
            return Err(CmdError::new(
                ErrorCode::NeedsApproval,
                "missing or wrong X-BMCP-Signature",
            )
            .with_retry(Retry::No)
            .with_hint("Send `X-BMCP-Signature: sha256=<hex HMAC-SHA256 of the body with the hook's secret>`."));
        }
        let row = hook_inputs(&hook, body)?;
        let registry = self.registry()?;
        let args = json!({
            "script": hook.script,
            "data": { "rows": [row] },
            "parallel": 1,
            "retries": 0,
            "heal": false,
        });
        let call_ctx = Ctx::new(ctx.caller.clone(), ctx.backend.clone(), ctx.events.clone());
        let run = registry.call(call_ctx, "run.start", args).await?;
        let run_id = run["id"]
            .as_str()
            .ok_or_else(|| CmdError::internal("run.start returned no id"))?
            .to_string();
        self.hooks.record_fired(id, &run_id);
        self.events.emit(
            topics::WEBHOOK_FIRED,
            json!({ "id": id, "script": hook.script, "run_id": run_id }),
        );
        Ok((run_id, hook.script))
    }
}

/// The script inputs for one call (see the module docs).
fn hook_inputs(hook: &Hook, body: &str) -> Result<Map<String, Value>, CmdError> {
    let mut row = hook.inputs.clone();
    if !hook.inputs_from_body {
        row.insert("body".into(), Value::String(body.to_string()));
        return Ok(row);
    }
    if body.trim().is_empty() {
        return Ok(row);
    }
    match serde_json::from_str::<Value>(body) {
        Ok(Value::Object(fields)) => {
            row.extend(fields);
            Ok(row)
        }
        Ok(_) => Err(CmdError::invalid_params(
            "the body must be a JSON object (its keys become the script's inputs)",
        )),
        Err(err) => Err(CmdError::invalid_params(format!(
            "the body is not valid JSON: {err}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // RFC 4231 test case 2.
        assert_eq!(
            crate::hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block size is hashed first.
        assert_eq!(
            crate::hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn signatures() {
        let body = br#"{"email":"a@b.test"}"#;
        let good = signature_for("0123456789abcdef", body);
        assert!(good.starts_with("sha256=") && good.len() == 7 + 64);
        assert!(verify_signature("0123456789abcdef", body, &good));
        assert!(verify_signature(
            "0123456789abcdef",
            body,
            &good.to_uppercase().replace("SHA256=", "sha256=")
        ));
        assert!(
            !verify_signature("0123456789abcdeX", body, &good),
            "other secret"
        );
        assert!(
            !verify_signature("0123456789abcdef", b"{}", &good),
            "other body"
        );
        assert!(
            !verify_signature("0123456789abcdef", body, &good[7..]),
            "no prefix"
        );
        assert!(!verify_signature("0123456789abcdef", body, ""), "missing");
        assert!(
            !verify_signature("0123456789abcdef", body, "sha256="),
            "empty"
        );
    }

    fn hook(from_body: bool) -> Hook {
        let mut inputs = Map::new();
        inputs.insert("fixed".into(), json!("1"));
        Hook {
            id: "h".into(),
            script: "s".into(),
            secret: "x".repeat(16),
            inputs_from_body: from_body,
            inputs,
            created_ms: 0,
            last_fired_ms: None,
            last_run_id: None,
        }
    }

    #[test]
    fn inputs_from_body_or_raw() {
        let row = hook_inputs(&hook(true), r#"{"email":"a@b.test","n":2}"#).unwrap();
        assert_eq!(
            json!(row),
            json!({ "fixed": "1", "email": "a@b.test", "n": 2 })
        );
        assert_eq!(
            json!(hook_inputs(&hook(true), "").unwrap()),
            json!({ "fixed": "1" })
        );
        assert!(hook_inputs(&hook(true), "[1,2]").is_err());
        assert!(hook_inputs(&hook(true), "{oops").is_err());
        let row = hook_inputs(&hook(false), "raw text").unwrap();
        assert_eq!(json!(row), json!({ "fixed": "1", "body": "raw text" }));
    }
}
