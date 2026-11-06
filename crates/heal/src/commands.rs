//! Registry commands of this crate.
//!
//! | Command | |
//! |---|---|
//! | `run.heal_propose {id, provider?, model?, feedback?}` | AI patch for a paused run (validated) |
//! | `run.heal_accept {proposal_id}` | apply it (`run.heal_apply`), the run continues |
//! | `run.heal_reject {proposal_id, feedback?}` | ask for another patch (max 3 attempts) |
//! | `run.heal_proposals {id?}` | proposals with status, diff and patched code |
//! | `page.extract {tab?, schema, hint?, cache_key?}` | structured data with a cached AI plan |

use bmcp_core::{CmdError, Command, Ctx, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::HealService;
use crate::extract::{ExtractArgs, ExtractOutput};
use crate::heal::{Applied, Proposal};

/// Register the commands. The app also inserts a [`HealService`] and calls [`crate::attach`].
pub fn register(registry: &mut Registry) {
    registry
        .add::<HealPropose>()
        .add::<HealAccept>()
        .add::<HealReject>()
        .add::<HealProposals>()
        .add::<Extract>();
}

pub struct HealPropose;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeArgs {
    /// Run id of a run paused by run.heal_needed.
    pub id: String,
    /// AI provider from ai_providers (default: the configured default provider).
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Extra instructions for the AI, e.g. "the button is now called Log in".
    pub feedback: Option<String>,
}

impl Command for HealPropose {
    const NAME: &'static str = "run.heal_propose";
    type Args = ProposeArgs;
    type Output = Proposal;

    fn meta() -> Meta {
        Meta::new(
            "Ask the AI for a minimal patch for a run paused by run.heal_needed (it sees the \
             script, the failing line, the error and the page snapshot). The patch is validated \
             (applies, stays near the failing step, transpiles). Returns {proposal_id, diff, \
             explanation, confidence}; answer with run_heal_accept or run_heal_reject.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ProposeArgs) -> Result<Proposal, CmdError> {
        let heal = HealService::from_ctx(&ctx)?;
        let proposal = heal
            .propose(&args.id, args.provider, args.model, args.feedback)
            .await?;
        heal.announce(&proposal, None);
        Ok(proposal)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalIdArgs {
    pub proposal_id: String,
}

pub struct HealAccept;

impl Command for HealAccept {
    const NAME: &'static str = "run.heal_accept";
    type Args = ProposalIdArgs;
    type Output = Applied;

    fn meta() -> Meta {
        Meta::new(
            "Accept a heal proposal: the patched script is saved as the next version \
             (<name>.v2, …), the broken rows are re-queued and the run continues with it.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ProposalIdArgs) -> Result<Applied, CmdError> {
        HealService::from_ctx(&ctx)?
            .accept(&args.proposal_id, false)
            .await
    }
}

pub struct HealReject;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectArgs {
    pub proposal_id: String,
    /// What is wrong with it; the AI gets this for the next attempt.
    pub feedback: Option<String>,
}

impl Command for HealReject {
    const NAME: &'static str = "run.heal_reject";
    type Args = RejectArgs;
    type Output = Proposal;

    fn meta() -> Meta {
        Meta::new(
            "Reject a heal proposal and get a new one written with your feedback (at most 3 \
             attempts per broken row; then run.heal_failed and the run stays paused).",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RejectArgs) -> Result<Proposal, CmdError> {
        HealService::from_ctx(&ctx)?
            .reject(&args.proposal_id, args.feedback)
            .await
    }
}

pub struct HealProposals;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalsArgs {
    /// Only proposals of this run.
    pub id: Option<String>,
}

/// A proposal with its full patched script.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposalWithCode {
    #[serde(flatten)]
    pub proposal: Proposal,
    pub patched_code: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposalsOutput {
    /// Newest first.
    pub proposals: Vec<ProposalWithCode>,
}

impl Command for HealProposals {
    const NAME: &'static str = "run.heal_proposals";
    type Args = ProposalsArgs;
    type Output = ProposalsOutput;

    fn meta() -> Meta {
        Meta::new(
            "List heal proposals of this browser session (pending, accepted, rejected, \
             superseded), newest first, with diff and the whole patched script.",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, args: ProposalsArgs) -> Result<ProposalsOutput, CmdError> {
        let proposals = HealService::from_ctx(&ctx)?
            .proposals(args.id.as_deref())
            .into_iter()
            .map(|p| ProposalWithCode {
                patched_code: p.patched_code.clone(),
                proposal: p,
            })
            .collect();
        Ok(ProposalsOutput { proposals })
    }
}

pub struct Extract;

impl Command for Extract {
    const NAME: &'static str = "page.extract";
    type Args = ExtractArgs;
    type Output = ExtractOutput;

    fn meta() -> Meta {
        Meta::new(
            "Extract structured data matching a JSON Schema from the page. The first time an AI \
             builds a selector plan (cached in data/extract-cache/<cache_key>.json); later calls \
             run the plan without AI and rebuild it once if required fields come back empty. \
             Returns {data, from_cache, plan_id}.",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, args: ExtractArgs) -> Result<ExtractOutput, CmdError> {
        HealService::from_ctx(&ctx)?.extract(&ctx, args).await
    }
}
