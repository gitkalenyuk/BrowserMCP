//! [`DataStore`]: the one SQLite database (`data/db/browsermcp.sqlite`) and its schema.
//!
//! * One connection behind a mutex. Every command runs its SQL through [`DataStore::run`],
//!   which moves the work to tokio's blocking thread pool, so a slow query (an import) never
//!   stalls the async worker threads.
//! * WAL journal: readers do not block the writer, and a crash cannot corrupt the file.
//! * Migrations: SQLite's `PRAGMA user_version` holds the number of applied entries of
//!   [`MIGRATIONS`]. On open every newer entry runs in its own transaction. To change the schema,
//!   **append** a new entry; never edit an applied one.
//!
//! Times are stored as integers: milliseconds since the Unix epoch (UTC).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bmcp_core::{CmdError, Ctx, DataDir, ErrorCode, Retry};
use rusqlite::Connection;

/// Fixed id of the "Bookmarks bar" root folder (shown on the new-tab page).
pub const BOOKMARKS_BAR: i64 = 1;
/// Fixed id of the "Other bookmarks" root folder.
pub const OTHER_BOOKMARKS: i64 = 2;

/// Schema changes, oldest first. Entry `i` brings the database to `user_version = i + 1`.
pub const MIGRATIONS: &[&str] = &[
    // 1: bookmarks, history, downloads, passwords, pending cookies.
    r#"
    -- Folders and bookmarks share one tree. Folders have url = NULL. The two root folders
    -- (ids 1 and 2) always exist and cannot be removed or moved.
    CREATE TABLE bookmarks (
        id       INTEGER PRIMARY KEY,
        parent   INTEGER REFERENCES bookmarks(id) ON DELETE CASCADE,
        kind     TEXT    NOT NULL CHECK (kind IN ('folder', 'url')),
        title    TEXT    NOT NULL DEFAULT '',
        url      TEXT,
        position INTEGER NOT NULL DEFAULT 0,
        created  INTEGER NOT NULL
    );
    CREATE INDEX bookmarks_by_parent ON bookmarks(parent, position);
    CREATE INDEX bookmarks_by_url ON bookmarks(url);
    INSERT INTO bookmarks (id, parent, kind, title, position, created) VALUES
        (1, NULL, 'folder', 'Bookmarks bar', 0, 0),
        (2, NULL, 'folder', 'Other bookmarks', 1, 0);

    -- History: one row per URL (with derived stats) + one row per visit.
    CREATE TABLE urls (
        id          INTEGER PRIMARY KEY,
        url         TEXT    NOT NULL UNIQUE,
        title       TEXT    NOT NULL DEFAULT '',
        visit_count INTEGER NOT NULL DEFAULT 0,
        last_visit  INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX urls_by_last_visit ON urls(last_visit);
    CREATE TABLE visits (
        id         INTEGER PRIMARY KEY,
        url_id     INTEGER NOT NULL REFERENCES urls(id) ON DELETE CASCADE,
        visit_time INTEGER NOT NULL,
        -- link | typed | bookmark | reload | form_submit | keyword | generated | toplevel
        transition TEXT    NOT NULL DEFAULT 'link',
        -- Makes re-importing the same Chrome history a no-op.
        UNIQUE (url_id, visit_time)
    );
    CREATE INDEX visits_by_time ON visits(visit_time);

    -- Filled by a later CEF download hook; only listed for now.
    CREATE TABLE downloads (
        id             INTEGER PRIMARY KEY,
        url            TEXT    NOT NULL,
        path           TEXT    NOT NULL DEFAULT '',
        mime           TEXT    NOT NULL DEFAULT '',
        total_bytes    INTEGER,
        received_bytes INTEGER NOT NULL DEFAULT 0,
        -- in_progress | complete | cancelled | interrupted
        state          TEXT    NOT NULL DEFAULT 'in_progress',
        started        INTEGER NOT NULL,
        ended          INTEGER
    );

    -- PLAIN TEXT passwords: the user chose no encryption (docs/plan.md, "Секрети").
    CREATE TABLE passwords (
        id       INTEGER PRIMARY KEY,
        origin   TEXT    NOT NULL,
        username TEXT    NOT NULL DEFAULT '',
        password TEXT    NOT NULL,
        note     TEXT    NOT NULL DEFAULT '',
        created  INTEGER NOT NULL,
        updated  INTEGER NOT NULL,
        UNIQUE (origin, username)
    );

    -- Cookies imported from a JSON export, waiting for the CEF layer to push them into the
    -- profile's cookie manager (and then delete the rows).
    CREATE TABLE pending_cookies (
        id        INTEGER PRIMARY KEY,
        domain    TEXT    NOT NULL,
        name      TEXT    NOT NULL,
        value     TEXT    NOT NULL,
        path      TEXT    NOT NULL DEFAULT '/',
        expires   INTEGER,           -- Unix ms; NULL = session cookie
        secure    INTEGER NOT NULL DEFAULT 0,
        http_only INTEGER NOT NULL DEFAULT 0,
        same_site TEXT    NOT NULL DEFAULT 'unspecified', -- unspecified | no_restriction | lax | strict
        imported  INTEGER NOT NULL,
        UNIQUE (domain, name, path)
    );
    "#,
];

/// Errors of the data layer. Converted to [`CmdError`] at the command boundary.
#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Bad input from the caller (→ `INVALID_PARAMS`).
    #[error("{0}")]
    Invalid(String),
    /// The id/file/profile does not exist (→ `NOT_FOUND`).
    #[error("{0}")]
    NotFound(String),
}

pub type DataResult<T> = Result<T, DataError>;

impl From<DataError> for CmdError {
    fn from(err: DataError) -> Self {
        let message = err.to_string();
        match err {
            DataError::Invalid(_) => CmdError::invalid_params(message),
            DataError::NotFound(_) => {
                CmdError::new(ErrorCode::NotFound, message).with_retry(Retry::No)
            }
            DataError::Sqlite(_) | DataError::Io(_) => CmdError::internal(message),
        }
    }
}

/// Handle to the database. Cheap to clone; every clone shares the same connection.
#[derive(Clone)]
pub struct DataStore {
    conn: Arc<Mutex<Connection>>,
}

impl DataStore {
    /// File name inside `data/db/`.
    pub const FILE_NAME: &'static str = "browsermcp.sqlite";

    /// Open (or create) `<data>/db/browsermcp.sqlite`.
    pub fn open_in(data: &DataDir) -> DataResult<Self> {
        std::fs::create_dir_all(data.db())?;
        Self::open(&data.db().join(Self::FILE_NAME))
    }

    /// Open (or create) the database file at `path` and bring its schema up to date.
    pub fn open(path: &Path) -> DataResult<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory() -> DataResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> DataResult<Self> {
        // WAL for crash safety and concurrent readers; in-memory databases ignore it.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(DataStore {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Current `user_version` (= number of applied migrations).
    pub fn schema_version(&self) -> DataResult<u32> {
        self.with(|conn| Ok(user_version(conn)?))
    }

    /// Run `f` on the connection right here (blocking). Prefer [`DataStore::run`] in async code.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> DataResult<T>) -> DataResult<T> {
        // A panic inside `f` rolls back any open transaction (it is dropped), so a poisoned
        // lock still guards a consistent connection.
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut conn)
    }

    /// Run `f` on the connection on tokio's blocking pool and wait for it.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> DataResult<T> + Send + 'static,
    ) -> DataResult<T> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.with(f))
            .await
            .unwrap_or_else(|join| {
                Err(DataError::Io(std::io::Error::other(format!(
                    "database task failed: {join}"
                ))))
            })
    }

    /// The store the app put into the registry's extensions.
    pub fn from_ctx(ctx: &Ctx) -> Result<DataStore, CmdError> {
        ctx.extensions
            .get::<DataStore>()
            .map(|store| (*store).clone())
            .ok_or_else(|| CmdError::internal("the data store (browsermcp.sqlite) is not open"))
    }
}

impl std::fmt::Debug for DataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataStore").finish_non_exhaustive()
    }
}

fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Apply every migration newer than the database's `user_version`, each in a transaction.
fn migrate(conn: &mut Connection) -> DataResult<()> {
    let current = user_version(conn)? as usize;
    if current > MIGRATIONS.len() {
        return Err(DataError::Invalid(format!(
            "browsermcp.sqlite has schema version {current}, but this build only knows {}; \
             use a newer BrowserMCP",
            MIGRATIONS.len()
        )));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as u32)?;
        tx.commit()?;
    }
    Ok(())
}

/// Wall-clock time in Unix milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `LIKE` pattern matching `q` anywhere, with `%`, `_` and `\` in `q` taken literally
/// (use with `ESCAPE '\'`).
pub fn like_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent_and_keep_data() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(dir.path());
        let store = DataStore::open_in(&data).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
        store
            .with(|c| {
                c.execute(
                    "INSERT INTO urls (url, title) VALUES ('https://a.test/', 'A')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);

        // Re-opening applies nothing and keeps the row.
        let store = DataStore::open_in(&data).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
        let n: i64 = store
            .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM urls", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 1);
        // The file sits where the layout says and uses WAL.
        assert!(data.db().join(DataStore::FILE_NAME).is_file());
        let mode: String = store
            .with(|c| Ok(c.pragma_query_value(None, "journal_mode", |r| r.get(0))?))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);
        let err = DataStore::open(&path).unwrap_err();
        assert!(matches!(err, DataError::Invalid(_)), "{err}");
    }

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("50%_a\\b"), "%50\\%\\_a\\\\b%");
    }
}
