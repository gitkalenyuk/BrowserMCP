//! Saved passwords.
//!
//! **NOT ENCRYPTED.** Passwords are stored as plain text in the `passwords` table of
//! `data/db/browsermcp.sqlite`. This is the user's explicit choice (docs/plan.md, "Секрети":
//! the AI has full access, no encryption). Anyone who can read the `data/` folder can read the
//! passwords. Do not add "light" obfuscation that would only pretend otherwise.
//!
//! What we do instead: listings hide the password unless the caller asks (`reveal: true`), and
//! the commands that return or store a password are marked [`Sensitivity::Secret`]
//! (`bmcp_core::Sensitivity`) so API layers can show approval prompts.
//!
//! Entries are keyed by `(origin, username)`; the origin is normalized to
//! `scheme://host[:port]` (`https://github.com/login` → `https://github.com`).

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{DataError, DataResult, DataStore, now_ms};

/// `https://GitHub.com/login?x` → `https://github.com`; `github.com` → `https://github.com`.
/// Origins without a host (e.g. Chrome's `android://…` entries) are kept as given.
pub fn normalize_origin(input: &str) -> DataResult<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(DataError::Invalid("origin must not be empty".into()));
    }
    let parsed = url::Url::parse(input).or_else(|_| url::Url::parse(&format!("https://{input}")));
    match parsed {
        Ok(url) if url.has_host() && matches!(url.scheme(), "http" | "https") => {
            Ok(url.origin().ascii_serialization())
        }
        Ok(_) => Ok(input.to_owned()),
        Err(e) => Err(DataError::Invalid(format!("invalid origin {input:?}: {e}"))),
    }
}

/// One saved login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PasswordEntry {
    pub id: i64,
    /// `scheme://host[:port]`.
    pub origin: String,
    pub username: String,
    /// The password in plain text — only when revealed, otherwise `null`.
    pub password: Option<String>,
    pub note: String,
    /// Unix ms.
    pub created: i64,
    pub updated: i64,
}

/// Save (insert, or update the password of the same origin + username). Returns
/// `(id, created)`.
pub fn save(
    conn: &Connection,
    origin: &str,
    username: &str,
    password: &str,
    note: &str,
    now: i64,
) -> DataResult<(i64, bool)> {
    let origin = normalize_origin(origin)?;
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM passwords WHERE origin = ?1 AND username = ?2",
            params![origin, username],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE passwords SET password = ?2, note = CASE WHEN ?3 <> '' THEN ?3 ELSE note END,
                     updated = ?4
                 WHERE id = ?1",
                params![id, password, note, now],
            )?;
            Ok((id, false))
        }
        None => {
            conn.execute(
                "INSERT INTO passwords (origin, username, password, note, created, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![origin, username, password, note, now],
            )?;
            Ok((conn.last_insert_rowid(), true))
        }
    }
}

/// Entries for `origin` (all when `None`), sorted by origin and username. The password is
/// filled in only when `reveal` is true.
pub fn list(
    conn: &Connection,
    origin: Option<&str>,
    reveal: bool,
) -> DataResult<Vec<PasswordEntry>> {
    let origin = origin.map(normalize_origin).transpose()?;
    let mut stmt = conn.prepare(
        "SELECT id, origin, username, password, note, created, updated FROM passwords
         WHERE ?1 IS NULL OR origin = ?1
         ORDER BY origin, username, id",
    )?;
    let rows = stmt
        .query_map([origin], |r| {
            let password: String = r.get(3)?;
            Ok(PasswordEntry {
                id: r.get(0)?,
                origin: r.get(1)?,
                username: r.get(2)?,
                password: reveal.then_some(password),
                note: r.get(4)?,
                created: r.get(5)?,
                updated: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete(conn: &Connection, id: i64) -> DataResult<()> {
    match conn.execute("DELETE FROM passwords WHERE id = ?1", [id])? {
        0 => Err(DataError::NotFound(format!("password {id} not found"))),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize, JsonSchema)]
pub struct EntriesOutput {
    pub entries: Vec<PasswordEntry>,
}

/// `passwords.list`.
pub struct List;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Only this site (URL or origin). Default: all sites.
    pub origin: Option<String>,
    /// Include the plain-text passwords (default false: `password` is null).
    #[serde(default)]
    pub reveal: bool,
}

impl Command for List {
    const NAME: &'static str = "passwords.list";
    type Args = ListArgs;
    type Output = EntriesOutput;

    fn meta() -> Meta {
        Meta::new(
            "List saved logins (origin, username, note, times). Passwords are null unless \
             reveal=true.",
            Toolset::Data,
        )
        // It can reveal passwords, so the whole command is Secret.
        .secret()
        .palette("Passwords")
    }

    async fn run(ctx: Ctx, args: ListArgs) -> Result<EntriesOutput, CmdError> {
        let entries = DataStore::from_ctx(&ctx)?
            .run(move |c| list(c, args.origin.as_deref(), args.reveal))
            .await?;
        Ok(EntriesOutput { entries })
    }
}

/// `passwords.get`.
pub struct Get;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    /// Site URL or origin, e.g. `https://github.com/login`.
    pub origin: String,
}

impl Command for Get {
    const NAME: &'static str = "passwords.get";
    type Args = GetArgs;
    type Output = EntriesOutput;

    fn meta() -> Meta {
        Meta::new(
            "Get the saved logins for a site, WITH plain-text passwords (for signing in). \
             Returns {entries:[{id,origin,username,password,…}]}, empty when none are saved.",
            Toolset::Data,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: GetArgs) -> Result<EntriesOutput, CmdError> {
        let entries = DataStore::from_ctx(&ctx)?
            .run(move |c| list(c, Some(&args.origin), true))
            .await?;
        Ok(EntriesOutput { entries })
    }
}

/// `passwords.save`.
pub struct Save;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveArgs {
    /// Site URL or origin.
    pub origin: String,
    pub username: String,
    pub password: String,
    /// Optional free text.
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SaveOutput {
    pub id: i64,
    /// The normalized origin it was saved under.
    pub origin: String,
    /// False when an existing login (same origin + username) was updated.
    pub created: bool,
}

impl Command for Save {
    const NAME: &'static str = "passwords.save";
    type Args = SaveArgs;
    type Output = SaveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Save a login (stored UNENCRYPTED). Same origin + username updates the password. \
             Returns {id, origin, created}.",
            Toolset::Data,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: SaveArgs) -> Result<SaveOutput, CmdError> {
        let origin = normalize_origin(&args.origin)?;
        let saved_origin = origin.clone();
        let (id, created) = DataStore::from_ctx(&ctx)?
            .run(move |c| {
                let note = args.note.unwrap_or_default();
                save(c, &origin, &args.username, &args.password, &note, now_ms())
            })
            .await?;
        Ok(SaveOutput {
            id,
            origin: saved_origin,
            created,
        })
    }
}

/// `passwords.delete`.
pub struct Delete;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// Entry id from passwords_list.
    pub id: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteOutput {
    pub deleted: bool,
}

impl Command for Delete {
    const NAME: &'static str = "passwords.delete";
    type Args = DeleteArgs;
    type Output = DeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete a saved login by id. Returns {deleted: true}.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: DeleteArgs) -> Result<DeleteOutput, CmdError> {
        DataStore::from_ctx(&ctx)?
            .run(move |c| delete(c, args.id))
            .await
            .map_err(|e| CmdError::from(e).with_hint("Call passwords_list to see the ids."))?;
        Ok(DeleteOutput { deleted: true })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tests::{call, call_err, setup};
    use bmcp_core::{ErrorCode, Sensitivity};

    #[test]
    fn origins_are_normalized() {
        assert_eq!(
            normalize_origin("https://GitHub.com/login?x=1").unwrap(),
            "https://github.com"
        );
        assert_eq!(
            normalize_origin("github.com").unwrap(),
            "https://github.com"
        );
        assert_eq!(
            normalize_origin("http://localhost:8080/a").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            normalize_origin("android://abc==@com.example.app/").unwrap(),
            "android://abc==@com.example.app/"
        );
        assert!(normalize_origin("  ").is_err());
    }

    #[tokio::test]
    async fn save_list_get_delete() {
        let (registry, ctx) = setup();
        let out = call(
            &registry,
            &ctx,
            "passwords.save",
            json!({"origin": "https://github.com/login", "username": "me", "password": "hunter2"}),
        )
        .await;
        assert_eq!(out["origin"], "https://github.com");
        assert_eq!(out["created"], true);
        let id = out["id"].as_i64().unwrap();
        // Same origin + username → update.
        let out = call(
            &registry,
            &ctx,
            "passwords.save",
            json!({"origin": "github.com", "username": "me", "password": "hunter3"}),
        )
        .await;
        assert_eq!(out["created"], false);
        assert_eq!(out["id"], id);
        call(
            &registry,
            &ctx,
            "passwords.save",
            json!({"origin": "https://example.org", "username": "x", "password": "y"}),
        )
        .await;

        // Not revealed by default.
        let out = call(&registry, &ctx, "passwords.list", json!({})).await;
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e["password"].is_null()));
        assert!(!out.to_string().contains("hunter"));

        let out = call(
            &registry,
            &ctx,
            "passwords.list",
            json!({"origin": "https://github.com/x", "reveal": true}),
        )
        .await;
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["entries"][0]["password"], "hunter3");

        let out = call(
            &registry,
            &ctx,
            "passwords.get",
            json!({"origin": "https://github.com"}),
        )
        .await;
        assert_eq!(out["entries"][0]["username"], "me");
        assert_eq!(out["entries"][0]["password"], "hunter3");
        let out = call(
            &registry,
            &ctx,
            "passwords.get",
            json!({"origin": "https://nothing.test"}),
        )
        .await;
        assert_eq!(out["entries"], json!([]));

        let out = call(&registry, &ctx, "passwords.delete", json!({"id": id})).await;
        assert_eq!(out, json!({"deleted": true}));
        let err = call_err(&registry, &ctx, "passwords.delete", json!({"id": id})).await;
        assert_eq!(err.code, ErrorCode::NotFound);

        // Commands that return or take secrets are marked as such.
        for name in ["passwords.list", "passwords.get", "passwords.save"] {
            assert_eq!(
                registry.get(name).unwrap().sensitivity,
                Sensitivity::Secret,
                "{name}"
            );
        }
    }
}
