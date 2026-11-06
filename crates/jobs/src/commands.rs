//! The registry commands of this crate. Each one is a thin wrapper over the [`JobsService`].
//!
//! | Command | What |
//! |---|---|
//! | `script.list/get/save/delete` | the library in `data/scripts/*.ts` |
//! | `script.run` | run a saved script or inline code once, wait, return output + result |
//! | `run.start` | run a script once per data row (CSV/JSON/Excel/inline), N in parallel |
//! | `run.status/list/results` | progress, run list, paged row results |
//! | `run.pause/resume/cancel` | control a run |
//! | `run.export` | csv/xlsx/json/sqlite/webhook |
//! | `run.heal_apply` | continue a run paused by `run.heal_needed` with a patched script |
//! | `trigger.set/list/remove` | cron triggers that run a script |

use std::sync::Arc;
use std::time::Duration;

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset, command::Empty};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::data::{DataArgs, load_rows, object_to_row};
use crate::export::{ExportArgs, ExportOutput};
use crate::host::HostOutcome;
use crate::runner::{DEFAULT_TIMEOUT_SECS, ScriptSource, StartRequest};
use crate::scripts::ScriptInfo;
use crate::store::{Counts, RowRecord, RowStatus, RunRecord};
use crate::triggers::{TriggerInfo, TriggerKind};
use crate::{Inner, JobsService};

fn service(ctx: &Ctx) -> Result<Arc<Inner>, CmdError> {
    ctx.extensions
        .get::<JobsService>()
        .map(|s| s.inner.clone())
        .ok_or_else(|| CmdError::internal("the jobs service is not running"))
}

// ------------------------------------------------------------------ script.*

pub struct ScriptList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScriptListOutput {
    pub scripts: Vec<ScriptInfo>,
}

impl Command for ScriptList {
    const NAME: &'static str = "script.list";
    type Args = Empty;
    type Output = ScriptListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List saved scripts (data/scripts/*.ts) with size and modification time. Healed \
             versions appear as <name>.v2, <name>.v3, …",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ScriptListOutput, CmdError> {
        Ok(ScriptListOutput {
            scripts: service(&ctx)?.scripts.list()?,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameArgs {
    /// Script name without .ts, e.g. "login" or "login.v2".
    pub name: String,
}

pub struct ScriptGet;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScriptGetOutput {
    pub name: String,
    /// TypeScript source.
    pub code: String,
}

impl Command for ScriptGet {
    const NAME: &'static str = "script.get";
    type Args = NameArgs;
    type Output = ScriptGetOutput;

    fn meta() -> Meta {
        Meta::new("Return the source code of a saved script.", Toolset::Script)
    }

    async fn run(ctx: Ctx, args: NameArgs) -> Result<ScriptGetOutput, CmdError> {
        let code = service(&ctx)?.scripts.get(&args.name)?;
        Ok(ScriptGetOutput {
            name: args.name,
            code,
        })
    }
}

pub struct ScriptSave;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptSaveArgs {
    /// 1-64 chars of A-Z a-z 0-9 _ - (no .ts); overwrites an existing script.
    pub name: String,
    /// TypeScript source. Globals: browser.<ns>.<method>(args) calls a browser command,
    /// inputs.<key> (strings), browser.output({...}) reports a structured result, sleep(ms).
    pub code: String,
}

impl Command for ScriptSave {
    const NAME: &'static str = "script.save";
    type Args = ScriptSaveArgs;
    type Output = ScriptInfo;

    fn meta() -> Meta {
        Meta::new(
            "Create or overwrite a saved TypeScript script in data/scripts/<name>.ts.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ScriptSaveArgs) -> Result<ScriptInfo, CmdError> {
        service(&ctx)?.scripts.save(&args.name, &args.code)
    }
}

pub struct ScriptDelete;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScriptDeleteOutput {
    /// False when the script did not exist.
    pub deleted: bool,
}

impl Command for ScriptDelete {
    const NAME: &'static str = "script.delete";
    type Args = NameArgs;
    type Output = ScriptDeleteOutput;

    fn meta() -> Meta {
        Meta::new("Delete a saved script file.", Toolset::Script).act()
    }

    async fn run(ctx: Ctx, args: NameArgs) -> Result<ScriptDeleteOutput, CmdError> {
        Ok(ScriptDeleteOutput {
            deleted: service(&ctx)?.scripts.delete(&args.name)?,
        })
    }
}

pub struct ScriptRun;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptRunArgs {
    /// Saved script to run. Give `name` or `code`.
    pub name: Option<String>,
    /// Inline TypeScript to run instead of a saved script.
    pub code: Option<String>,
    /// Values the script reads as inputs.<key> (passed as strings).
    pub inputs: Option<Map<String, Value>>,
    /// Kill the script after this many seconds (default 300).
    pub timeout_secs: Option<u64>,
}

impl Command for ScriptRun {
    const NAME: &'static str = "script.run";
    type Args = ScriptRunArgs;
    type Output = HostOutcome;

    fn meta() -> Meta {
        Meta::new(
            "Run a saved script (name) or inline TypeScript (code) once in the script host and \
             wait. Returns ok, exit_code, duration_ms, result (from browser.output), stdout/stderr \
             tails and a parsed error {code, command, message, line}.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ScriptRunArgs) -> Result<HostOutcome, CmdError> {
        let inner = service(&ctx)?;
        let source = match (&args.name, &args.code) {
            (Some(name), None) => ScriptSource::Name(name),
            (None, Some(code)) => ScriptSource::Code(code),
            _ => {
                return Err(CmdError::invalid_params(
                    "give exactly one of `name` or `code`",
                ));
            }
        };
        let inputs = object_to_row(args.inputs.unwrap_or_default())?;
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1));
        // A shutdown of the browser also stops a script started by a call.
        let cancel: CancellationToken = inner.shutdown.child_token();
        inner
            .run_script(source, &inputs, &[], timeout, &cancel)
            .await
    }
}

// ------------------------------------------------------------------ run.*

/// A run with its row counts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunInfo {
    #[serde(flatten)]
    pub run: RunRecord,
    pub counts: Counts,
    /// Rows that reached a final state (ok + failed + cancelled).
    pub done: u64,
}

fn run_info(inner: &Inner, id: &str) -> Result<RunInfo, CmdError> {
    let run = inner.store.run(id)?;
    let counts = inner.store.counts(id)?;
    Ok(RunInfo {
        done: counts.done(),
        run,
        counts,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunIdArgs {
    /// Run id from run_start / run_list.
    pub id: String,
}

pub struct RunStart;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunStartArgs {
    /// Saved script to run once per row.
    pub script: String,
    /// The rows: exactly one of csv_path, json_path, xlsx_path or rows.
    pub data: DataArgs,
    /// Rows at once (default 4; limited to CPU count / 2 and free RAM / 400 MB).
    pub parallel: Option<usize>,
    /// Extra attempts for a failed row, with exponential backoff (default 1).
    pub retries: Option<u32>,
    /// Per-row timeout in seconds (default 300).
    pub timeout_secs: Option<u64>,
    /// Stop the run at the first failed row (default false).
    pub stop_on_error: Option<bool>,
    /// Pause and emit run.heal_needed when a row breaks on a page.* selector error
    /// (NOT_FOUND, NOT_INTERACTABLE, STALE_REF, TIMEOUT, AMBIGUOUS_TARGET). Default true.
    pub heal: Option<bool>,
    /// Browser profile of the rows: "default" (shared, tabs in the browser window), "isolated"
    /// (each row gets a fresh in-memory profile and off-screen tabs: parallel rows never
    /// share cookies, focus or keyboard), or a profile id (profile_create; shared by all rows).
    pub profile: Option<String>,
}

impl Command for RunStart {
    const NAME: &'static str = "run.start";
    type Args = RunStartArgs;
    type Output = RunInfo;

    fn meta() -> Meta {
        Meta::new(
            "Start a mass run: the script runs once per data row (columns → inputs.<col>), \
             `parallel` at a time, with retries. Returns at once with the run id; follow it with \
             run_status / run_results or the run.progress events.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RunStartArgs) -> Result<RunInfo, CmdError> {
        let inner = service(&ctx)?;
        let rows = load_rows(&args.data)?;
        let run = inner.start_run(StartRequest {
            script: args.script,
            rows,
            parallel: args.parallel,
            retries: args.retries,
            timeout_secs: args.timeout_secs,
            stop_on_error: args.stop_on_error.unwrap_or(false),
            heal: args.heal.unwrap_or(true),
            profile: args.profile,
        })?;
        run_info(&inner, &run.id)
    }
}

pub struct RunStatusCmd;

impl Command for RunStatusCmd {
    const NAME: &'static str = "run.status";
    type Args = RunIdArgs;
    type Output = RunInfo;

    fn meta() -> Meta {
        Meta::new(
            "Status of a run: running/paused/finished/failed/cancelled, row counts, options and \
             an open heal request.",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, args: RunIdArgs) -> Result<RunInfo, CmdError> {
        let inner = service(&ctx)?;
        run_info(&inner, &args.id)
    }
}

pub struct RunList;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunListArgs {
    /// Max runs, newest first (default 50).
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunListOutput {
    pub runs: Vec<RunInfo>,
}

impl Command for RunList {
    const NAME: &'static str = "run.list";
    type Args = RunListArgs;
    type Output = RunListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List runs (also from before a browser restart), newest first, with row counts.",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, args: RunListArgs) -> Result<RunListOutput, CmdError> {
        let inner = service(&ctx)?;
        let runs = inner
            .store
            .list_runs(args.limit.unwrap_or(50))?
            .into_iter()
            .map(|run| {
                let counts = inner.store.counts(&run.id)?;
                Ok(RunInfo {
                    done: counts.done(),
                    run,
                    counts,
                })
            })
            .collect::<Result<_, CmdError>>()?;
        Ok(RunListOutput { runs })
    }
}

pub struct RunResults;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunResultsArgs {
    pub id: String,
    /// `next_cursor` from the previous page; omit for the first page.
    pub cursor: Option<u64>,
    /// Rows per page (default 100, max 1000).
    pub limit: Option<u64>,
    /// Only rows with this status.
    pub status: Option<RowStatus>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunResultsOutput {
    pub id: String,
    pub rows: Vec<RowRecord>,
    /// Pass as `cursor` for the next page; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<u64>,
}

impl Command for RunResults {
    const NAME: &'static str = "run.results";
    type Args = RunResultsArgs;
    type Output = RunResultsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Row results of a run, paged by cursor: status, inputs, result, error, attempts, \
             duration and stdout tail per row.",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, args: RunResultsArgs) -> Result<RunResultsOutput, CmdError> {
        let inner = service(&ctx)?;
        inner.store.run(&args.id)?; // NOT_FOUND for unknown runs
        let limit = args.limit.unwrap_or(100).clamp(1, 1000);
        let rows = inner
            .store
            .rows(&args.id, args.cursor, limit, args.status)?;
        let next_cursor = if rows.len() as u64 == limit {
            rows.last().map(|r| r.row)
        } else {
            None
        };
        Ok(RunResultsOutput {
            id: args.id,
            rows,
            next_cursor,
        })
    }
}

pub struct RunPause;

impl Command for RunPause {
    const NAME: &'static str = "run.pause";
    type Args = RunIdArgs;
    type Output = RunInfo;

    fn meta() -> Meta {
        Meta::new(
            "Pause a run: rows already running finish, no new rows start.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RunIdArgs) -> Result<RunInfo, CmdError> {
        let inner = service(&ctx)?;
        inner.pause(&args.id)?;
        run_info(&inner, &args.id)
    }
}

pub struct RunResume;

impl Command for RunResume {
    const NAME: &'static str = "run.resume";
    type Args = RunIdArgs;
    type Output = RunInfo;

    fn meta() -> Meta {
        Meta::new(
            "Resume a paused run (also one interrupted by a browser restart). Rows waiting for \
             a heal patch are marked failed; use run_heal_apply to fix them instead.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RunIdArgs) -> Result<RunInfo, CmdError> {
        let inner = service(&ctx)?;
        inner.resume(&args.id)?;
        run_info(&inner, &args.id)
    }
}

pub struct RunCancel;

impl Command for RunCancel {
    const NAME: &'static str = "run.cancel";
    type Args = RunIdArgs;
    type Output = RunInfo;

    fn meta() -> Meta {
        Meta::new(
            "Cancel a run: kills running scripts, marks the remaining rows cancelled.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RunIdArgs) -> Result<RunInfo, CmdError> {
        let inner = service(&ctx)?;
        inner.cancel(&args.id)?;
        run_info(&inner, &args.id)
    }
}

pub struct RunExport;

impl Command for RunExport {
    const NAME: &'static str = "run.export";
    type Args = ExportArgs;
    type Output = ExportOutput;

    fn meta() -> Meta {
        Meta::new(
            "Export a run's rows to csv, xlsx, json or sqlite (default data/runs/<id>/results.<ext>) \
             or POST them to a webhook in JSON batches. Columns: row, status, inputs, result \
             keys, error, attempts, duration.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ExportArgs) -> Result<ExportOutput, CmdError> {
        service(&ctx)?.export(args).await
    }
}

pub struct RunHealApply;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealApplyArgs {
    pub id: String,
    /// The fixed script source; saved as the next version (<name>.v2, .v3, …).
    pub patched_code: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HealApplyOutput {
    pub id: String,
    /// The new script version the run continues with.
    pub script: String,
    /// Rows put back into the queue.
    pub requeued: usize,
}

impl Command for RunHealApply {
    const NAME: &'static str = "run.heal_apply";
    type Args = HealApplyArgs;
    type Output = HealApplyOutput;

    fn meta() -> Meta {
        Meta::new(
            "Answer a run.heal_needed pause: save the patched script as a new version, re-queue \
             the broken rows and resume the run with the new version.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: HealApplyArgs) -> Result<HealApplyOutput, CmdError> {
        let (script, requeued) = service(&ctx)?.heal_apply(&args.id, &args.patched_code)?;
        Ok(HealApplyOutput {
            id: args.id,
            script,
            requeued,
        })
    }
}

// ------------------------------------------------------------------ trigger.*

pub struct TriggerSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerSetArgs {
    /// Only "cron" for now.
    pub kind: TriggerKind,
    /// Cron expression in local time: 5 fields "min hour day month weekday"
    /// (e.g. "0 9 * * Mon-Fri") or 6 with seconds first.
    pub spec: String,
    /// Saved script to run.
    pub script: String,
    /// inputs.<key> for the script.
    pub inputs: Option<Map<String, Value>>,
    /// Replace the trigger with this id (default: a new id).
    pub id: Option<String>,
}

impl Command for TriggerSet {
    const NAME: &'static str = "trigger.set";
    type Args = TriggerSetArgs;
    type Output = TriggerInfo;

    fn meta() -> Meta {
        Meta::new(
            "Create or replace a cron trigger that runs a saved script on a schedule. Returns \
             the trigger with its next firing time.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TriggerSetArgs) -> Result<TriggerInfo, CmdError> {
        let TriggerKind::Cron = args.kind;
        service(&ctx)?.set_trigger(
            args.id,
            &args.spec,
            &args.script,
            args.inputs.unwrap_or_default(),
        )
    }
}

pub struct TriggerList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct TriggerListOutput {
    pub triggers: Vec<TriggerInfo>,
}

impl Command for TriggerList {
    const NAME: &'static str = "trigger.list";
    type Args = Empty;
    type Output = TriggerListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List triggers with their next and last firing.",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<TriggerListOutput, CmdError> {
        Ok(TriggerListOutput {
            triggers: service(&ctx)?.list_triggers()?,
        })
    }
}

pub struct TriggerRemove;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerRemoveArgs {
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TriggerRemoveOutput {
    /// False when no trigger had that id.
    pub removed: bool,
}

impl Command for TriggerRemove {
    const NAME: &'static str = "trigger.remove";
    type Args = TriggerRemoveArgs;
    type Output = TriggerRemoveOutput;

    fn meta() -> Meta {
        Meta::new("Delete a trigger.", Toolset::Run).act()
    }

    async fn run(ctx: Ctx, args: TriggerRemoveArgs) -> Result<TriggerRemoveOutput, CmdError> {
        Ok(TriggerRemoveOutput {
            removed: service(&ctx)?.remove_trigger(&args.id)?,
        })
    }
}
