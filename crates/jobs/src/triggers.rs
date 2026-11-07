//! Cron triggers: run a saved script on a schedule.
//!
//! `spec` is a cron expression in the browser's **local time**. The usual 5 fields
//! (`min hour day month weekday`, e.g. `"*/15 9-17 * * Mon-Fri"`) are accepted, as are 6 or 7
//! fields with seconds first (and an optional year), the format of the `cron` crate.
//! Triggers are stored in `runs.sqlite`. The scheduler checks once a second; firings missed
//! while the browser was closed are not caught up, and a trigger whose previous script is
//! still running skips that firing.
//!
//! TODO(webhook triggers): `kind: "webhook"` needs a new HTTP route in `crates/api`.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bmcp_core::CmdError;
#[cfg(test)]
use bmcp_core::ErrorCode;
use chrono::{DateTime, Local, TimeZone};
use cron::Schedule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::runner::ScriptSource;
use crate::store::{TriggerRecord, now_ms};
use crate::{Inner, topics};

/// Trigger kinds. Only `cron` for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Cron,
}

/// A trigger plus when it fires next.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TriggerInfo {
    #[serde(flatten)]
    pub trigger: TriggerRecord,
    /// Next firing, milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_fire_ms: Option<i64>,
    /// Next firing as local RFC 3339 time, e.g. `2026-11-06T09:00:00+02:00`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_fire: Option<String>,
}

/// Parse a cron spec (see the [module docs](self)); 5 fields get `0` seconds prepended.
pub fn parse_spec(spec: &str) -> Result<Schedule, CmdError> {
    let fields = spec.split_whitespace().count();
    let full = if fields == 5 {
        format!("0 {}", spec.trim())
    } else {
        spec.trim().to_string()
    };
    Schedule::from_str(&full).map_err(|e| {
        CmdError::invalid_params(format!("invalid cron spec {spec:?}: {e}")).with_hint(
            "Use 5 fields `min hour day month weekday` (e.g. \"0 9 * * Mon-Fri\") or 6 with \
             seconds first.",
        )
    })
}

/// The first firing strictly after `after`.
pub fn next_fire<Tz: TimeZone>(schedule: &Schedule, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    schedule.after(after).next()
}

impl Inner {
    pub fn set_trigger(
        &self,
        id: Option<String>,
        spec: &str,
        script: &str,
        inputs: Map<String, Value>,
    ) -> Result<TriggerInfo, CmdError> {
        let schedule = parse_spec(spec)?;
        self.scripts.require(script)?;
        crate::data::object_to_row(inputs.clone())?; // same column rules as run data
        let id = match id {
            Some(id) => {
                if id.is_empty() || id.len() > 64 {
                    return Err(CmdError::invalid_params("trigger id must be 1-64 chars"));
                }
                id
            }
            None => format!("trg-{}", now_ms()),
        };
        let record = TriggerRecord {
            id,
            kind: "cron".into(),
            spec: spec.trim().to_string(),
            script: script.to_string(),
            inputs,
            created_ms: now_ms(),
            last_fired_ms: None,
            last_status: None,
        };
        self.store.insert_trigger(&record)?;
        Ok(with_next(record, &schedule))
    }

    pub fn list_triggers(&self) -> Result<Vec<TriggerInfo>, CmdError> {
        Ok(self
            .store
            .triggers()?
            .into_iter()
            .map(|t| match parse_spec(&t.spec) {
                Ok(schedule) => with_next(t, &schedule),
                Err(_) => TriggerInfo {
                    trigger: t,
                    next_fire_ms: None,
                    next_fire: None,
                },
            })
            .collect())
    }

    pub fn remove_trigger(&self, id: &str) -> Result<bool, CmdError> {
        self.store.remove_trigger(id)
    }

    /// Start the script of every trigger due in `(from, to]`; returns their ids.
    pub fn fire_due(self: &Arc<Self>, from: DateTime<Local>, to: DateTime<Local>) -> Vec<String> {
        let triggers = match self.store.triggers() {
            Ok(t) => t,
            Err(err) => {
                tracing::error!(%err, "cannot read triggers");
                return Vec::new();
            }
        };
        let mut fired = Vec::new();
        for trigger in triggers {
            let Ok(schedule) = parse_spec(&trigger.spec) else {
                continue;
            };
            if !next_fire(&schedule, &from).is_some_and(|next| next <= to) {
                continue;
            }
            // Never run the same trigger twice at once.
            if !self
                .firing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(trigger.id.clone())
            {
                tracing::warn!(trigger = %trigger.id, "previous run still going; skipped");
                continue;
            }
            fired.push(trigger.id.clone());
            tokio::spawn(fire(self.clone(), trigger));
        }
        fired
    }
}

/// Run the trigger's script, record the outcome, emit `trigger.fired`.
async fn fire(inner: Arc<Inner>, trigger: TriggerRecord) {
    let inputs = crate::data::object_to_row(trigger.inputs.clone()).unwrap_or_default();
    let fired_ms = now_ms();
    let outcome = inner
        .run_script(
            ScriptSource::Name(&trigger.script),
            &inputs,
            &[],
            // Settings → Recording & scripts (data/config/scripts.json).
            Duration::from_secs(inner.settings().script_timeout_secs),
            &inner.shutdown,
        )
        .await;
    let (status, event) = match &outcome {
        Ok(o) => (
            o.error
                .as_ref()
                .map_or("ok".to_string(), |e| e.code.clone()),
            json!({"id": trigger.id, "script": trigger.script, "ok": o.ok,
                   "exit_code": o.exit_code, "duration_ms": o.duration_ms,
                   "result": o.result, "error": o.error}),
        ),
        Err(err) => (
            err.code.as_str().to_string(),
            json!({"id": trigger.id, "script": trigger.script, "ok": false,
                   "exit_code": null, "duration_ms": 0,
                   "error": {"code": err.code, "message": err.message}}),
        ),
    };
    if let Err(err) = inner.store.trigger_fired(&trigger.id, fired_ms, &status) {
        tracing::error!(%err, "cannot record trigger firing");
    }
    inner
        .firing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&trigger.id);
    inner.events.emit(topics::TRIGGER_FIRED, event);
}

/// The scheduler task: every second, fire what became due since the last check.
pub(crate) async fn scheduler(inner: Arc<Inner>) {
    let mut last = Local::now();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = inner.shutdown.cancelled() => break,
        }
        let now = Local::now();
        inner.fire_due(last, now);
        last = now;
    }
}

fn with_next(trigger: TriggerRecord, schedule: &Schedule) -> TriggerInfo {
    let next = next_fire(schedule, &Local::now());
    TriggerInfo {
        trigger,
        next_fire_ms: next.as_ref().map(|t| t.timestamp_millis()),
        next_fire: next.map(|t| t.to_rfc3339()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn five_field_specs_fire_on_the_minute() {
        let every_15 = parse_spec("*/15 * * * *").unwrap();
        assert_eq!(
            next_fire(&every_15, &at("2026-11-06T10:07:30Z")).unwrap(),
            at("2026-11-06T10:15:00Z")
        );
        // Strictly after: exactly on a firing time gives the next one.
        assert_eq!(
            next_fire(&every_15, &at("2026-11-06T10:15:00Z")).unwrap(),
            at("2026-11-06T10:30:00Z")
        );
        let weekdays_9 = parse_spec("0 9 * * Mon-Fri").unwrap();
        // 2026-11-06 is a Friday → next is Monday the 9th.
        assert_eq!(
            next_fire(&weekdays_9, &at("2026-11-06T09:00:00Z")).unwrap(),
            at("2026-11-09T09:00:00Z")
        );
    }

    #[test]
    fn six_field_specs_have_seconds() {
        let s = parse_spec("*/10 * * * * *").unwrap();
        assert_eq!(
            next_fire(&s, &at("2026-11-06T10:00:01Z")).unwrap(),
            at("2026-11-06T10:00:10Z")
        );
    }

    #[test]
    fn bad_specs_are_invalid_params() {
        for bad in ["", "every day", "61 * * * *", "* * *"] {
            assert_eq!(
                parse_spec(bad).unwrap_err().code,
                ErrorCode::InvalidParams,
                "{bad:?}"
            );
        }
    }
}
