//! Import from Google Chrome (docs/plan.md, "Імпорт з Chrome": every part is optional).
//!
//! | Part | Source | Goes to |
//! |---|---|---|
//! | `bookmarks` | `<profile>/Bookmarks` (JSON) | bookmark bar → folder 1, "Other" → folder 2, "Mobile" → a sub-folder of 2 |
//! | `history` | `<profile>/History` (SQLite, **copied** to a temp folder first: Chrome keeps it locked) | `urls` + `visits` |
//! | `passwords_csv` | the CSV from chrome://password-manager → Settings → Export (`name,url,username,password,note`) | `passwords` (plain text, see [`crate::passwords`]) |
//! | `cookies_json` | a JSON array as written by cookie-export extensions (`{domain,name,value,path,expirationDate,secure,httpOnly,sameSite}`) | `pending_cookies` |
//!
//! Imported cookies are **not live yet**: they wait in `pending_cookies` until a later CEF step
//! pushes them into the profile's cookie manager (`CefCookieManager::SetCookie`) using
//! [`list_pending_cookies`] and then removes them with [`delete_pending_cookies`].
//!
//! Every part is idempotent: importing the same files twice adds nothing the second time.
//! Chrome stores times as microseconds since 1601-01-01 ([`webkit_to_unix_ms`]).

use std::io::Read;
use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bookmarks::{add_bookmark, find_or_create_folder, folder_has_url};
use crate::history::{self, is_recordable};
use crate::passwords;
use crate::store::{BOOKMARKS_BAR, DataError, DataResult, DataStore, OTHER_BOOKMARKS, now_ms};

/// Milliseconds between 1601-01-01 (WebKit/Windows epoch) and 1970-01-01 (Unix epoch).
const WEBKIT_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;

/// Chrome time (µs since 1601-01-01 UTC) → Unix ms. `0` (Chrome's "unknown") stays `0`.
pub fn webkit_to_unix_ms(webkit_us: i64) -> i64 {
    if webkit_us <= 0 {
        0
    } else {
        webkit_us / 1000 - WEBKIT_EPOCH_OFFSET_MS
    }
}

/// What to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Bookmarks,
    History,
    PasswordsCsv,
    CookiesJson,
}

/// Counts for one part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub imported: usize,
    /// Already present, or not importable (bad URL, subframe visit, empty password, …).
    pub skipped: usize,
}

// ---------------------------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------------------------

/// Import Chrome's `Bookmarks` JSON text.
pub fn import_bookmarks_json(conn: &mut Connection, text: &str) -> DataResult<Counts> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| DataError::Invalid(format!("Bookmarks is not valid JSON: {e}")))?;
    let roots = root
        .get("roots")
        .ok_or_else(|| DataError::Invalid("Bookmarks has no \"roots\"".into()))?;
    let tx = conn.transaction()?;
    let mut counts = Counts::default();
    for (key, target) in [
        ("bookmark_bar", Some(BOOKMARKS_BAR)),
        ("other", Some(OTHER_BOOKMARKS)),
        ("synced", None),
    ] {
        let Some(children) = roots[key]["children"].as_array().filter(|c| !c.is_empty()) else {
            continue;
        };
        // "Mobile bookmarks" only gets a folder when it has something in it.
        let folder = match target {
            Some(folder) => folder,
            None => find_or_create_folder(&tx, OTHER_BOOKMARKS, "Mobile bookmarks")?,
        };
        import_bookmark_nodes(&tx, children, folder, &mut counts)?;
    }
    tx.commit()?;
    Ok(counts)
}

fn import_bookmark_nodes(
    conn: &Connection,
    nodes: &[Value],
    folder: i64,
    counts: &mut Counts,
) -> DataResult<()> {
    for node in nodes {
        let name = node["name"].as_str().unwrap_or_default();
        let created = node["date_added"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(webkit_to_unix_ms)
            .filter(|&ms| ms > 0)
            .unwrap_or_else(now_ms);
        match node["type"].as_str() {
            Some("url") => {
                let url = node["url"].as_str().unwrap_or_default();
                if url.is_empty() || folder_has_url(conn, folder, url)? {
                    counts.skipped += 1;
                    continue;
                }
                match add_bookmark(conn, folder, url, Some(name), created) {
                    Ok(_) => counts.imported += 1,
                    Err(DataError::Invalid(_)) => counts.skipped += 1,
                    Err(other) => return Err(other),
                }
            }
            Some("folder") => {
                let title = if name.trim().is_empty() {
                    "Folder"
                } else {
                    name
                };
                let sub = find_or_create_folder(conn, folder, title)?;
                let children = node["children"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                import_bookmark_nodes(conn, children, sub, counts)?;
            }
            _ => counts.skipped += 1,
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------------------------

/// Chrome's core page transition (`transition & 0xFF`) → our transition name; `None` = skip
/// (iframe loads are not pages the user visited).
fn transition_name(chrome: i64) -> Option<&'static str> {
    Some(match chrome & 0xFF {
        1 => "typed",
        2 => "bookmark",
        3 | 4 => return None, // AUTO_SUBFRAME, MANUAL_SUBFRAME
        5 | 10 => "generated",
        6 => "toplevel",
        7 => "form_submit",
        8 => "reload",
        9 => "keyword",
        _ => "link",
    })
}

/// Import a Chrome `History` file. The file (and its `-wal`/`-journal`) is copied to a temp
/// folder first, so a running Chrome's lock does not matter and the original is never touched.
pub fn import_history_file(conn: &mut Connection, history: &Path) -> DataResult<Counts> {
    if !history.is_file() {
        return Err(DataError::NotFound(format!(
            "{} not found",
            history.display()
        )));
    }
    let tmp = tempfile::tempdir()?;
    let copy = tmp.path().join("History");
    std::fs::copy(history, &copy)?;
    for suffix in ["-wal", "-journal"] {
        let side = PathBuf::from(format!("{}{suffix}", history.display()));
        if side.is_file() {
            std::fs::copy(&side, tmp.path().join(format!("History{suffix}")))?;
        }
    }
    let source = Connection::open(&copy)?;
    let counts = import_history_db(conn, &source)?;
    drop(source);
    Ok(counts)
}

/// Copy the visits of an open Chrome history database into ours.
pub fn import_history_db(conn: &mut Connection, source: &Connection) -> DataResult<Counts> {
    let mut stmt = source.prepare(
        "SELECT u.url, COALESCE(u.title, ''), v.visit_time, v.transition
         FROM visits v JOIN urls u ON u.id = v.url
         ORDER BY v.visit_time",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;

    let tx = conn.transaction()?;
    let mut counts = Counts::default();
    {
        let mut insert_url = tx.prepare(
            "INSERT INTO urls (url, title) VALUES (?1, ?2)
             ON CONFLICT(url) DO UPDATE SET title = CASE WHEN urls.title = '' THEN excluded.title ELSE urls.title END",
        )?;
        let mut url_id = tx.prepare("SELECT id FROM urls WHERE url = ?1")?;
        let mut insert_visit = tx.prepare(
            "INSERT OR IGNORE INTO visits (url_id, visit_time, transition) VALUES (?1, ?2, ?3)",
        )?;
        for row in rows {
            let (url, title, visit_time, transition) = row?;
            let (Some(transition), true) = (transition_name(transition), is_recordable(&url))
            else {
                counts.skipped += 1;
                continue;
            };
            insert_url.execute(params![url, title])?;
            let id: i64 = url_id.query_row([&url], |r| r.get(0))?;
            match insert_visit.execute(params![id, webkit_to_unix_ms(visit_time), transition])? {
                0 => counts.skipped += 1, // already imported
                _ => counts.imported += 1,
            }
        }
    }
    history::recompute_stats(&tx)?;
    tx.commit()?;
    Ok(counts)
}

// ---------------------------------------------------------------------------------------------
// Passwords (CSV)
// ---------------------------------------------------------------------------------------------

/// Import Chrome's password CSV (`name,url,username,password[,note]`; columns found by header).
pub fn import_passwords_csv(conn: &mut Connection, csv_data: impl Read) -> DataResult<Counts> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::Headers)
        .from_reader(csv_data);
    let headers = reader
        .headers()
        .map_err(|e| DataError::Invalid(format!("password CSV: {e}")))?
        .clone();
    let column = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let (Some(url_col), Some(password_col)) = (column("url"), column("password")) else {
        return Err(DataError::Invalid(
            "password CSV needs `url` and `password` columns (Chrome: name,url,username,password,note)"
                .into(),
        ));
    };
    let (user_col, note_col) = (column("username"), column("note"));

    let tx = conn.transaction()?;
    let now = now_ms();
    let mut counts = Counts::default();
    for record in reader.records() {
        let Ok(record) = record else {
            counts.skipped += 1;
            continue;
        };
        let field = |col: Option<usize>| col.and_then(|c| record.get(c)).unwrap_or_default();
        let (url, password) = (field(Some(url_col)), field(Some(password_col)));
        if url.trim().is_empty() || password.is_empty() {
            counts.skipped += 1;
            continue;
        }
        match passwords::save(&tx, url, field(user_col), password, field(note_col), now) {
            Ok(_) => counts.imported += 1,
            Err(DataError::Invalid(_)) => counts.skipped += 1,
            Err(other) => return Err(other),
        }
    }
    tx.commit()?;
    Ok(counts)
}

// ---------------------------------------------------------------------------------------------
// Cookies (JSON) → pending_cookies
// ---------------------------------------------------------------------------------------------

/// One cookie of a JSON export (extra fields like `hostOnly`, `storeId` are ignored).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedCookie {
    domain: String,
    name: String,
    value: String,
    #[serde(default = "root_path")]
    path: String,
    /// Unix seconds (may have a fraction).
    expiration_date: Option<f64>,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    http_only: bool,
    same_site: Option<String>,
    /// True for session cookies (no expiry).
    #[serde(default)]
    session: bool,
}

fn root_path() -> String {
    "/".into()
}

/// `"None"`/`"no_restriction"` → `no_restriction`, `"Lax"` → `lax`, `"Strict"` → `strict`,
/// anything else → `unspecified`.
fn same_site(value: Option<&str>) -> &'static str {
    match value.map(str::to_ascii_lowercase).as_deref() {
        Some("none" | "no_restriction") => "no_restriction",
        Some("lax") => "lax",
        Some("strict") => "strict",
        _ => "unspecified",
    }
}

/// Store the cookies of a JSON export into `pending_cookies` (same domain+name+path replaces).
pub fn import_cookies_json(conn: &mut Connection, text: &str) -> DataResult<Counts> {
    let items: Vec<Value> = serde_json::from_str(text).map_err(|e| {
        DataError::Invalid(format!(
            "cookie export must be a JSON array of cookies: {e}"
        ))
    })?;
    let tx = conn.transaction()?;
    let now = now_ms();
    let mut counts = Counts::default();
    {
        let mut upsert = tx.prepare(
            "INSERT INTO pending_cookies
                 (domain, name, value, path, expires, secure, http_only, same_site, imported)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(domain, name, path) DO UPDATE SET
                 value = excluded.value, expires = excluded.expires, secure = excluded.secure,
                 http_only = excluded.http_only, same_site = excluded.same_site,
                 imported = excluded.imported",
        )?;
        for item in items {
            let Ok(cookie) = serde_json::from_value::<ExportedCookie>(item) else {
                counts.skipped += 1;
                continue;
            };
            if cookie.domain.is_empty() || cookie.name.is_empty() {
                counts.skipped += 1;
                continue;
            }
            let expires = match (cookie.session, cookie.expiration_date) {
                (false, Some(secs)) => Some((secs * 1000.0) as i64),
                _ => None,
            };
            upsert.execute(params![
                cookie.domain,
                cookie.name,
                cookie.value,
                cookie.path,
                expires,
                cookie.secure,
                cookie.http_only,
                same_site(cookie.same_site.as_deref()),
                now,
            ])?;
            counts.imported += 1;
        }
    }
    tx.commit()?;
    Ok(counts)
}

/// A cookie waiting to be pushed into CEF's cookie manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingCookie {
    pub id: i64,
    pub domain: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// Unix ms; `None` = session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    /// `unspecified`, `no_restriction`, `lax` or `strict`.
    pub same_site: String,
}

/// Every pending cookie (for the CEF step that applies them).
pub fn list_pending_cookies(conn: &Connection) -> DataResult<Vec<PendingCookie>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain, name, value, path, expires, secure, http_only, same_site
         FROM pending_cookies ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PendingCookie {
                id: r.get(0)?,
                domain: r.get(1)?,
                name: r.get(2)?,
                value: r.get(3)?,
                path: r.get(4)?,
                expires: r.get(5)?,
                secure: r.get(6)?,
                http_only: r.get(7)?,
                same_site: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Remove pending cookies once CEF has accepted them.
pub fn delete_pending_cookies(conn: &Connection, ids: &[i64]) -> DataResult<usize> {
    let mut stmt = conn.prepare("DELETE FROM pending_cookies WHERE id = ?1")?;
    let mut removed = 0;
    for id in ids {
        removed += stmt.execute([id])?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------------------------
// Profile detection
// ---------------------------------------------------------------------------------------------

/// Where Chrome keeps its "User Data" folder on this OS (may not exist).
pub fn chrome_user_data_dirs() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        dirs.extend(env("LOCALAPPDATA").map(|d| d.join("Google").join("Chrome").join("User Data")));
    } else if cfg!(target_os = "macos") {
        dirs.extend(env("HOME").map(|h| h.join("Library/Application Support/Google/Chrome")));
    } else {
        dirs.extend(env("HOME").map(|h| h.join(".config/google-chrome")));
        dirs.extend(env("HOME").map(|h| h.join(".config/chromium")));
    }
    dirs
}

/// A Chrome profile folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ChromeProfile {
    /// Folder name: `Default`, `Profile 1`, …
    pub name: String,
    /// Full path; pass it as `profile_dir` to import_chrome.
    pub path: String,
    pub has_bookmarks: bool,
    pub has_history: bool,
}

/// The `Default` and `Profile N` folders inside a Chrome "User Data" folder. Only looks at
/// folder and file names; reads no profile data.
pub fn detect_profiles(user_data: &Path) -> Vec<ChromeProfile> {
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return Vec::new();
    };
    let mut profiles: Vec<ChromeProfile> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_profile = name == "Default"
                || name
                    .strip_prefix("Profile ")
                    .is_some_and(|n| n.parse::<u32>().is_ok());
            is_profile.then(|| {
                let path = e.path();
                ChromeProfile {
                    has_bookmarks: path.join("Bookmarks").is_file(),
                    has_history: path.join("History").is_file(),
                    path: path.display().to_string(),
                    name,
                }
            })
        })
        .collect();
    // Default first, then Profile 1, 2, 10 in numeric order.
    profiles.sort_by_key(|p| {
        p.name
            .strip_prefix("Profile ")
            .and_then(|n| n.parse::<u32>().ok())
            .map_or(0, |n| n + 1)
    });
    profiles
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// `import.chrome_detect`.
pub struct Detect;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetectArgs {
    /// Chrome "User Data" folder to scan (default: Chrome's standard location on this OS).
    pub user_data_dir: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DetectOutput {
    /// The folders that were scanned.
    pub user_data_dirs: Vec<String>,
    pub profiles: Vec<ChromeProfile>,
}

impl Command for Detect {
    const NAME: &'static str = "import.chrome_detect";
    type Args = DetectArgs;
    type Output = DetectOutput;

    fn meta() -> Meta {
        Meta::new(
            "Find Chrome profiles that can be imported (Default, Profile 1, …). Lists folders \
             only; reads no data. Returns {user_data_dirs, profiles:[{name,path,has_bookmarks,\
             has_history}]}.",
            Toolset::Data,
        )
    }

    async fn run(_ctx: Ctx, args: DetectArgs) -> Result<DetectOutput, CmdError> {
        let dirs = match args.user_data_dir {
            Some(dir) => vec![PathBuf::from(dir)],
            None => chrome_user_data_dirs(),
        };
        let profiles = dirs.iter().flat_map(|d| detect_profiles(d)).collect();
        Ok(DetectOutput {
            user_data_dirs: dirs.iter().map(|d| d.display().to_string()).collect(),
            profiles,
        })
    }
}

/// `import.chrome`.
pub struct Import;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// What to import; each part is independent.
    pub parts: Vec<Part>,
    /// Chrome profile folder for bookmarks/history (default: `<Chrome User Data>/Default`;
    /// see import_chrome_detect).
    pub profile_dir: Option<String>,
    /// The CSV exported from Chrome's password manager (needed for `passwords_csv`).
    pub passwords_csv_path: Option<String>,
    /// A cookie export JSON file (needed for `cookies_json`).
    pub cookies_json_path: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PartResult {
    pub part: Part,
    pub ok: bool,
    /// File it read.
    pub source: String,
    pub imported: usize,
    pub skipped: usize,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportOutput {
    /// One entry per requested part, in order. A failed part does not stop the others.
    pub results: Vec<PartResult>,
}

impl Command for Import {
    const NAME: &'static str = "import.chrome";
    type Args = ImportArgs;
    type Output = ImportOutput;

    fn meta() -> Meta {
        Meta::new(
            "Import from Chrome: bookmarks and history from a profile folder, passwords from \
             Chrome's exported CSV, cookies from a JSON export (stored until applied to the \
             browser). Re-importing adds no duplicates. Returns per-part counts.",
            Toolset::Data,
        )
        // Imports passwords and cookies.
        .secret()
        .palette("Import from Chrome…")
    }

    async fn run(ctx: Ctx, args: ImportArgs) -> Result<ImportOutput, CmdError> {
        if args.parts.is_empty() {
            return Err(CmdError::invalid_params(
                "`parts` must name at least one part",
            ));
        }
        if args.parts.contains(&Part::PasswordsCsv) && args.passwords_csv_path.is_none() {
            return Err(CmdError::invalid_params(
                "`passwords_csv` needs `passwords_csv_path` (export it in Chrome: Password \
                 Manager → Settings → Export passwords)",
            ));
        }
        if args.parts.contains(&Part::CookiesJson) && args.cookies_json_path.is_none() {
            return Err(CmdError::invalid_params(
                "`cookies_json` needs `cookies_json_path`",
            ));
        }
        let profile = args.profile_dir.map(PathBuf::from).or_else(|| {
            chrome_user_data_dirs()
                .into_iter()
                .map(|d| d.join("Default"))
                .find(|d| d.is_dir())
        });

        let store = DataStore::from_ctx(&ctx)?;
        let results = store
            .run(move |conn| {
                let mut results = Vec::new();
                for part in args.parts {
                    let source = match part {
                        Part::Bookmarks => profile.as_ref().map(|p| p.join("Bookmarks")),
                        Part::History => profile.as_ref().map(|p| p.join("History")),
                        Part::PasswordsCsv => args.passwords_csv_path.as_ref().map(PathBuf::from),
                        Part::CookiesJson => args.cookies_json_path.as_ref().map(PathBuf::from),
                    };
                    let outcome = match &source {
                        None => Err(DataError::NotFound(
                            "no Chrome profile found; pass `profile_dir`".into(),
                        )),
                        Some(path) => import_part(conn, part, path),
                    };
                    let (counts, error) = match outcome {
                        Ok(counts) => (counts, None),
                        Err(err) => (Counts::default(), Some(err.to_string())),
                    };
                    results.push(PartResult {
                        part,
                        ok: error.is_none(),
                        source: source.map(|p| p.display().to_string()).unwrap_or_default(),
                        imported: counts.imported,
                        skipped: counts.skipped,
                        error,
                    });
                }
                Ok(results)
            })
            .await?;
        Ok(ImportOutput { results })
    }
}

fn import_part(conn: &mut Connection, part: Part, path: &Path) -> DataResult<Counts> {
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map_err(|e| DataError::NotFound(format!("cannot read {}: {e}", path.display())))
    };
    match part {
        Part::Bookmarks => import_bookmarks_json(conn, &read(path)?),
        Part::History => import_history_file(conn, path),
        Part::PasswordsCsv => import_passwords_csv(conn, read(path)?.as_bytes()),
        Part::CookiesJson => import_cookies_json(conn, &read(path)?),
    }
}
