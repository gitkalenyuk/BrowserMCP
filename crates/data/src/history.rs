//! Browsing history: `urls` (one row per URL, with `visit_count` / `last_visit`) and `visits`
//! (one row per page load).
//!
//! Visits are recorded automatically by the [`crate::recorder`]; only `http`, `https` and `file`
//! URLs are kept (no `about:blank`, `bmcp://ui`, `devtools://`, `data:` …). A second load of the
//! same URL within [`DEDUPE_MS`] is not a new visit (it only refreshes the title).
//!
//! Commands: `history.search`, `history.delete`, `data.top_sites` (frecency).

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{DataError, DataResult, DataStore, like_pattern, now_ms};

/// Loads of the same URL closer together than this count as one visit.
pub const DEDUPE_MS: i64 = 30_000;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether a URL belongs in the history.
pub fn is_recordable(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    ["http://", "https://", "file://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// Record a visit of `url` at `time` (Unix ms). Returns `false` when it was skipped: not a
/// recordable URL, or the same URL was visited less than [`DEDUPE_MS`] before/after.
/// A non-empty `title` replaces the stored one either way.
pub fn record_visit(
    conn: &mut Connection,
    url: &str,
    title: &str,
    transition: &str,
    time: i64,
) -> DataResult<bool> {
    if !is_recordable(url) {
        return Ok(false);
    }
    let tx = conn.transaction()?;
    let existing: Option<(i64, i64)> = tx
        .query_row(
            "SELECT id, last_visit FROM urls WHERE url = ?1",
            [url],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let recorded = match existing {
        Some((id, last)) if (time - last).abs() < DEDUPE_MS => {
            set_title(&tx, id, title)?;
            false
        }
        Some((id, _)) => {
            let added = tx.execute(
                "INSERT OR IGNORE INTO visits (url_id, visit_time, transition) VALUES (?1, ?2, ?3)",
                params![id, time, transition],
            )?;
            tx.execute(
                "UPDATE urls SET visit_count = visit_count + ?2, last_visit = MAX(last_visit, ?3)
                 WHERE id = ?1",
                params![id, added as i64, time],
            )?;
            set_title(&tx, id, title)?;
            added > 0
        }
        None => {
            tx.execute(
                "INSERT INTO urls (url, title, visit_count, last_visit) VALUES (?1, ?2, 1, ?3)",
                params![url, title.trim(), time],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO visits (url_id, visit_time, transition) VALUES (?1, ?2, ?3)",
                params![id, time, transition],
            )?;
            true
        }
    };
    tx.commit()?;
    Ok(recorded)
}

fn set_title(conn: &Connection, url_id: i64, title: &str) -> DataResult<()> {
    let title = title.trim();
    if !title.is_empty() {
        conn.execute(
            "UPDATE urls SET title = ?2 WHERE id = ?1",
            params![url_id, title],
        )?;
    }
    Ok(())
}

/// Set the title of a URL already in the history (titles often arrive after the load ends).
pub fn update_title(conn: &Connection, url: &str, title: &str) -> DataResult<()> {
    let title = title.trim();
    if !title.is_empty() {
        conn.execute(
            "UPDATE urls SET title = ?2 WHERE url = ?1",
            params![url, title],
        )?;
    }
    Ok(())
}

/// Recompute `visit_count` / `last_visit` from the visits, and drop URLs without visits.
pub fn recompute_stats(conn: &Connection) -> DataResult<()> {
    conn.execute_batch(
        "UPDATE urls SET
             visit_count = (SELECT COUNT(*) FROM visits v WHERE v.url_id = urls.id),
             last_visit = COALESCE((SELECT MAX(visit_time) FROM visits v WHERE v.url_id = urls.id), 0);
         DELETE FROM urls WHERE visit_count = 0;",
    )?;
    Ok(())
}

/// One URL of the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Unix ms of the newest visit.
    pub last_visit: i64,
}

/// URLs whose URL or title contains `q`, visited at or after `since`, newest first.
pub fn search(
    conn: &Connection,
    q: Option<&str>,
    since: Option<i64>,
    limit: usize,
) -> DataResult<Vec<HistoryEntry>> {
    let pattern = like_pattern(q.unwrap_or("").trim());
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count, last_visit FROM urls
         WHERE (url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\') AND last_visit >= ?2
         ORDER BY last_visit DESC, id DESC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![pattern, since.unwrap_or(0), limit as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get(1)?,
                visit_count: r.get(2)?,
                last_visit: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// What a delete removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Deleted {
    pub visits: usize,
    pub urls: usize,
}

/// Forget one URL and all its visits.
pub fn delete_url(conn: &mut Connection, url: &str) -> DataResult<Deleted> {
    let tx = conn.transaction()?;
    let visits: i64 = tx.query_row(
        "SELECT COUNT(*) FROM visits v JOIN urls u ON u.id = v.url_id WHERE u.url = ?1",
        [url],
        |r| r.get(0),
    )?;
    // Visits go with it (`ON DELETE CASCADE`).
    let urls = tx.execute("DELETE FROM urls WHERE url = ?1", [url])?;
    tx.commit()?;
    Ok(Deleted {
        visits: visits as usize,
        urls,
    })
}

/// Forget every visit before `before` (Unix ms) and the URLs left without visits.
pub fn delete_before(conn: &mut Connection, before: i64) -> DataResult<Deleted> {
    let tx = conn.transaction()?;
    let visits = tx.execute("DELETE FROM visits WHERE visit_time < ?1", [before])?;
    let urls_before: i64 = tx.query_row("SELECT COUNT(*) FROM urls", [], |r| r.get(0))?;
    recompute_stats(&tx)?;
    let urls_after: i64 = tx.query_row("SELECT COUNT(*) FROM urls", [], |r| r.get(0))?;
    tx.commit()?;
    Ok(Deleted {
        visits,
        urls: (urls_before - urls_after) as usize,
    })
}

/// One entry of the new-tab page's "Top sites".
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TopSite {
    /// Page title, or the host name when the page had none.
    pub title: String,
    pub url: String,
    pub visit_count: i64,
    pub last_visit: i64,
    /// Frecency score (higher = shown first).
    pub score: i64,
}

/// The most "frecent" URLs at time `now`.
///
/// Frecency (like Firefox, simplified): every visit adds points by its age — 100 within 4 days,
/// 70 within 2 weeks, 50 within a month, 30 within 3 months, 10 if older — doubled for visits
/// the user asked for explicitly (`typed`, `bookmark`). Ties go to the most recent.
pub fn top_sites(conn: &Connection, now: i64, limit: usize) -> DataResult<Vec<TopSite>> {
    let mut stmt = conn.prepare(
        "SELECT u.url, u.title, u.visit_count, u.last_visit,
                SUM(
                    CASE
                        WHEN v.visit_time >= ?1 - 4 * ?2 THEN 100
                        WHEN v.visit_time >= ?1 - 14 * ?2 THEN 70
                        WHEN v.visit_time >= ?1 - 31 * ?2 THEN 50
                        WHEN v.visit_time >= ?1 - 90 * ?2 THEN 30
                        ELSE 10
                    END
                    * CASE WHEN v.transition IN ('typed', 'bookmark') THEN 2 ELSE 1 END
                ) AS score
         FROM urls u JOIN visits v ON v.url_id = u.id
         GROUP BY u.id
         ORDER BY score DESC, u.last_visit DESC, u.id DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![now, DAY_MS, limit as i64], |r| {
            let url: String = r.get(0)?;
            let title: String = r.get(1)?;
            Ok(TopSite {
                title: if title.is_empty() {
                    host_of(&url)
                } else {
                    title
                },
                url,
                visit_count: r.get(2)?,
                last_visit: r.get(3)?,
                score: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `"https://www.example.org/a"` → `"www.example.org"` (the URL itself if it has no host).
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// `history.search`.
pub struct Search;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Text to find in URLs and titles (default: everything).
    pub q: Option<String>,
    /// Only URLs visited at or after this time (Unix ms).
    pub since: Option<i64>,
    /// Max results (default 100, max 5000).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Matching URLs, most recently visited first.
    pub entries: Vec<HistoryEntry>,
}

impl Command for Search {
    const NAME: &'static str = "history.search";
    type Args = SearchArgs;
    type Output = SearchOutput;

    fn meta() -> Meta {
        Meta::new(
            "Search the browsing history by text in URL/title and/or visit time. Returns \
             {entries:[{url,title,visit_count,last_visit}]}, most recent first (times are \
             Unix ms).",
            Toolset::Data,
        )
        .palette("Search history…")
    }

    async fn run(ctx: Ctx, args: SearchArgs) -> Result<SearchOutput, CmdError> {
        let limit = args.limit.unwrap_or(100).clamp(1, 5000);
        let entries = DataStore::from_ctx(&ctx)?
            .run(move |c| search(c, args.q.as_deref(), args.since, limit))
            .await?;
        Ok(SearchOutput { entries })
    }
}

/// `history.delete`.
pub struct Delete;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// Forget this exact URL (all its visits).
    pub url: Option<String>,
    /// Forget every visit before this time (Unix ms). Use the current time to clear everything.
    pub before: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteOutput {
    pub deleted_visits: usize,
    pub deleted_urls: usize,
}

impl Command for Delete {
    const NAME: &'static str = "history.delete";
    type Args = DeleteArgs;
    type Output = DeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete history: one URL (`url`) or every visit before a time (`before`, Unix ms). \
             Give exactly one. Returns how many visits and URLs were removed.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: DeleteArgs) -> Result<DeleteOutput, CmdError> {
        let store = DataStore::from_ctx(&ctx)?;
        let deleted = match (args.url, args.before) {
            (Some(url), None) => store.run(move |c| delete_url(c, &url)).await?,
            (None, Some(before)) => store.run(move |c| delete_before(c, before)).await?,
            _ => {
                return Err(
                    DataError::Invalid("give exactly one of `url` or `before`".into()).into(),
                );
            }
        };
        Ok(DeleteOutput {
            deleted_visits: deleted.visits,
            deleted_urls: deleted.urls,
        })
    }
}

/// `data.top_sites`.
pub struct TopSites;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopSitesArgs {
    /// How many (default 8, max 100).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TopSitesOutput {
    /// Most frecent first; `{title, url}` is what the new-tab page renders.
    pub sites: Vec<TopSite>,
}

impl Command for TopSites {
    const NAME: &'static str = "data.top_sites";
    type Args = TopSitesArgs;
    type Output = TopSitesOutput;

    fn meta() -> Meta {
        Meta::new(
            "The most frequently and recently visited pages (frecency), as shown on the new-tab \
             page. Returns {sites:[{title,url,visit_count,last_visit,score}]}.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: TopSitesArgs) -> Result<TopSitesOutput, CmdError> {
        let limit = args.limit.unwrap_or(8).clamp(1, 100);
        let sites = DataStore::from_ctx(&ctx)?
            .run(move |c| top_sites(c, now_ms(), limit))
            .await?;
        Ok(TopSitesOutput { sites })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tests::{call, call_err, setup};
    use bmcp_core::ErrorCode;

    fn store() -> DataStore {
        DataStore::open_in_memory().unwrap()
    }

    fn visit(store: &DataStore, url: &str, title: &str, transition: &str, time: i64) -> bool {
        store
            .with(|c| record_visit(c, url, title, transition, time))
            .unwrap()
    }

    #[test]
    fn dedupes_within_30_seconds_and_skips_internal_urls() {
        let store = store();
        assert!(visit(&store, "https://a.test/", "", "link", 100_000));
        // 10 s later: same visit, but the late title is kept.
        assert!(!visit(&store, "https://a.test/", "A page", "link", 110_000));
        // 31 s after the first: a new visit.
        assert!(visit(&store, "https://a.test/", "", "link", 131_000));
        assert!(!visit(&store, "about:blank", "", "link", 200_000));
        assert!(!visit(&store, "bmcp://ui/newtab", "", "link", 200_000));

        let all = store.with(|c| search(c, None, None, 10)).unwrap();
        assert_eq!(
            all,
            [HistoryEntry {
                url: "https://a.test/".into(),
                title: "A page".into(),
                visit_count: 2,
                last_visit: 131_000,
            }]
        );
    }

    #[test]
    fn frecency_prefers_recent_and_typed_visits() {
        let store = store();
        let now = 1_000 * DAY_MS;
        // old.test: 5 visits 100+ days ago → 5 × 10 = 50.
        for i in 0..5 {
            visit(
                &store,
                "https://old.test/",
                "Old",
                "link",
                now - (100 + i) * DAY_MS,
            );
        }
        // recent.test: 1 visit yesterday → 100.
        visit(
            &store,
            "https://recent.test/",
            "Recent",
            "link",
            now - DAY_MS,
        );
        // typed.test: 1 typed visit 10 days ago → 70 × 2 = 140.
        visit(
            &store,
            "https://typed.test/",
            "",
            "typed",
            now - 10 * DAY_MS,
        );
        // month.test: 2 visits ~20 days ago → 2 × 50 = 100, but older than recent.test.
        visit(
            &store,
            "https://month.test/",
            "Month",
            "link",
            now - 20 * DAY_MS,
        );
        visit(
            &store,
            "https://month.test/",
            "Month",
            "link",
            now - 21 * DAY_MS,
        );

        let top = store.with(|c| top_sites(c, now, 10)).unwrap();
        let order: Vec<(&str, i64)> = top.iter().map(|s| (s.url.as_str(), s.score)).collect();
        assert_eq!(
            order,
            [
                ("https://typed.test/", 140),
                ("https://recent.test/", 100),
                ("https://month.test/", 100),
                ("https://old.test/", 50),
            ]
        );
        // No title → host name.
        assert_eq!(top[0].title, "typed.test");
        assert_eq!(store.with(|c| top_sites(c, now, 2)).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn search_and_delete_commands() {
        let (registry, ctx) = setup();
        let store = DataStore::from_ctx(&ctx).unwrap();
        visit(&store, "https://rust-lang.org/", "Rust", "link", 1_000_000);
        visit(&store, "https://docs.rs/", "Docs.rs", "link", 2_000_000);
        visit(&store, "https://docs.rs/", "Docs.rs", "link", 3_000_000);
        visit(
            &store,
            "https://crates.io/",
            "crates.io: Rust Package Registry",
            "link",
            4_000_000,
        );

        let out = call(&registry, &ctx, "history.search", json!({"q": "rust"})).await;
        let urls: Vec<_> = out["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["url"].clone())
            .collect();
        assert_eq!(
            urls,
            [json!("https://crates.io/"), json!("https://rust-lang.org/")]
        );

        let out = call(
            &registry,
            &ctx,
            "history.search",
            json!({"since": 2_500_000, "limit": 1}),
        )
        .await;
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["entries"][0]["url"], "https://crates.io/");

        // Delete one URL.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"url": "https://crates.io/"}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 1, "deleted_urls": 1}));

        // Delete before a time: the first docs.rs visit and rust-lang.org go.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"before": 2_500_000}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 2, "deleted_urls": 1}));
        let out = call(&registry, &ctx, "history.search", json!({})).await;
        assert_eq!(
            out["entries"],
            json!([{"url": "https://docs.rs/", "title": "Docs.rs", "visit_count": 1, "last_visit": 3_000_000}])
        );

        let err = call_err(&registry, &ctx, "history.delete", json!({})).await;
        assert_eq!(err.code, ErrorCode::InvalidParams);
        let err = call_err(
            &registry,
            &ctx,
            "history.delete",
            json!({"url": "x", "before": 1}),
        )
        .await;
        assert_eq!(err.code, ErrorCode::InvalidParams);

        // Top sites through the command (real clock: all visits are "old" → 10 points each).
        let out = call(&registry, &ctx, "data.top_sites", json!({})).await;
        assert_eq!(out["sites"][0]["url"], "https://docs.rs/");
        assert_eq!(out["sites"][0]["title"], "Docs.rs");
    }
}
