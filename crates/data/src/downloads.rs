//! Downloads. The table is filled from the CEF download hook's `download.*` events
//! ([`spawn_download_recorder`] → [`upsert_download`]); `downloads.list` reads it.

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

// ------------------------------------------------------------------ recording from events

/// Topics the CEF download handler emits. Data: `{id (the browser's download id), url, path,
/// mime, total_bytes, received_bytes, state, started, ended, tab, run}`.
pub const DOWNLOAD_STARTED: &str = "download.started";
pub const DOWNLOAD_PROGRESS: &str = "download.progress";
pub const DOWNLOAD_DONE: &str = "download.done";

/// The row for a download event (`row_id` 0 = new row). `None` if the event has no URL.
pub fn download_from_event(data: &serde_json::Value, row_id: i64) -> Option<Download> {
    let s = |k: &str| data[k].as_str().unwrap_or_default().to_string();
    let url = s("url");
    if url.is_empty() {
        return None;
    }
    Some(Download {
        id: row_id,
        url,
        path: s("path"),
        mime: s("mime"),
        total_bytes: data["total_bytes"].as_i64().filter(|n| *n >= 0),
        received_bytes: data["received_bytes"].as_i64().unwrap_or(0),
        state: match data["state"].as_str() {
            Some(state @ ("complete" | "cancelled" | "interrupted")) => state.to_string(),
            _ => "in_progress".to_string(),
        },
        started: data["started"]
            .as_i64()
            .unwrap_or_else(crate::store::now_ms),
        ended: data["ended"].as_i64(),
    })
}

/// Keep the `downloads` table up to date from `download.*` events (one row per browser
/// download id). Runs until the bus closes.
pub fn spawn_download_recorder(
    store: DataStore,
    events: &bmcp_core::EventBus,
) -> tokio::task::JoinHandle<()> {
    let mut rx = events.subscribe();
    tokio::spawn(async move {
        // Browser download id → row id.
        let mut rows: std::collections::HashMap<u64, i64> = std::collections::HashMap::new();
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if ![DOWNLOAD_STARTED, DOWNLOAD_PROGRESS, DOWNLOAD_DONE].contains(&event.topic.as_str())
            {
                continue;
            }
            let Some(key) = event.data["id"].as_u64() else {
                continue;
            };
            let row_id = rows.get(&key).copied().unwrap_or(0);
            let Some(download) = download_from_event(&event.data, row_id) else {
                continue;
            };
            match store.run(move |c| upsert_download(c, &download)).await {
                Ok(id) => {
                    if event.topic == DOWNLOAD_DONE {
                        rows.remove(&key);
                    } else {
                        rows.insert(key, id);
                    }
                }
                Err(err) => tracing::warn!(%err, "download not recorded"),
            }
        }
    })
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

    #[tokio::test]
    async fn events_become_one_row_per_download() {
        let store = DataStore::open_in_memory().unwrap();
        let bus = bmcp_core::EventBus::new();
        let task = spawn_download_recorder(store.clone(), &bus);
        let base = json!({"id": 7, "url": "https://a.test/f.zip", "path": "D:/dl/f.zip",
                          "mime": "application/zip", "total_bytes": 100, "started": 1_000});
        let with = |extra: serde_json::Value| {
            let mut v = base.clone();
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        bus.emit(
            DOWNLOAD_STARTED,
            with(json!({"received_bytes": 0, "state": "in_progress"})),
        );
        bus.emit(DOWNLOAD_PROGRESS, with(json!({"received_bytes": 50})));
        bus.emit(
            DOWNLOAD_DONE,
            with(json!({"received_bytes": 100, "state": "complete", "ended": 2_000})),
        );
        // Let the recorder catch up.
        for _ in 0..100 {
            let rows = store.with(|c| list(c, 10)).unwrap();
            if rows.first().is_some_and(|r| r.state == "complete") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let rows = store.with(|c| list(c, 10)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].received_bytes, 100);
        assert_eq!(rows[0].ended, Some(2_000));
        assert_eq!(rows[0].path, "D:/dl/f.zip");
        task.abort();
        assert!(download_from_event(&json!({"id": 1}), 0).is_none());
    }
}
