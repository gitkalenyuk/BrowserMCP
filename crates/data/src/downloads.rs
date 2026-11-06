//! Downloads. The table is filled by a later CEF download hook (`OnBeforeDownload` /
//! `OnDownloadUpdated` → [`upsert_download`]); for now there is only `downloads.list`.

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{DataResult, DataStore};

/// One download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Download {
    /// Row id. 0 in [`upsert_download`] means "new row".
    pub id: i64,
    pub url: String,
    /// Target file on disk.
    pub path: String,
    pub mime: String,
    /// Size in bytes when the server said it.
    pub total_bytes: Option<i64>,
    pub received_bytes: i64,
    /// `in_progress`, `complete`, `cancelled` or `interrupted`.
    pub state: String,
    /// Unix ms.
    pub started: i64,
    /// Unix ms; `null` while in progress.
    pub ended: Option<i64>,
}

/// Insert (`id == 0`) or update a download; returns its id. For the CEF download hook.
pub fn upsert_download(conn: &Connection, d: &Download) -> DataResult<i64> {
    if d.id == 0 {
        conn.execute(
            "INSERT INTO downloads (url, path, mime, total_bytes, received_bytes, state, started, ended)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![d.url, d.path, d.mime, d.total_bytes, d.received_bytes, d.state, d.started, d.ended],
        )?;
        Ok(conn.last_insert_rowid())
    } else {
        conn.execute(
            "UPDATE downloads SET url = ?2, path = ?3, mime = ?4, total_bytes = ?5,
                 received_bytes = ?6, state = ?7, started = ?8, ended = ?9
             WHERE id = ?1",
            params![
                d.id,
                d.url,
                d.path,
                d.mime,
                d.total_bytes,
                d.received_bytes,
                d.state,
                d.started,
                d.ended
            ],
        )?;
        Ok(d.id)
    }
}

/// Newest first.
pub fn list(conn: &Connection, limit: usize) -> DataResult<Vec<Download>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, path, mime, total_bytes, received_bytes, state, started, ended
         FROM downloads ORDER BY started DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(Download {
                id: r.get(0)?,
                url: r.get(1)?,
                path: r.get(2)?,
                mime: r.get(3)?,
                total_bytes: r.get(4)?,
                received_bytes: r.get(5)?,
                state: r.get(6)?,
                started: r.get(7)?,
                ended: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `downloads.list`.
pub struct List;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Max results (default 100, max 5000).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Newest first.
    pub downloads: Vec<Download>,
}

impl Command for List {
    const NAME: &'static str = "downloads.list";
    type Args = ListArgs;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List downloads, newest first: url, path, mime, sizes, state, start/end time.",
            Toolset::Data,
        )
        .palette("Downloads")
    }

    async fn run(ctx: Ctx, args: ListArgs) -> Result<ListOutput, CmdError> {
        let limit = args.limit.unwrap_or(100).clamp(1, 5000);
        let downloads = DataStore::from_ctx(&ctx)?
            .run(move |c| list(c, limit))
            .await?;
        Ok(ListOutput { downloads })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tests::{call, setup};

    #[tokio::test]
    async fn list_shows_newest_first() {
        let (registry, ctx) = setup();
        let out = call(&registry, &ctx, "downloads.list", json!({})).await;
        assert_eq!(out, json!({"downloads": []}));

        let store = DataStore::from_ctx(&ctx).unwrap();
        let mut d = Download {
            id: 0,
            url: "https://a.test/f.zip".into(),
            path: "C:/Downloads/f.zip".into(),
            mime: "application/zip".into(),
            total_bytes: Some(10),
            received_bytes: 0,
            state: "in_progress".into(),
            started: 1_000,
            ended: None,
        };
        d.id = store.with(|c| upsert_download(c, &d)).unwrap();
        d.received_bytes = 10;
        d.state = "complete".into();
        d.ended = Some(2_000);
        store.with(|c| upsert_download(c, &d)).unwrap();
        let newer = Download {
            id: 0,
            started: 5_000,
            ..d.clone()
        };
        store.with(|c| upsert_download(c, &newer)).unwrap();

        let out = call(&registry, &ctx, "downloads.list", json!({})).await;
        let list = out["downloads"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["started"], 5_000);
        assert_eq!(list[1]["state"], "complete");
        assert_eq!(list[1]["ended"], 2_000);
    }
}
