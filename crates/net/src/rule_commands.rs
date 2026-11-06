//! Interception commands: `net.rule_add`, `net.rule_list`, `net.rule_remove`,
//! `net.breakpoint`, `net.paused_list`, `net.resume`, and `net.replay`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, Retry, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::intercept::{self, Edits};
use crate::rules::{Action, MatchSpec, Matcher, Rule, RuleKind, Stage, validate};
use crate::state::{Env, NetState, Paused, TabNet};
use crate::store::{cut, header};

/// Add a rule or breakpoint to a tab and update `Fetch.enable`.
async fn add_rule(
    env: &Env,
    matcher: Matcher,
    kind: RuleKind,
    prefix: &str,
) -> Result<Rule, CmdError> {
    let _config = env.tn.config.lock().await;
    let rule = Rule {
        id: env.net.new_id(prefix),
        tab: env.tab,
        matcher,
        kind,
        hits: 0,
    };
    env.tn.lock().rules.push(rule.clone());
    if let Err(err) = env.sync_fetch().await {
        // Do not keep a rule that is not active.
        let mut inner = env.tn.lock();
        inner.rules.retain(|r| r.id != rule.id);
        inner.stop_listener_if_idle();
        return Err(err);
    }
    Ok(rule)
}

/// The tab that has the rule / pause with this id.
fn find_tab(net: &NetState, has: impl Fn(&TabNet) -> bool) -> Option<Arc<TabNet>> {
    net.all().into_iter().find(|tn| has(tn))
}

// ------------------------------------------------------------------ net.rule_add

pub struct RuleAdd;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleAddArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Which requests: {url_glob | url_regex, method?, stage?: request|response}.
    #[serde(rename = "match")]
    pub matcher: MatchSpec,
    /// "block", {"redirect":{"url"}}, {"set_headers":{name:value}}, {"remove_headers":[names]},
    /// {"rewrite_body":{"find","replace"}} or {"mock":{"status","headers","body"}}.
    pub action: Action,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RuleOutput {
    /// Rule id (`r3`) or breakpoint id (`b4`), for net_rule_remove.
    pub id: String,
    pub text: String,
}

impl Command for RuleAdd {
    const NAME: &'static str = "net.rule_add";
    type Args = RuleAddArgs;
    type Output = RuleOutput;

    fn meta() -> Meta {
        Meta::new(
            "Add a traffic rule for a tab: block, redirect, set/remove headers, rewrite the \
             body or mock the response of matching requests (request or response stage). \
             Returns the rule id; remove with net_rule_remove.",
            Toolset::Net,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &RuleAddArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: RuleAddArgs) -> Result<RuleOutput, CmdError> {
        let matcher = Matcher::new(&args.matcher, None)?;
        validate(&args.action, matcher.stage)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let rule = add_rule(&env, matcher, RuleKind::Action(args.action), "r").await?;
        Ok(RuleOutput {
            id: rule.id.clone(),
            text: rule.line(),
        })
    }
}

// ------------------------------------------------------------------ net.breakpoint

pub struct Breakpoint;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BreakpointArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Which requests: {url_glob | url_regex, method?}. Default: all.
    #[serde(rename = "match")]
    pub matcher: Option<MatchSpec>,
    /// request (default: before sending, edit the request) or response (edit the answer).
    pub stage: Option<Stage>,
}

impl Command for Breakpoint {
    const NAME: &'static str = "net.breakpoint";
    type Args = BreakpointArgs;
    type Output = RuleOutput;

    fn meta() -> Meta {
        Meta::new(
            "Pause matching requests (like Burp intercept) at the request or response stage. \
             Each pause emits a net.paused event and shows in net_paused_list; answer with \
             net_resume (edit/continue/drop) within 60 s or it continues unchanged.",
            Toolset::Net,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &BreakpointArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: BreakpointArgs) -> Result<RuleOutput, CmdError> {
        let matcher = Matcher::new(&args.matcher.unwrap_or_default(), args.stage)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let rule = add_rule(&env, matcher, RuleKind::Breakpoint, "b").await?;
        Ok(RuleOutput {
            id: rule.id.clone(),
            text: format!(
                "{}\npaused requests: net.paused events and net_paused_list; answer with net_resume",
                rule.line()
            ),
        })
    }
}

// ------------------------------------------------------------------ net.rule_list

pub struct RuleList;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabFilterArgs {
    /// Only this tab. Default: all tabs.
    pub tab: Option<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LinesOutput {
    pub count: usize,
    /// One line per item.
    pub text: String,
}

impl Command for RuleList {
    const NAME: &'static str = "net.rule_list";
    type Args = TabFilterArgs;
    type Output = LinesOutput;

    fn meta() -> Meta {
        Meta::new(
            "Active traffic rules and breakpoints, one line each: id, tab, stage, match, \
             action, hits.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: TabFilterArgs) -> Result<LinesOutput, CmdError> {
        let lines: Vec<String> = NetState::from_ctx(&ctx)
            .all()
            .iter()
            .filter(|tn| args.tab.is_none_or(|t| t == tn.tab))
            .flat_map(|tn| tn.lock().rules.iter().map(Rule::line).collect::<Vec<_>>())
            .collect();
        Ok(LinesOutput {
            count: lines.len(),
            text: if lines.is_empty() {
                "no rules or breakpoints".into()
            } else {
                lines.join("\n")
            },
        })
    }
}

// ------------------------------------------------------------------ net.rule_remove

pub struct RuleRemove;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleRemoveArgs {
    /// Rule or breakpoint id from net_rule_add / net_breakpoint / net_rule_list.
    pub id: String,
}

impl Command for RuleRemove {
    const NAME: &'static str = "net.rule_remove";
    type Args = RuleRemoveArgs;
    type Output = RuleOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove a traffic rule or breakpoint by id. Requests still paused by a removed \
             breakpoint are continued unchanged.",
            Toolset::Net,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: RuleRemoveArgs) -> Result<RuleOutput, CmdError> {
        let net = NetState::from_ctx(&ctx);
        let tn = find_tab(&net, |tn| tn.lock().rules.iter().any(|r| r.id == args.id)).ok_or_else(
            || {
                CmdError::new(ErrorCode::NotFound, format!("no rule {}", args.id))
                    .with_retry(Retry::No)
                    .with_hint("See net_rule_list for the active ids.")
            },
        )?;
        let env = Env::for_tab(&ctx, tn.clone());
        let _config = tn.config.lock().await;
        let (removed, released) = {
            let mut inner = tn.lock();
            let removed = inner.rules.iter().find(|r| r.id == args.id).cloned();
            inner.rules.retain(|r| r.id != args.id);
            let held: Vec<String> = inner
                .paused
                .values()
                .filter(|p| p.rule_id == args.id)
                .map(|p| p.pause_id.clone())
                .collect();
            let released: Vec<Paused> = held
                .iter()
                .filter_map(|id| inner.paused.remove(id))
                .collect();
            (removed, released)
        };
        // Answer what the breakpoint still holds (with what its other matching rules would
        // do) before Fetch may be disabled.
        for p in &released {
            let outcome = intercept::plan(&env, &p.params, p.stage, &p.actions).await;
            let _ = intercept::finish(&env, &p.request_id, &outcome).await;
        }
        env.sync_fetch().await?;
        let line = removed.map(|r| r.line()).unwrap_or_default();
        Ok(RuleOutput {
            id: args.id,
            text: format!(
                "removed {line}{}",
                if released.is_empty() {
                    String::new()
                } else {
                    format!("; continued {} paused requests", released.len())
                }
            ),
        })
    }
}

// ------------------------------------------------------------------ net.paused_list

pub struct PausedList;

impl Command for PausedList {
    const NAME: &'static str = "net.paused_list";
    type Args = TabFilterArgs;
    type Output = LinesOutput;

    fn meta() -> Meta {
        Meta::new(
            "Requests held by breakpoints, one line each: pause id, breakpoint, stage, method, \
             url, seconds left before they continue on their own.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: TabFilterArgs) -> Result<LinesOutput, CmdError> {
        let lines: Vec<String> = NetState::from_ctx(&ctx)
            .all()
            .iter()
            .filter(|tn| args.tab.is_none_or(|t| t == tn.tab))
            .flat_map(|tn| {
                tn.lock()
                    .paused
                    .values()
                    .map(Paused::line)
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok(LinesOutput {
            count: lines.len(),
            text: if lines.is_empty() {
                "nothing is paused".into()
            } else {
                lines.join("\n")
            },
        })
    }
}

// ------------------------------------------------------------------ net.resume

pub struct Resume;

/// What `net.resume` does with the request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeAction {
    /// Send it on (with the edits, if any).
    #[default]
    Continue,
    /// Fail it (the page sees net::ERR_ABORTED).
    Drop,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeArgs {
    /// Pause id from the net.paused event or net_paused_list.
    pub pause_id: String,
    /// Changes before continuing: url, method, headers, body (request stage); status,
    /// headers, response_body (response stage; at the request stage they answer without
    /// sending).
    pub edits: Option<Edits>,
    /// continue (default) or drop.
    pub action: Option<ResumeAction>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResumeOutput {
    pub pause_id: String,
    /// continued, fulfilled or dropped.
    pub done: String,
    pub text: String,
}

impl Command for Resume {
    const NAME: &'static str = "net.resume";
    type Args = ResumeArgs;
    type Output = ResumeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Release a request held by a breakpoint: continue (optionally with edited url, \
             method, headers, body, status, response_body) or drop it.",
            Toolset::Net,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ResumeArgs) -> Result<ResumeOutput, CmdError> {
        let net = NetState::from_ctx(&ctx);
        let tn = find_tab(&net, |tn| tn.lock().paused.contains_key(&args.pause_id)).ok_or_else(
            || {
                CmdError::new(
                    ErrorCode::NotFound,
                    format!("no paused request {}", args.pause_id),
                )
                .with_retry(Retry::No)
                .with_hint(
                    "It was already resumed, continued automatically after 60 s, or its tab \
                     closed. See net_paused_list.",
                )
            },
        )?;
        let edits = args.edits.unwrap_or_default();
        let drop = args.action == Some(ResumeAction::Drop);
        let current = tn.lock().paused.get(&args.pause_id).cloned();
        let stage = current.as_ref().map(|p| p.stage).unwrap_or_default();
        if stage == Stage::Response
            && (edits.url.is_some() || edits.method.is_some() || edits.body.is_some())
        {
            return Err(CmdError::invalid_params(
                "this request is paused at the response stage: url, method and body were \
                 already sent; edit status, headers or response_body instead",
            ));
        }
        // Checked before taking the pause, so it stays paused for a corrected resume.
        if !drop && let Some(problem) = current.and_then(|p| intercept::body_problem(&p, &edits)) {
            return Err(CmdError::invalid_params(format!(
                "{problem}: give response_body together with status"
            )));
        }
        // Take it out of the map first: whoever removes it answers it (not the timer).
        let paused = tn
            .lock()
            .paused
            .remove(&args.pause_id)
            .ok_or_else(|| CmdError::new(ErrorCode::NotFound, "already resumed"))?;
        let env = Env::for_tab(&ctx, tn.clone());
        let done = intercept::resume(&env, &paused, &edits, drop).await;
        tn.lock().stop_listener_if_idle();
        let done = done?;
        Ok(ResumeOutput {
            text: format!(
                "{done} {} {} {}",
                paused.pause_id,
                paused.method(),
                paused.url()
            ),
            pause_id: paused.pause_id,
            done: done.to_string(),
        })
    }
}

// ------------------------------------------------------------------ net.replay

pub struct Replay;

/// Changes for `net.replay`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayEdits {
    pub url: Option<String>,
    pub method: Option<String>,
    /// Headers to set; a null value removes one.
    pub headers: Option<BTreeMap<String, Option<String>>>,
    /// New request body.
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Request id from net_list.
    pub id: u64,
    pub edits: Option<ReplayEdits>,
    /// Cut the response body to this many bytes (default 4096).
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReplayOutput {
    pub tab: TabId,
    pub status: u16,
    pub status_text: String,
    pub content_type: String,
    /// Length of the whole response text.
    pub length: usize,
    /// Response text, cut to max_bytes.
    pub body: String,
    pub truncated: bool,
}

/// Headers `fetch()` refuses or sets itself; sending them would fail or be ignored.
const FORBIDDEN_HEADERS: &[&str] = &[
    "accept-charset",
    "accept-encoding",
    "access-control-request-headers",
    "access-control-request-method",
    "connection",
    "content-length",
    "cookie",
    "cookie2",
    "date",
    "dnt",
    "expect",
    "host",
    "keep-alive",
    "origin",
    "referer",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
    "via",
];

impl Command for Replay {
    const NAME: &'static str = "net.replay";
    type Args = ReplayArgs;
    type Output = ReplayOutput;

    fn meta() -> Meta {
        Meta::new(
            "Send a captured request again from the page (fetch with the page's cookies), \
             optionally with edited url, method, headers or body. Returns status and the \
             response body cut to max_bytes. Subject to the page's CORS rules.",
            Toolset::Net,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &ReplayArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ReplayArgs) -> Result<ReplayOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let max = args.max_bytes.unwrap_or(4096).max(16);
        let entry = env.tn.lock().store.get(args.id).cloned().ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("no request #{} on tab {tab}", args.id),
            )
            .with_retry(Retry::No)
            .with_hint("Ids come from net_list.")
        })?;
        let edits = args.edits.unwrap_or_default();
        let url = edits.url.clone().unwrap_or(entry.url.clone());
        let method = edits
            .method
            .clone()
            .unwrap_or(entry.method.clone())
            .to_ascii_uppercase();

        let mut headers: Vec<(String, String)> = entry
            .req_headers
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                // HTTP/2 pseudo headers (`:path`) and browser-controlled ones.
                !lower.starts_with(':')
                    && !lower.starts_with("sec-")
                    && !lower.starts_with("proxy-")
                    && !FORBIDDEN_HEADERS.contains(&lower.as_str())
            })
            .cloned()
            .collect();
        for (name, value) in edits.headers.iter().flatten() {
            headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
            if let Some(value) = value {
                headers.push((name.clone(), value.clone()));
            }
        }
        let body = edits.body.clone().or(entry.req_body.clone());
        // GET and HEAD may not have a body in fetch().
        let body = body.filter(|_| method != "GET" && method != "HEAD");

        let expression = format!(
            "(async () => {{\
               const r = await fetch({url}, {{ method: {method}, headers: {headers}, body: {body}, \
                                              credentials: 'include', redirect: 'follow' }});\
               const t = await r.text();\
               return {{ status: r.status, statusText: r.statusText, \
                         type: r.headers.get('content-type') || '', length: t.length, \
                         body: t.slice(0, {max}) }};\
             }})()",
            url = json!(url),
            method = json!(method),
            headers = Value::Array(headers.iter().map(|(n, v)| json!([n, v])).collect()),
            body = body.map(Value::String).unwrap_or(Value::Null),
        );
        let reply = env
            .cdp(
                "Runtime.evaluate",
                json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("fetch failed");
            return Err(CmdError::new(
                ErrorCode::Internal,
                format!("replay failed in the page: {text}"),
            )
            .with_retry(Retry::No)
            .with_hint(
                "A TypeError usually means CORS or a network error: replay from a tab on the \
                 same site, or check the url.",
            ));
        }
        let v = &reply["result"]["value"];
        let text = v["body"].as_str().unwrap_or_default();
        let shown = cut(text, max);
        let length = v["length"].as_u64().unwrap_or(0) as usize;
        Ok(ReplayOutput {
            tab,
            status: v["status"].as_u64().unwrap_or(0) as u16,
            status_text: v["statusText"].as_str().unwrap_or_default().to_string(),
            content_type: v["type"]
                .as_str()
                .or_else(|| header(&entry.res_headers, "content-type"))
                .unwrap_or_default()
                .to_string(),
            // The page already sliced to `max` (UTF-16 units); `cut` then limits bytes.
            truncated: length > max || shown.len() < text.len(),
            length,
            body: shown.to_string(),
        })
    }
}
