//! Patch proposals for a paused run: propose → (accept | reject with feedback → propose …).
//!
//! A **session** is one heal request (one run, the broken row, the script version it ran). It
//! allows `max_attempts` AI calls in total: a reply that fails validation is sent back to the
//! model automatically with the problem, and each `run.heal_reject` feedback starts the next
//! attempt. When the attempts are used up, `run.heal_failed` is emitted and the run stays paused
//! (fix it by hand with `run.heal_apply`, or `run.resume` to skip the broken rows).
//!
//! Proposals live in memory: after a browser restart, call `run.heal_propose` again.

use std::collections::HashMap;

use bmcp_ai::oneshot::TextRequest;
use bmcp_core::{CmdError, ErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::context::{HealContext, SYSTEM};
use crate::patch::{ValidateInput, parse_answer, validate};
use crate::{HealService, topics};

/// Where a proposal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Waiting for `run.heal_accept` / `run.heal_reject`.
    Pending,
    Accepted,
    Rejected,
    /// A newer proposal for the same run replaced it.
    Superseded,
}

/// A validated patch for a paused run.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Proposal {
    pub proposal_id: String,
    /// The run.
    pub id: String,
    /// The row that broke.
    pub row: u64,
    /// The script version the row ran (the patch becomes the next version).
    pub script: String,
    /// Unified diff of the change.
    pub diff: String,
    pub explanation: String,
    /// The model's own estimate, 0..1.
    pub confidence: f64,
    /// 1-based attempt of this heal request.
    pub attempt: u32,
    pub max_attempts: u32,
    /// What was validated, e.g. ["applies", "region 12-42", "transpiles"].
    pub checks: Vec<String>,
    pub status: ProposalStatus,
    /// AI provider and model that wrote it.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_ms: u64,
    /// The whole patched script (not sent in events; `run.heal_proposals` returns it).
    #[serde(skip)]
    pub patched_code: String,
    /// Creation order in this browser session (see [`HealService::reconcile`]).
    #[serde(skip)]
    seq: u64,
}

/// One heal request (see the [module docs](self)).
#[derive(Debug, Clone)]
struct Session {
    row: u64,
    script: String,
    attempts: u32,
    /// Problems of earlier attempts, sent back to the model.
    history: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    /// The run's heal mode (`ask` / `auto`), for the events.
    mode: String,
}

impl Session {
    fn new(row: u64, script: &str) -> Self {
        Session {
            row,
            script: script.to_string(),
            attempts: 0,
            history: Vec::new(),
            provider: None,
            model: None,
            mode: "ask".into(),
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    pub proposals: HashMap<String, Proposal>,
    sessions: HashMap<String, Session>,
    /// Patches applied by "auto" mode, per run.
    auto_applied: HashMap<String, u32>,
    pub tabs: crate::watch::TabTracker,
    next: u64,
}

/// What `run.heal_accept` returns.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Applied {
    pub id: String,
    pub proposal_id: String,
    /// The new script version the run continues with.
    pub script: String,
    /// Rows put back into the queue.
    pub requeued: u64,
}

fn now_ms() -> u64 {
    bmcp_jobs::store::now_ms()
}

/// The open heal request of a run plus its run options.
struct Request {
    ctx: HealContext,
    mode: String,
}

impl HealService {
    /// Everything about the paused run the AI needs (see [`HealContext`]).
    async fn gather(&self, run_id: &str) -> Result<Request, CmdError> {
        let run = self.call("run.status", json!({ "id": run_id })).await?;
        let Some(heal) = run.get("heal").filter(|h| h.is_object()) else {
            return Err(CmdError::invalid_params(format!(
                "run '{run_id}' has no open heal request"
            ))
            .with_hint(
                "Healing works while a run is paused by run.heal_needed (see run_status).",
            ));
        };
        let script = heal["script"].as_str().unwrap_or_default().to_string();
        let row = heal["row"].as_u64().unwrap_or_default();
        let code = self.call("script.get", json!({ "name": script })).await?["code"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let snapshot = self
            .jobs()
            .and_then(|jobs| jobs.heal_snapshot(run_id, Some(row)).ok().flatten());
        Ok(Request {
            ctx: HealContext {
                run: run_id.to_string(),
                row,
                script,
                code,
                error: heal["error"].clone(),
                line: heal["line"].as_u64().map(|l| l as u32),
                snapshot,
            },
            mode: run["options"]["heal_mode"]
                .as_str()
                .unwrap_or("ask")
                .to_string(),
        })
    }

    /// Ask the AI for a patch for the run's open heal request and validate it; failed
    /// validations are retried with the problem until the attempts are used up.
    /// `feedback` is added to the session's history first.
    pub async fn propose(
        &self,
        run_id: &str,
        provider: Option<String>,
        model: Option<String>,
        feedback: Option<String>,
    ) -> Result<Proposal, CmdError> {
        let _one_at_a_time = self.propose_lock.lock().await;
        let req = self.gather(run_id).await?;
        let ctx = &req.ctx;
        let max = self.config.max_attempts;
        {
            let mut state = self.state();
            let session = state
                .sessions
                .entry(run_id.to_string())
                .or_insert_with(|| Session::new(ctx.row, &ctx.script));
            // A new heal request of the same run (another row broke later): start over.
            if session.row != ctx.row || session.script != ctx.script {
                *session = Session::new(ctx.row, &ctx.script);
            }
            session.mode = req.mode.clone();
            if provider.is_some() {
                session.provider = provider;
            }
            if model.is_some() {
                session.model = model;
            }
            if let Some(feedback) = feedback.filter(|f| !f.trim().is_empty()) {
                session
                    .history
                    .push(format!("reviewer feedback: {}", feedback.trim()));
            }
        }

        loop {
            // Take the next attempt number (and what we need) under the lock.
            let (attempt, history, provider, model) = {
                let mut state = self.state();
                let session = state
                    .sessions
                    .get_mut(run_id)
                    .ok_or_else(|| CmdError::internal("heal session vanished"))?;
                if session.attempts >= max {
                    drop(state);
                    return Err(self.exhausted(ctx, max));
                }
                session.attempts += 1;
                (
                    session.attempts,
                    session.history.clone(),
                    session.provider.clone(),
                    session.model.clone(),
                )
            };

            let completion = match self
                .complete(TextRequest {
                    provider,
                    model,
                    system: SYSTEM.to_string(),
                    prompt: ctx.prompt(self.config.radius, &history),
                    timeout: None,
                })
                .await
            {
                Ok(c) => c,
                Err(err) => {
                    // The provider failed (not configured, offline…): that attempt doesn't count.
                    if let Some(s) = self.state().sessions.get_mut(run_id) {
                        s.attempts = s.attempts.saturating_sub(1);
                    }
                    return Err(err);
                }
            };

            let host = self.host();
            let checked = match parse_answer(&completion.text) {
                Ok(candidate) => validate(
                    ValidateInput {
                        script_name: &ctx.script,
                        base: &ctx.code,
                        line: ctx.line,
                        radius: self.config.radius,
                        host: host.as_deref(),
                        tmp_dir: &self.config.heal_dir.join("tmp"),
                    },
                    &candidate,
                )
                .await
                .map(|v| (v, candidate)),
                Err(problem) => Err(problem),
            };
            match checked {
                Ok((valid, candidate)) => {
                    let mut state = self.state();
                    state.next += 1;
                    let proposal = Proposal {
                        proposal_id: format!("heal-{}-{}", now_ms(), state.next),
                        id: run_id.to_string(),
                        row: ctx.row,
                        script: ctx.script.clone(),
                        diff: valid.diff,
                        explanation: candidate.explanation,
                        confidence: candidate.confidence,
                        attempt,
                        max_attempts: max,
                        checks: valid.checks,
                        status: ProposalStatus::Pending,
                        provider: completion.provider,
                        model: completion.model,
                        created_ms: now_ms(),
                        patched_code: valid.patched_code,
                        seq: state.next,
                    };
                    for old in state.proposals.values_mut() {
                        if old.id == run_id && old.status == ProposalStatus::Pending {
                            old.status = ProposalStatus::Superseded;
                        }
                    }
                    state
                        .proposals
                        .insert(proposal.proposal_id.clone(), proposal.clone());
                    return Ok(proposal);
                }
                Err(problem) => {
                    tracing::info!(run = run_id, attempt, %problem, "heal proposal rejected by validation");
                    if let Some(s) = self.state().sessions.get_mut(run_id) {
                        s.history
                            .push(format!("attempt {attempt} was invalid: {problem}"));
                    }
                }
            }
        }
    }

    /// Attempts used up: `run.heal_failed` and the error for the caller.
    fn exhausted(&self, ctx: &HealContext, max: u32) -> CmdError {
        let last = self
            .state()
            .sessions
            .get(&ctx.run)
            .and_then(|s| s.history.last().cloned())
            .unwrap_or_default();
        let message = format!("no usable patch after {max} attempts; last problem: {last}");
        self.emit(
            topics::HEAL_FAILED,
            json!({"id": ctx.run, "row": ctx.row, "attempts": max, "error": message}),
        );
        CmdError::invalid_params(message).with_hint(
            "Fix the script yourself and call run_heal_apply, or run_resume to skip the broken rows.",
        )
    }

    /// A proposal by id (any status).
    pub fn proposal(&self, proposal_id: &str) -> Result<Proposal, CmdError> {
        self.state()
            .proposals
            .get(proposal_id)
            .cloned()
            .ok_or_else(|| {
                CmdError::new(
                    ErrorCode::NotFound,
                    format!("no heal proposal '{proposal_id}'"),
                )
                .with_hint("Call run_heal_proposals; after a browser restart propose again.")
            })
    }

    /// Proposals, newest first; only those of run `id` when given.
    pub fn proposals(&self, id: Option<&str>) -> Vec<Proposal> {
        let mut list: Vec<Proposal> = self
            .state()
            .proposals
            .values()
            .filter(|p| id.is_none_or(|id| p.id == id))
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            b.created_ms
                .cmp(&a.created_ms)
                .then(b.proposal_id.cmp(&a.proposal_id))
        });
        list
    }

    /// A pending proposal that still answers its run's open heal request. A stale one (the run
    /// was cancelled, finished, resumed or fixed by hand since) is marked superseded and refused.
    async fn pending(&self, proposal_id: &str) -> Result<Proposal, CmdError> {
        let proposal = self.proposal(proposal_id)?;
        if proposal.status == ProposalStatus::Pending {
            self.reconcile(&proposal.id).await?;
        }
        let status = self.proposal(proposal_id)?.status;
        if status == ProposalStatus::Superseded && proposal.status == ProposalStatus::Pending {
            return Err(CmdError::invalid_params(format!(
                "proposal '{proposal_id}' is outdated: run '{}' is no longer waiting for this fix",
                proposal.id
            ))
            .with_hint(
                "The run was cancelled, finished, resumed or fixed by hand. See run_status; \
                 a new break gets a new proposal.",
            ));
        }
        if status != ProposalStatus::Pending {
            return Err(CmdError::invalid_params(format!(
                "proposal '{proposal_id}' is {status:?}, not pending"
            )));
        }
        Ok(proposal)
    }

    /// The open heal request of a run as `(row, script)`; `None` when the run is not waiting
    /// for a fix (finished, cancelled, resumed, patched, or gone).
    async fn open_request(&self, run_id: &str) -> Result<Option<(u64, String)>, CmdError> {
        let run = match self.call("run.status", json!({ "id": run_id })).await {
            Ok(run) => run,
            Err(err) if err.code == ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if matches!(
            run["status"].as_str(),
            Some("finished" | "failed" | "cancelled")
        ) {
            return Ok(None);
        }
        Ok(run.get("heal").filter(|h| h.is_object()).map(|heal| {
            (
                heal["row"].as_u64().unwrap_or_default(),
                heal["script"].as_str().unwrap_or_default().to_string(),
            )
        }))
    }

    /// Mark the pending proposals of `run_id` superseded unless they answer `open` (the run's
    /// open heal request as `(row, script)`). Only proposals up to sequence number `upto` are
    /// touched when it is given (those that existed when the run was read). Returns how many
    /// proposals changed.
    pub(crate) fn supersede_stale(
        &self,
        run_id: &str,
        open: Option<(u64, &str)>,
        upto: Option<u64>,
    ) -> usize {
        let mut state = self.state();
        let mut changed = 0;
        for p in state.proposals.values_mut() {
            if p.id == run_id
                && p.status == ProposalStatus::Pending
                && upto.is_none_or(|upto| p.seq <= upto)
                && open != Some((p.row, p.script.as_str()))
            {
                p.status = ProposalStatus::Superseded;
                changed += 1;
            }
        }
        changed
    }

    /// The run ended (finished, failed, cancelled): its pending proposals are outdated and its
    /// AI session is over.
    pub(crate) fn run_ended(&self, run_id: &str) {
        self.supersede_stale(run_id, None, None);
        self.state().sessions.remove(run_id);
    }

    /// Whether `run_id` has a pending proposal.
    pub(crate) fn has_pending(&self, run_id: &str) -> bool {
        self.state()
            .proposals
            .values()
            .any(|p| p.id == run_id && p.status == ProposalStatus::Pending)
    }

    /// Check the pending proposals of `run_id` against the run: those that do not answer its
    /// open heal request any more (cancelled, finished, resumed, fixed by hand with
    /// `run.heal_apply`) become superseded. Returns how many changed.
    pub async fn reconcile(&self, run_id: &str) -> Result<usize, CmdError> {
        // Proposals made while the run is read answer the state after it: leave them alone.
        let upto = self.state().next;
        let open = self.open_request(run_id).await?;
        Ok(self.supersede_stale(
            run_id,
            open.as_ref().map(|(row, s)| (*row, s.as_str())),
            Some(upto),
        ))
    }

    /// [`Self::proposals`] after checking every run with pending proposals against its state,
    /// so outdated ones show as superseded. A run that cannot be read keeps its proposals.
    pub async fn current_proposals(&self, id: Option<&str>) -> Vec<Proposal> {
        let mut runs: Vec<String> = self
            .state()
            .proposals
            .values()
            .filter(|p| p.status == ProposalStatus::Pending && id.is_none_or(|id| p.id == id))
            .map(|p| p.id.clone())
            .collect();
        runs.sort();
        runs.dedup();
        for run in runs {
            if let Err(err) = self.reconcile(&run).await {
                tracing::debug!(run, %err, "cannot check heal proposals against the run");
            }
        }
        self.proposals(id)
    }

    fn set_status(&self, proposal_id: &str, status: ProposalStatus) {
        if let Some(p) = self.state().proposals.get_mut(proposal_id) {
            p.status = status;
        }
    }

    /// Apply a pending proposal with `run.heal_apply` (saves `<name>.vN`, re-queues the broken
    /// rows, resumes the run). Emits `run.heal_applied`.
    pub async fn accept(&self, proposal_id: &str, auto: bool) -> Result<Applied, CmdError> {
        let proposal = self.pending(proposal_id).await?;
        let out = self
            .call(
                "run.heal_apply",
                json!({"id": proposal.id, "patched_code": proposal.patched_code}),
            )
            .await?;
        self.set_status(proposal_id, ProposalStatus::Accepted);
        self.state().sessions.remove(&proposal.id);
        let applied = Applied {
            id: proposal.id.clone(),
            proposal_id: proposal_id.to_string(),
            script: out["script"].as_str().unwrap_or_default().to_string(),
            requeued: out["requeued"].as_u64().unwrap_or_default(),
        };
        self.emit(
            topics::HEAL_APPLIED,
            json!({"id": applied.id, "proposal_id": applied.proposal_id,
                   "script": applied.script, "requeued": applied.requeued, "auto": auto}),
        );
        Ok(applied)
    }

    /// Reject a pending proposal and ask for a new one with the feedback (the next attempt of
    /// the same session). The new proposal is announced with `run.heal_proposed`.
    pub async fn reject(
        &self,
        proposal_id: &str,
        feedback: Option<String>,
    ) -> Result<Proposal, CmdError> {
        let proposal = self.pending(proposal_id).await?;
        self.set_status(proposal_id, ProposalStatus::Rejected);
        let reason = feedback
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| "no reason given".into());
        let note = format!(
            "a reviewer rejected attempt {} ({}): {}",
            proposal.attempt,
            proposal.explanation,
            reason.trim()
        );
        if let Some(s) = self.state().sessions.get_mut(&proposal.id) {
            s.history.push(note);
        }
        let next = self.propose(&proposal.id, None, None, None).await?;
        self.announce(&next, None);
        Ok(next)
    }

    /// `run.heal_proposed` for a pending proposal.
    pub fn announce(&self, p: &Proposal, note: Option<&str>) {
        let mode = self
            .state()
            .sessions
            .get(&p.id)
            .map_or_else(|| "ask".to_string(), |s| s.mode.clone());
        let mut data = json!({
            "id": p.id,
            "proposal_id": p.proposal_id,
            "row": p.row,
            "script": p.script,
            "diff": p.diff,
            // The whole fixed script (what run.heal_apply would get), for clients that show
            // or apply it themselves.
            "patched_code": p.patched_code,
            "explanation": p.explanation,
            "confidence": p.confidence,
            "attempt": p.attempt,
            "max_attempts": p.max_attempts,
            "checks": p.checks,
            "mode": mode,
        });
        if let Some(note) = note {
            data["note"] = json!(note);
        }
        self.emit(topics::HEAL_PROPOSED, data);
    }

    /// React to `run.heal_needed`: propose a patch; "ask" announces it, "auto" applies a
    /// confident one that passes a dry run (else announces it with the reason).
    pub async fn on_heal_needed(&self, data: &Value) {
        let Some(run_id) = data["id"].as_str() else {
            return;
        };
        let mode = data["mode"].as_str().unwrap_or("ask");
        if mode == "off" {
            return;
        }
        let proposal = match self.propose(run_id, None, None, None).await {
            Ok(p) => p,
            Err(err) => {
                // Exhaustion already emitted run.heal_failed; other errors (no provider…) not.
                if !err.message.starts_with("no usable patch") {
                    self.emit(
                        topics::HEAL_FAILED,
                        json!({"id": run_id, "row": data["row"], "attempts": 0,
                               "error": err.message}),
                    );
                }
                tracing::warn!(run = run_id, %err, "self-healing: no proposal");
                return;
            }
        };
        if mode != "auto" {
            self.announce(&proposal, None);
            return;
        }
        match self.try_auto(&proposal).await {
            Ok(_) => {}
            Err(reason) => {
                tracing::info!(run = run_id, %reason, "auto-heal fell back to asking");
                self.announce(
                    &proposal,
                    Some(&format!("not applied automatically: {reason}")),
                );
            }
        }
    }

    /// "auto": apply when confident enough and the failing row passes with the patch.
    async fn try_auto(&self, p: &Proposal) -> Result<Applied, String> {
        if p.confidence < self.config.auto_threshold {
            return Err(format!(
                "confidence {:.2} is below {:.2}",
                p.confidence, self.config.auto_threshold
            ));
        }
        let done = self
            .state()
            .auto_applied
            .get(&p.id)
            .copied()
            .unwrap_or_default();
        if done >= self.config.max_auto_per_run {
            return Err(format!(
                "{done} patches were already applied automatically in this run"
            ));
        }
        self.dry_run(p).await?;
        let applied = self
            .accept(&p.proposal_id, true)
            .await
            .map_err(|e| e.message)?;
        *self.state().auto_applied.entry(p.id.clone()).or_default() += 1;
        Ok(applied)
    }

    /// Run the failing row once with the patched code (`script.run`).
    async fn dry_run(&self, p: &Proposal) -> Result<(), String> {
        let run = self
            .call("run.status", json!({"id": p.id}))
            .await
            .map_err(|e| e.message)?;
        let rows = self
            .call(
                "run.results",
                json!({"id": p.id, "status": "needs_heal", "limit": 1000}),
            )
            .await
            .map_err(|e| e.message)?;
        let inputs = rows["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["row"].as_u64() == Some(p.row))
            .map(|r| r["inputs"].clone())
            .ok_or_else(|| format!("row {} is no longer waiting for a patch", p.row))?;
        let outcome = self
            .call(
                "script.run",
                json!({"code": p.patched_code, "inputs": inputs,
                       "timeout_secs": run["options"]["timeout_secs"].as_u64().unwrap_or(300)}),
            )
            .await
            .map_err(|e| e.message)?;
        if outcome["ok"] == true {
            return Ok(());
        }
        let error = &outcome["error"];
        Err(format!(
            "dry run of row {} failed: {} {}",
            p.row,
            error["code"].as_str().unwrap_or("?"),
            error["message"].as_str().unwrap_or("")
        ))
    }
}
