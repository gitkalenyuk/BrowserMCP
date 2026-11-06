//! Persistence of runs, rows and triggers in `data/db/runs.sqlite`.
//!
//! The database is the source of truth: the runner claims rows from it and writes every row
//! result back right away, so `run.status` / `run.results` / `run.list` work at any time and
//! after a restart. Calls are short and synchronous (one `Mutex<Connection>`); SQLite in WAL
//! mode handles thousands of row updates per second, far more than script processes produce.

use std::path::Path;
use std::sync::Mutex;

use bmcp_core::{CmdError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::data::Row;
use crate::host::ScriptFailure;

/// Run-wide settings, fixed at `run.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunOptions {
    /// Worker slots actually used (after the CPU/RAM auto-limit).
    pub parallel: usize,
    /// What the caller asked for.
    pub parallel_requested: usize,
    /// Extra attempts per row after a failure.
    pub retries: u32,
    /// Per-row script timeout.
    pub timeout_secs: u64,
    /// Stop the whole run at the first failed row.
    pub stop_on_error: bool,
    /// Pause and ask for a patch when a row breaks on a page.* selector error.
    pub heal: bool,
    /// Browser profile the rows run in: `None`/"default", "isolated" (a fresh in-memory
    /// profile + off-screen tabs per row attempt), or a profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Run status: `running` → `finished` / `failed` (stop_on_error) / `cancelled`; `paused` in
/// between (by `run.pause`, a heal request, or a restart of the browser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Finished,
    Failed,
    Cancelled,
}

/// Row status: `pending` → `running` → `ok` / `failed` / `needs_heal` / `cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    Pending,
    Running,
    Ok,
    Failed,
    NeedsHeal,
    Cancelled,
}

macro_rules! text_enum {
    ($ty:ty { $($variant:ident = $text:literal),* $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),* }
            }
            /// Only we write these columns; an unknown value means a newer/corrupt database.
            fn parse(text: &str) -> rusqlite::Result<Self> {
                match text {
                    $($text => Ok(Self::$variant),)*
                    _ => Err(corrupt(format!("unknown {} {text:?}", stringify!($ty)))),
                }
            }
        }
    };
}
text_enum!(RunStatus { Running = "running", Paused = "paused", Finished = "finished", Failed = "failed", Cancelled = "cancelled" });
text_enum!(RowStatus { Pending = "pending", Running = "running", Ok = "ok", Failed = "failed", NeedsHeal = "needs_heal", Cancelled = "cancelled" });

impl RunStatus {
    /// Finished, failed or cancelled: nothing will run any more.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            RunStatus::Finished | RunStatus::Failed | RunStatus::Cancelled
        )
    }
}

/// One run as stored.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RunRecord {
    pub id: String,
    /// Script name; changes to `<name>.vN` after `run.heal_apply`.
    pub script: String,
    pub status: RunStatus,
    pub options: RunOptions,
    pub total: u64,
    pub created_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    /// The open heal request (`run.heal_needed` payload) while the run waits for a patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heal: Option<Value>,
    /// Why the run stopped or paused (stop_on_error, restart, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Rows per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Counts {
    pub pending: u64,
    pub running: u64,
    pub ok: u64,
    pub failed: u64,
    pub needs_heal: u64,
    pub cancelled: u64,
}

impl Counts {
    /// Rows that reached a final state (ok, failed or cancelled).
    pub fn done(&self) -> u64 {
        self.ok + self.failed + self.cancelled
    }
}

/// One row as stored.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RowRecord {
    /// 0-based row number in the input data.
    pub row: u64,
    pub status: RowStatus,
    /// The row's input columns.
    pub inputs: Map<String, Value>,
    /// Merged `browser.output(...)` values; `null` if none.
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptFailure>,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// End of the script's stdout (last attempt).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stdout_tail: String,
    /// The script (version) the last attempt ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip)]
    pub input_pairs: Row,
}

/// A row the runner took from the queue.
#[derive(Debug, Clone)]
pub struct ClaimedRow {
    pub idx: u64,
    pub inputs: Row,
    /// Attempts made before (non-zero after a restart in the middle of retries).
    pub attempts: u32,
}

/// The final result of a row, written by the runner.
#[derive(Debug, Clone)]
pub struct RowUpdate {
    pub status: RowStatus,
    pub attempts: u32,
    pub result: Value,
    pub error: Option<ScriptFailure>,
    pub duration_ms: u64,
    pub stdout_tail: String,
    pub script: String,
}

/// A cron trigger as stored.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TriggerRecord {
    pub id: String,
    /// Always `cron` for now.
    pub kind: String,
    /// The cron expression as given.
    pub spec: String,
    pub script: String,
    pub inputs: Map<String, Value>,
    pub created_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired_ms: Option<u64>,
    /// `ok`, or the error code of the last firing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS runs (
    id          TEXT PRIMARY KEY,
    script      TEXT NOT NULL,
    status      TEXT NOT NULL,
    options     TEXT NOT NULL,
    total       INTEGER NOT NULL,
    created_ms  INTEGER NOT NULL,
    finished_ms INTEGER,
    heal        TEXT,
    note        TEXT
);
CREATE TABLE IF NOT EXISTS rows (
    run_id      TEXT NOT NULL,
    idx         INTEGER NOT NULL,
    inputs      TEXT NOT NULL,
    status      TEXT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    result      TEXT,
    error       TEXT,
    duration_ms INTEGER,
    stdout_tail TEXT NOT NULL DEFAULT '',
    script      TEXT,
    PRIMARY KEY (run_id, idx)
);
CREATE INDEX IF NOT EXISTS rows_by_status ON rows (run_id, status, idx);
CREATE TABLE IF NOT EXISTS triggers (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    spec          TEXT NOT NULL,
    script        TEXT NOT NULL,
    inputs        TEXT NOT NULL,
    created_ms    INTEGER NOT NULL,
    last_fired_ms INTEGER,
    last_status   TEXT
);
";

impl Store {
    /// Open (or create) the database at `path`.
    pub fn open(path: &Path) -> Result<Store, CmdError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CmdError::internal(format!("{}: {e}", dir.display())))?;
        }
        let conn = Connection::open(path).map_err(db)?;
        conn.execute_batch(SCHEMA).map_err(db)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic mid-call cannot leave SQLite half-written (each call is one statement or a
        // transaction), so a poisoned lock is safe to reuse.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// After a restart: runs that were `running` become `paused` (their worker processes are
    /// gone) and their `running` rows go back to `pending`. Returns how many runs were paused.
    pub fn recover_interrupted(&self) -> Result<usize, CmdError> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(db)?;
        tx.execute(
            "UPDATE rows SET status = 'pending' WHERE status = 'running'",
            [],
        )
        .map_err(db)?;
        let paused = tx
            .execute(
                "UPDATE runs SET status = 'paused', note = 'interrupted by a browser restart; \
                 call run_resume to continue' WHERE status = 'running'",
                [],
            )
            .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(paused)
    }

    // ------------------------------------------------------------------ runs

    pub fn insert_run(&self, run: &RunRecord, rows: &[Row]) -> Result<(), CmdError> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(db)?;
        tx.execute(
            "INSERT INTO runs (id, script, status, options, total, created_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.id,
                run.script,
                run.status.as_str(),
                to_json(&run.options),
                run.total,
                run.created_ms
            ],
        )
        .map_err(db)?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO rows (run_id, idx, inputs, status) VALUES (?1, ?2, ?3, 'pending')",
                )
                .map_err(db)?;
            for (idx, row) in rows.iter().enumerate() {
                insert
                    .execute(params![run.id, idx as u64, to_json(row)])
                    .map_err(db)?;
            }
        }
        tx.commit().map_err(db)
    }

    pub fn run_exists(&self, id: &str) -> Result<bool, CmdError> {
        self.conn()
            .query_row("SELECT 1 FROM runs WHERE id = ?1", [id], |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
            .map_err(db)
    }

    /// The run, or `NOT_FOUND`.
    pub fn run(&self, id: &str) -> Result<RunRecord, CmdError> {
        self.conn()
            .query_row(
                "SELECT id, script, status, options, total, created_ms, finished_ms, heal, note \
                 FROM runs WHERE id = ?1",
                [id],
                run_from_sql,
            )
            .optional()
            .map_err(db)?
            .ok_or_else(|| {
                CmdError::new(ErrorCode::NotFound, format!("run '{id}' not found"))
                    .with_hint("Call run_list to see the runs.")
            })
    }

    /// Newest first.
    pub fn list_runs(&self, limit: u64) -> Result<Vec<RunRecord>, CmdError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, script, status, options, total, created_ms, finished_ms, heal, note \
                 FROM runs ORDER BY created_ms DESC, id DESC LIMIT ?1",
            )
            .map_err(db)?;
        stmt.query_map([limit], run_from_sql)
            .map_err(db)?
            .collect::<Result<_, _>>()
            .map_err(db)
    }

    pub fn counts(&self, id: &str) -> Result<Counts, CmdError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) FROM rows WHERE run_id = ?1 GROUP BY status")
            .map_err(db)?;
        let mut counts = Counts::default();
        let rows = stmt
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))
            .map_err(db)?;
        for row in rows {
            let (status, n) = row.map_err(db)?;
            match RowStatus::parse(&status).map_err(db)? {
                RowStatus::Pending => counts.pending = n,
                RowStatus::Running => counts.running = n,
                RowStatus::Ok => counts.ok = n,
                RowStatus::Failed => counts.failed = n,
                RowStatus::NeedsHeal => counts.needs_heal = n,
                RowStatus::Cancelled => counts.cancelled = n,
            }
        }
        Ok(counts)
    }

    /// Set the status; a final status also sets `finished_ms`. `note` replaces the old note
    /// (`None` clears it).
    pub fn set_status(
        &self,
        id: &str,
        status: RunStatus,
        note: Option<&str>,
    ) -> Result<(), CmdError> {
        let finished = status.is_final().then(now_ms);
        self.conn()
            .execute(
                "UPDATE runs SET status = ?2, note = ?3, finished_ms = ?4 WHERE id = ?1",
                params![id, status.as_str(), note, finished],
            )
            .map(drop)
            .map_err(db)
    }

    /// Store (or clear) the open heal request.
    pub fn set_heal(&self, id: &str, heal: Option<&Value>) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "UPDATE runs SET heal = ?2 WHERE id = ?1",
                params![id, heal.map(Value::to_string)],
            )
            .map(drop)
            .map_err(db)
    }

    pub fn set_script(&self, id: &str, script: &str) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "UPDATE runs SET script = ?2 WHERE id = ?1",
                params![id, script],
            )
            .map(drop)
            .map_err(db)
    }

    // ------------------------------------------------------------------ rows

    /// Take the next pending row (lowest index) and mark it `running`, atomically.
    pub fn claim_next(&self, id: &str) -> Result<Option<ClaimedRow>, CmdError> {
        self.conn()
            .query_row(
                "UPDATE rows SET status = 'running' WHERE run_id = ?1 AND idx = \
                 (SELECT idx FROM rows WHERE run_id = ?1 AND status = 'pending' ORDER BY idx LIMIT 1) \
                 RETURNING idx, inputs, attempts",
                [id],
                |r| {
                    Ok(ClaimedRow {
                        idx: r.get(0)?,
                        inputs: from_json(&r.get::<_, String>(1)?)?,
                        attempts: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(db)
    }

    /// Remember the attempt count while a row is still retrying (so a restart continues the
    /// count instead of starting over).
    pub fn set_attempts(&self, id: &str, idx: u64, attempts: u32) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "UPDATE rows SET attempts = ?3 WHERE run_id = ?1 AND idx = ?2",
                params![id, idx, attempts],
            )
            .map(drop)
            .map_err(db)
    }

    pub fn finish_row(&self, id: &str, idx: u64, update: &RowUpdate) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "UPDATE rows SET status = ?3, attempts = ?4, result = ?5, error = ?6, \
                 duration_ms = ?7, stdout_tail = ?8, script = ?9 WHERE run_id = ?1 AND idx = ?2",
                params![
                    id,
                    idx,
                    update.status.as_str(),
                    update.attempts,
                    update.result.to_string(),
                    update.error.as_ref().map(to_json),
                    update.duration_ms,
                    update.stdout_tail,
                    update.script,
                ],
            )
            .map(drop)
            .map_err(db)
    }

    /// Move every row in status `from` to `to`; returns how many. Moving to `pending` also
    /// resets the attempt count (a fresh start, e.g. with a healed script).
    pub fn move_rows(&self, id: &str, from: RowStatus, to: RowStatus) -> Result<usize, CmdError> {
        let sql = if to == RowStatus::Pending {
            "UPDATE rows SET status = ?3, attempts = 0 WHERE run_id = ?1 AND status = ?2"
        } else {
            "UPDATE rows SET status = ?3 WHERE run_id = ?1 AND status = ?2"
        };
        self.conn()
            .execute(sql, params![id, from.as_str(), to.as_str()])
            .map_err(db)
    }

    /// Rows with `idx > after`, in order; optionally only one status.
    pub fn rows(
        &self,
        id: &str,
        after: Option<u64>,
        limit: u64,
        status: Option<RowStatus>,
    ) -> Result<Vec<RowRecord>, CmdError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT idx, status, inputs, result, error, attempts, duration_ms, stdout_tail, script \
                 FROM rows WHERE run_id = ?1 AND idx > ?2 AND (?3 IS NULL OR status = ?3) \
                 ORDER BY idx LIMIT ?4",
            )
            .map_err(db)?;
        // -1 so `after = None` includes row 0.
        let after = after.map_or(-1, |a| a as i64);
        stmt.query_map(
            params![id, after, status.map(RowStatus::as_str), limit],
            |r| {
                let input_pairs: Row = from_json(&r.get::<_, String>(2)?)?;
                Ok(RowRecord {
                    row: r.get(0)?,
                    status: RowStatus::parse(&r.get::<_, String>(1)?)?,
                    inputs: input_pairs
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect(),
                    result: match r.get::<_, Option<String>>(3)? {
                        Some(s) => from_json(&s)?,
                        None => Value::Null,
                    },
                    error: r
                        .get::<_, Option<String>>(4)?
                        .map(|s| from_json(&s))
                        .transpose()?,
                    attempts: r.get(5)?,
                    duration_ms: r.get(6)?,
                    stdout_tail: r.get(7)?,
                    script: r.get(8)?,
                    input_pairs,
                })
            },
        )
        .map_err(db)?
        .collect::<Result<_, _>>()
        .map_err(db)
    }

    // ------------------------------------------------------------------ triggers

    pub fn insert_trigger(&self, t: &TriggerRecord) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "INSERT OR REPLACE INTO triggers (id, kind, spec, script, inputs, created_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    t.id,
                    t.kind,
                    t.spec,
                    t.script,
                    Value::Object(t.inputs.clone()).to_string(),
                    t.created_ms
                ],
            )
            .map(drop)
            .map_err(db)
    }

    pub fn triggers(&self) -> Result<Vec<TriggerRecord>, CmdError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, spec, script, inputs, created_ms, last_fired_ms, last_status \
                 FROM triggers ORDER BY created_ms, id",
            )
            .map_err(db)?;
        stmt.query_map([], |r| {
            Ok(TriggerRecord {
                id: r.get(0)?,
                kind: r.get(1)?,
                spec: r.get(2)?,
                script: r.get(3)?,
                inputs: from_json(&r.get::<_, String>(4)?)?,
                created_ms: r.get(5)?,
                last_fired_ms: r.get(6)?,
                last_status: r.get(7)?,
            })
        })
        .map_err(db)?
        .collect::<Result<_, _>>()
        .map_err(db)
    }

    pub fn remove_trigger(&self, id: &str) -> Result<bool, CmdError> {
        self.conn()
            .execute("DELETE FROM triggers WHERE id = ?1", [id])
            .map(|n| n > 0)
            .map_err(db)
    }

    pub fn trigger_fired(&self, id: &str, at_ms: u64, status: &str) -> Result<(), CmdError> {
        self.conn()
            .execute(
                "UPDATE triggers SET last_fired_ms = ?2, last_status = ?3 WHERE id = ?1",
                params![id, at_ms, status],
            )
            .map(drop)
            .map_err(db)
    }
}

fn run_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: r.get(0)?,
        script: r.get(1)?,
        status: RunStatus::parse(&r.get::<_, String>(2)?)?,
        options: from_json(&r.get::<_, String>(3)?)?,
        total: r.get(4)?,
        created_ms: r.get(5)?,
        finished_ms: r.get(6)?,
        heal: r
            .get::<_, Option<String>>(7)?
            .map(|s| from_json(&s))
            .transpose()?,
        note: r.get(8)?,
    })
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("plain data always serializes")
}

/// Only we write these columns, so bad JSON means a corrupted database.
fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|e| corrupt(format!("bad JSON: {e}")))
}

fn corrupt(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, message.into())
}

fn db(err: rusqlite::Error) -> CmdError {
    CmdError::internal(format!("runs database: {err}"))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
