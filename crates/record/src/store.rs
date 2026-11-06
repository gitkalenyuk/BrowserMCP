//! Files on disk and the in-memory list of running recordings.
//!
//! ```text
//! data/recordings/<id>/actions.jsonl   one Step per line
//! data/recordings/<id>/meta.json       RecordingMeta
//! data/recordings/<id>/traffic.jsonl   (traffic: true)
//! data/recordings/<id>/video.webm      (video: true)
//! data/scripts/<name>.ts               record.codegen output
//! data/videos/<id>.webm                record.video_start without a recording
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bmcp_core::{CmdError, Ctx, DataDir, ErrorCode};

use crate::model::{RecordingMeta, Step};

/// The portable data folder, shared through `Extensions`. The app inserts it at startup
/// (`registry.extensions().insert(DataDirExt(data))`); without it we fall back to the default
/// `data/` folder next to the executable.
#[derive(Debug, Clone)]
pub struct DataDirExt(pub DataDir);

/// The data folder for this call.
pub fn data_dir(ctx: &Ctx) -> Result<DataDir, CmdError> {
    match ctx.extensions.get::<DataDirExt>() {
        Some(ext) => Ok(ext.0.clone()),
        None => DataDir::resolve(None).map_err(|e| CmdError::internal(e.to_string())),
    }
}

pub fn recordings_dir(data: &DataDir) -> PathBuf {
    data.root().join("recordings")
}

pub fn scripts_dir(data: &DataDir) -> PathBuf {
    data.root().join("scripts")
}

pub fn videos_dir(data: &DataDir) -> PathBuf {
    data.root().join("videos")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `"Login flow!"` → `"login-flow"`: safe as a file name and as an id part.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    let out: String = out.chars().take(40).collect();
    if out.is_empty() {
        "recording".to_string()
    } else {
        out
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) for a Unix time in milliseconds.
pub fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A new, not yet existing folder `data/recordings/<slug>-<stamp>[-N]`; returns `(id, dir)`.
pub fn new_recording_dir(data: &DataDir, name: &str) -> Result<(String, PathBuf), CmdError> {
    let base = format!("{}-{}", slug(name), stamp(now_ms()));
    let root = recordings_dir(data);
    for n in 1..1000 {
        let id = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let dir = root.join(&id);
        if !dir.exists() {
            std::fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
            return Ok((id, dir));
        }
    }
    Err(CmdError::internal(
        "cannot find a free recording folder name",
    ))
}

pub fn io_error(path: &Path, err: std::io::Error) -> CmdError {
    CmdError::internal(format!("{}: {err}", path.display()))
}

/// Ids are folder names: refuse anything that could leave `data/recordings/`.
pub fn check_id(id: &str) -> Result<(), CmdError> {
    let ok = !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "invalid recording id {id:?}"
        )))
    }
}

/// The steps of a recording, kept in memory while it runs and rewritten to `actions.jsonl`
/// after every change (recordings are small: rewriting is simpler than patching lines).
#[derive(Debug)]
pub struct Log {
    pub steps: Vec<Step>,
    next_seq: u64,
    path: PathBuf,
}

impl Log {
    pub fn new(path: PathBuf) -> Log {
        Log {
            steps: Vec::new(),
            next_seq: 1,
            path,
        }
    }

    /// Append a step (its `seq` is set here) and return the seq.
    pub fn push(&mut self, mut step: Step) -> u64 {
        step.seq = self.next_seq;
        self.next_seq += 1;
        self.steps.push(step);
        self.next_seq - 1
    }

    pub fn get_mut(&mut self, seq: u64) -> Option<&mut Step> {
        self.steps.iter_mut().find(|s| s.seq == seq)
    }

    /// The finished steps (not `pending`), in order.
    pub fn written(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| !s.pending)
    }

    /// Write every finished step to `actions.jsonl`.
    pub fn save(&self) -> Result<(), CmdError> {
        let mut text = String::new();
        for step in self.written() {
            text.push_str(&serde_json::to_string(step).unwrap_or_default());
            text.push('\n');
        }
        std::fs::write(&self.path, text).map_err(|e| io_error(&self.path, e))
    }
}

/// Read `actions.jsonl` (lines that do not parse are skipped).
pub fn read_steps(dir: &Path) -> Result<Vec<Step>, CmdError> {
    let path = dir.join("actions.jsonl");
    let text = std::fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

pub fn read_meta(dir: &Path) -> Result<RecordingMeta, CmdError> {
    let path = dir.join("meta.json");
    let text = std::fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
    serde_json::from_str(&text).map_err(|e| CmdError::internal(format!("{}: {e}", path.display())))
}

pub fn write_meta(dir: &Path, meta: &RecordingMeta) -> Result<(), CmdError> {
    let path = dir.join("meta.json");
    let text = serde_json::to_string_pretty(meta).unwrap_or_default();
    std::fs::write(&path, text).map_err(|e| io_error(&path, e))
}

/// Every recording on disk, newest first.
pub fn list_on_disk(data: &DataDir) -> Vec<RecordingMeta> {
    let Ok(entries) = std::fs::read_dir(recordings_dir(data)) else {
        return Vec::new();
    };
    let mut out: Vec<RecordingMeta> = entries
        .filter_map(Result::ok)
        .filter_map(|e| read_meta(&e.path()).ok())
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.started_ms));
    out
}

/// A running recording, as seen by the commands (the session task owns the rest).
pub struct Active {
    pub id: String,
    pub dir: PathBuf,
    pub meta: Arc<Mutex<RecordingMeta>>,
    pub log: Arc<Mutex<Log>>,
    /// Dropping or sending on it ends the session task.
    pub stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The session task; it returns the final meta.
    pub task: Mutex<Option<tokio::task::JoinHandle<RecordingMeta>>>,
}

/// All running recordings (an `Extensions` service).
#[derive(Default)]
pub struct Recordings {
    active: Mutex<HashMap<String, Arc<Active>>>,
}

impl Recordings {
    pub fn from_ctx(ctx: &Ctx) -> Arc<Recordings> {
        ctx.extensions.get_or_insert_with(Recordings::default)
    }

    pub fn insert(&self, active: Arc<Active>) {
        self.lock().insert(active.id.clone(), active);
    }

    pub fn get(&self, id: &str) -> Option<Arc<Active>> {
        self.lock().get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Active>> {
        self.lock().remove(id)
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.lock().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// `Some(id)` → that one; `None` → the only running recording.
    pub fn pick(&self, id: Option<&str>) -> Result<Arc<Active>, CmdError> {
        if let Some(id) = id {
            return self.get(id).ok_or_else(|| {
                CmdError::new(
                    ErrorCode::NotFound,
                    format!("no running recording with id {id:?}"),
                )
                .with_hint("record_list shows the recordings; only running ones can be stopped.")
            });
        }
        let ids = self.ids();
        match ids.as_slice() {
            [one] => Ok(self.get(one).expect("id was just listed")),
            [] => Err(CmdError::new(
                ErrorCode::NotFound,
                "no recording is running",
            )),
            _ => Err(CmdError::invalid_params(format!(
                "several recordings are running ({}); pass id",
                ids.join(", ")
            ))),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Active>>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Lock a std mutex, ignoring poisoning (a panic elsewhere does not corrupt a `Vec` push).
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_safe() {
        assert_eq!(slug("Login flow!"), "login-flow");
        assert_eq!(slug("  ../../etc  "), "etc");
        assert_eq!(slug("Вхід"), "recording");
        assert_eq!(slug(""), "recording");
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(stamp(0), "19700101-000000");
        // 2026-11-06 12:56:00 UTC
        assert_eq!(stamp(1_793_969_760_000), "20261106-125600");
        assert_eq!(stamp(951_782_400_000), "20000229-000000");
    }

    #[test]
    fn ids_cannot_escape() {
        assert!(check_id("login-20261106-125600").is_ok());
        for bad in ["", "..", "a/b", "a\\b", "a b"] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pending_steps_are_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let mut log = Log::new(tmp.path().join("actions.jsonl"));
        log.push(Step {
            action: "goto".into(),
            ..Step::default()
        });
        log.push(Step {
            action: "fill".into(),
            value: Some("hunter2".into()),
            pending: true,
            ..Step::default()
        });
        log.save().unwrap();
        let steps = read_steps(tmp.path()).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].seq, 1);
        assert_eq!(steps[0].action, "goto");
    }
}
