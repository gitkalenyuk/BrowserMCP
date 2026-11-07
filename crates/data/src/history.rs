//! Browsing history: `urls` (one row per URL, with `visit_count` / `last_visit`) and `visits`
//! (one row per page load).
//!
//! Visits are recorded automatically by the [`crate::recorder`]; only `http`, `https` and `file`
//! URLs are kept (no `about:blank`, `bmcp://ui`, `devtools://`, `data:` …). A second load of the
//! same URL within [`DEDUPE_MS`] is not a new visit (it only refreshes the title).
//!
//! Deleting works on *visits*: `history.delete` removes the visits in a time window (and/or of
//! one URL) and then drops the URLs that have no visit left, so clearing "the last hour" keeps
//! the older visits of the same pages. It emits `history.changed {op: "delete"}`.
//!
//! Commands: `history.search`, `history.delete`, `data.top_sites` (frecency).

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::history_changed;
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

/// Which visits [`delete_visits`] removes: those of `url` (default: every URL) with
/// `since <= visit_time < before` (default: no bound).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisitFilter {
    pub url: Option<String>,
    pub since: Option<i64>,
    pub before: Option<i64>,
}

/// Delete the visits matching `filter`, update the stats of the URLs they belonged to, and drop
/// those URLs when no visit is left. A URL given without a time window goes away completely.
pub fn delete_visits(conn: &mut Connection, filter: &VisitFilter) -> DataResult<Deleted> {
    let since = filter.since.unwrap_or(i64::MIN);
    let before = filter.before.unwrap_or(i64::MAX);
    let url = filter.url.as_deref();
    let tx = conn.transaction()?;

    // The URLs whose visits we touch (`?3 IS NULL` = any URL).
    let mut affected: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT v.url_id FROM visits v JOIN urls u ON u.id = v.url_id
             WHERE v.visit_time >= ?1 AND v.visit_time < ?2 AND (?3 IS NULL OR u.url = ?3)",
        )?;
        stmt.query_map(params![since, before, url], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    // A URL row without visits (possible after an interrupted import) still goes when the
    // caller names it without a window.
    if let (Some(url), None, None) = (url, filter.since, filter.before) {
        let id: Option<i64> = tx
            .query_row("SELECT id FROM urls WHERE url = ?1", [url], |r| r.get(0))
            .optional()?;
        affected.extend(id.filter(|id| !affected.contains(id)));
    }

    let visits = tx.execute(
        "DELETE FROM visits WHERE visit_time >= ?1 AND visit_time < ?2
             AND (?3 IS NULL OR url_id = (SELECT id FROM urls WHERE url = ?3))",
        params![since, before, url],
    )?;
    let mut urls = 0;
    for id in affected {
        recompute_url(&tx, id)?;
        urls += tx.execute("DELETE FROM urls WHERE id = ?1 AND visit_count = 0", [id])?;
    }
    tx.commit()?;
    Ok(Deleted { visits, urls })
}

/// The web origins (`https://host[:port]`) with a visit at or after `since` (Unix ms; `None`
/// = any time), sorted. For clearing site data of the sites visited in a time range.
pub fn visited_origins(conn: &Connection, since: Option<i64>) -> DataResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT u.url FROM urls u JOIN visits v ON v.url_id = u.id
         WHERE v.visit_time >= ?1",
    )?;
    let urls: Vec<String> = stmt
        .query_map([since.unwrap_or(i64::MIN)], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut origins: Vec<String> = urls.iter().filter_map(|u| web_origin(u)).collect();
    origins.sort();
    origins.dedup();
    Ok(origins)
}

/// `https://a.test:8080/x?y` → `https://a.test:8080`; non-web URLs → `None`.
pub fn web_origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// Recompute `visit_count` / `last_visit` of one URL from its visits.
fn recompute_url(conn: &Connection, id: i64) -> DataResult<()> {
    conn.execute(
        "UPDATE urls SET
             visit_count = (SELECT COUNT(*) FROM visits v WHERE v.url_id = urls.id),
             last_visit = COALESCE((SELECT MAX(visit_time) FROM visits v WHERE v.url_id = urls.id), 0)
         WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// `[start, end)` in Unix ms of the local calendar day `day` (`YYYY-MM-DD`, in the time zone
/// of the machine BrowserMCP runs on, which is also the UI's). DST days are 23 or 25 hours.
pub fn local_day_range(day: &str) -> DataResult<(i64, i64)> {
    use chrono::{Days, Local, NaiveDate, TimeDelta, TimeZone};

    let date = NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")
        .map_err(|_| DataError::Invalid(format!("`day` must be YYYY-MM-DD, got {day:?}")))?;
    let next = date
        .checked_add_days(Days::new(1))
        .ok_or_else(|| DataError::Invalid(format!("`day` {day:?} is out of range")))?;
    // Local midnight. Where a DST change skips midnight, the day starts at the first valid
    // local time (one hour later).
    let midnight = |d: NaiveDate| -> DataResult<i64> {
        let naive = d.and_time(chrono::NaiveTime::MIN);
        Local
            .from_local_datetime(&naive)
            .earliest()
            .or_else(|| {
                Local
                    .from_local_datetime(&(naive + TimeDelta::hours(1)))
                    .earliest()
            })
            .map(|t| t.timestamp_millis())
            .ok_or_else(|| DataError::Invalid(format!("no local midnight on {d}")))
    };
    Ok((midnight(date)?, midnight(next)?))
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

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// Only the visits of this exact URL (alone: the URL with all its visits).
    pub url: Option<String>,
    /// Only visits at or after this time (Unix ms).
    pub since: Option<i64>,
    /// Only visits before this time (Unix ms). Use the current time to clear everything.
    pub before: Option<i64>,
    /// Only visits on this local calendar day, `YYYY-MM-DD` (not together with
    /// `since`/`before`).
    pub day: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteOutput {
    pub deleted_visits: usize,
    /// URLs that had no visit left and were dropped.
    pub deleted_urls: usize,
}

impl DeleteArgs {
    /// Turn the arguments into a [`VisitFilter`], refusing an empty or contradictory one.
    fn filter(self) -> DataResult<VisitFilter> {
        if self.url.is_none() && self.since.is_none() && self.before.is_none() && self.day.is_none()
        {
            return Err(DataError::Invalid(
                "give `url`, `since`, `before` and/or `day` (nothing is deleted by default)".into(),
            ));
        }
        let (since, before) = match self.day {
            Some(_) if self.since.is_some() || self.before.is_some() => {
                return Err(DataError::Invalid(
                    "`day` cannot be combined with `since`/`before`".into(),
                ));
            }
            Some(day) => {
                let (start, end) = local_day_range(&day)?;
                (Some(start), Some(end))
            }
            None => (self.since, self.before),
        };
        if let (Some(since), Some(before)) = (since, before)
            && since >= before
        {
            return Err(DataError::Invalid("`since` must be before `before`".into()));
        }
        Ok(VisitFilter {
            url: self.url,
            since,
            before,
        })
    }
}

impl Command for Delete {
    const NAME: &'static str = "history.delete";
    type Args = DeleteArgs;
    type Output = DeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete history visits: in a time window (`since` and/or `before`, Unix ms), on a \
             local day (`day`, YYYY-MM-DD) and/or of one URL (`url`; alone it forgets the URL \
             completely). URLs left without visits are dropped. Returns how many visits and \
             URLs were removed.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: DeleteArgs) -> Result<DeleteOutput, CmdError> {
        let filter = args.filter()?;
        let deleted = DataStore::from_ctx(&ctx)?
            .run(move |c| delete_visits(c, &filter))
            .await?;
        if deleted.visits + deleted.urls > 0 {
            history_changed(&ctx, "delete");
        }
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
    fn visited_origins_since_a_time() {
        let store = store();
        visit(&store, "https://a.test/x", "", "link", 1_000);
        visit(&store, "https://a.test/y?q=1", "", "link", 5_000);
        visit(&store, "http://b.test:8080/", "", "link", 6_000);
        visit(&store, "file:///C:/x.html", "", "link", 7_000);
        let all = store.with(|c| visited_origins(c, None)).unwrap();
        assert_eq!(all, ["http://b.test:8080", "https://a.test"]);
        let recent = store.with(|c| visited_origins(c, Some(5_500))).unwrap();
        assert_eq!(recent, ["http://b.test:8080"]);
        assert_eq!(
            web_origin("https://A.test:443/p"),
            Some("https://a.test".into())
        );
        assert_eq!(web_origin("about:blank"), None);
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

        for args in [
            json!({}),
            json!({"since": 5, "before": 5}),
            json!({"day": "2026-11-06", "since": 1}),
            json!({"day": "06.11.2026"}),
            json!({"day": "2026-02-30"}),
        ] {
            let err = call_err(&registry, &ctx, "history.delete", args.clone()).await;
            assert_eq!(err.code, ErrorCode::InvalidParams, "{args}");
        }

        // Top sites through the command (real clock: all visits are "old" → 10 points each).
        let out = call(&registry, &ctx, "data.top_sites", json!({})).await;
        assert_eq!(out["sites"][0]["url"], "https://docs.rs/");
        assert_eq!(out["sites"][0]["title"], "Docs.rs");
    }

    /// `(url, visit_count, last_visit)` of every page, most recent first.
    fn visits_by_url(store: &DataStore) -> Vec<(String, i64, i64)> {
        store
            .with(|c| search(c, None, None, 100))
            .unwrap()
            .into_iter()
            .map(|e| (e.url, e.visit_count, e.last_visit))
            .collect()
    }

    #[tokio::test]
    async fn delete_removes_visits_in_a_window_not_whole_pages() {
        let (registry, ctx) = setup();
        let store = DataStore::from_ctx(&ctx).unwrap();
        // a.test: visits at 1 000 s, 2 000 s, 3 000 s; b.test: only at 2 500 s; c.test: 500 s.
        for t in [1_000_000, 2_000_000, 3_000_000] {
            visit(&store, "https://a.test/", "A", "link", t);
        }
        visit(&store, "https://b.test/", "B", "link", 2_500_000);
        visit(&store, "https://c.test/", "C", "link", 500_000);

        let mut rx = ctx.events.subscribe();
        // "The last ..." = since: the newest a.test visit and b.test go; older a.test stays.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"since": 2_400_000}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 2, "deleted_urls": 1}));
        assert_eq!(
            visits_by_url(&store),
            [
                ("https://a.test/".into(), 2, 2_000_000),
                ("https://c.test/".into(), 1, 500_000),
            ]
        );

        // A closed window [900 s, 1 500 s) takes the oldest a.test visit only.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"since": 900_000, "before": 1_500_000}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 1, "deleted_urls": 0}));

        // URL + window: only that URL's visits in the window.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"url": "https://c.test/", "before": 1_500_000}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 1, "deleted_urls": 1}));
        assert_eq!(
            visits_by_url(&store),
            [("https://a.test/".into(), 1, 2_000_000)]
        );

        // Nothing matched: no event.
        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"url": "https://nothing.test/"}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 0, "deleted_urls": 0}));

        let events = crate::tests::drain_events(&mut rx, "history.");
        let delete = ("history.changed".to_string(), json!({"op": "delete"}));
        assert_eq!(events, [delete.clone(), delete.clone(), delete]);
    }

    #[tokio::test]
    async fn delete_a_local_day() {
        use chrono::{Local, NaiveDate, TimeZone};
        let at = |d: &str, h: u32| {
            let naive = NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap();
            Local
                .from_local_datetime(&naive)
                .earliest()
                .unwrap()
                .timestamp_millis()
        };
        let (registry, ctx) = setup();
        let store = DataStore::from_ctx(&ctx).unwrap();
        // The page visited on 4, 5 and 6 November; another one only on the 5th.
        for d in ["2026-11-04", "2026-11-05", "2026-11-06"] {
            visit(&store, "https://a.test/", "A", "link", at(d, 12));
        }
        visit(&store, "https://b.test/", "B", "link", at("2026-11-05", 0));
        visit(&store, "https://b.test/", "B", "link", at("2026-11-05", 23));

        let out = call(
            &registry,
            &ctx,
            "history.delete",
            json!({"day": "2026-11-05"}),
        )
        .await;
        assert_eq!(out, json!({"deleted_visits": 3, "deleted_urls": 1}));
        assert_eq!(
            visits_by_url(&store),
            [("https://a.test/".into(), 2, at("2026-11-06", 12))]
        );

        // The day's bounds are local midnights, end exclusive.
        let (start, end) = local_day_range("2026-11-05").unwrap();
        assert_eq!(start, at("2026-11-05", 0));
        assert_eq!(end, at("2026-11-06", 0));
        assert!(local_day_range("2026-13-01").is_err());
    }
}
