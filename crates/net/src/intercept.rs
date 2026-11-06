//! Handling of `Fetch.requestPaused`: apply rules, hold requests for breakpoints, resume them.
//!
//! The one hard rule: **every paused request gets exactly one answer** (`Fetch.continueRequest`,
//! `continueResponse`, `fulfillRequest` or `failRequest`), otherwise the page hangs. Net is the
//! "net" client of the tab's Fetch coordinator (`bmcp-cdpshare`), which reads every pause once,
//! asks adblock first and net second, and sends every answer itself:
//! * rules are decided right away ([`on_paused`] returns the answer, or `Pass` when no rule
//!   matches, and the coordinator continues the request);
//! * a breakpoint parks the request in `TabInner::paused` (`Verdict::Hold`) and starts a 60 s
//!   timer ([`auto_continue`]); whoever removes it from the map first (`net.resume`, the timer,
//!   `net.rule_remove`) answers it through the coordinator ([`finish`]);
//! * when an answer fails (e.g. a bad edited header), the coordinator falls back to a plain
//!   continue and [`NetFetch::done`] emits `net.intercept_error`.
//!
//! Rule order: rules and breakpoints are checked in the order they were added. A block/mock
//! before the first matching breakpoint answers the request and it never pauses. Otherwise
//! the first matching breakpoint pauses it, and the matching action rules (up to the first
//! block/mock) are kept with the pause: releasing it without edits applies them.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bmcp_cdpshare::{Answer, AnswerError, Answered, FetchHandler, PausedRequest, Verdict};
use bmcp_core::{BoxFuture, CmdError, ErrorCode, Retry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::rules::{Action, RuleKind, Stage};
use crate::state::{Env, Paused};
use crate::store::{Headers, header, headers_from};

/// A paused request that nobody resumes is continued after this long.
pub const PAUSE_TIMEOUT: Duration = Duration::from_secs(60);

/// Net's client name at the tab's Fetch coordinator (also the `by` of its answers).
pub const CLIENT: &str = "net";

/// The answer to one paused request.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Let it through unchanged.
    Continue,
    /// Request stage: send it with these changes (`None` = unchanged).
    ContinueRequest {
        url: Option<String>,
        method: Option<String>,
        headers: Option<Headers>,
        post_data: Option<String>,
    },
    /// Response stage: keep the body, change status/headers.
    ContinueResponse {
        status: u16,
        phrase: String,
        headers: Headers,
    },
    /// Answer with this response (mock, rewritten body, edited response).
    Fulfill {
        status: u16,
        phrase: String,
        headers: Headers,
        body: Vec<u8>,
    },
    /// Fail it with a CDP `Network.ErrorReason` such as `BlockedByClient`.
    Fail(&'static str),
}

impl Outcome {
    /// The CDP answer (the coordinator adds the `requestId`).
    pub fn to_answer(&self) -> Answer {
        let (method, params) = self.to_cdp();
        Answer::new(method, params)
    }

    /// The CDP method and parameters that send this answer, without `requestId`.
    fn to_cdp(&self) -> (&'static str, Value) {
        match self {
            Outcome::Continue => ("Fetch.continueRequest", json!({})),
            Outcome::ContinueRequest {
                url,
                method,
                headers,
                post_data,
            } => {
                let mut p = json!({});
                if let Some(url) = url {
                    p["url"] = json!(url);
                }
                if let Some(method) = method {
                    p["method"] = json!(method);
                }
                if let Some(headers) = headers {
                    p["headers"] = headers_to_cdp(headers);
                }
                if let Some(post) = post_data {
                    // CDP wants the request body base64-encoded.
                    p["postData"] = json!(B64.encode(post.as_bytes()));
                }
                ("Fetch.continueRequest", p)
            }
            Outcome::ContinueResponse {
                status,
                phrase,
                headers,
            } => {
                let mut p = json!({
                    "responseCode": status,
                    "responseHeaders": headers_to_cdp(headers),
                });
                if !phrase.is_empty() {
                    p["responsePhrase"] = json!(phrase);
                }
                ("Fetch.continueResponse", p)
            }
            Outcome::Fulfill {
                status,
                phrase,
                headers,
                body,
            } => {
                let mut p = json!({
                    "responseCode": status,
                    "responseHeaders": headers_to_cdp(headers),
                    "body": B64.encode(body),
                });
                if !phrase.is_empty() {
                    p["responsePhrase"] = json!(phrase);
                }
                ("Fetch.fulfillRequest", p)
            }
            Outcome::Fail(reason) => ("Fetch.failRequest", json!({ "errorReason": reason })),
        }
    }
}

/// Answer a held request through the tab's Fetch coordinator. If the answer fails, the
/// coordinator still releases the request with a plain continue ([`NetFetch::done`] reports
/// it as `net.intercept_error`).
pub async fn finish(env: &Env, request_id: &str, outcome: &Outcome) -> Result<(), CmdError> {
    env.share
        .answer(env.tab, request_id, outcome.to_answer(), CLIENT)
        .await
        .map_err(|err| match err {
            AnswerError::NotPaused => CmdError::new(
                ErrorCode::NotFound,
                format!("request {request_id} is not paused any more"),
            )
            .with_retry(Retry::No)
            .with_hint("Its tab closed or the browser released it; nothing to resume."),
            AnswerError::Cdp(err) => CmdError::from(err),
        })
}

/// Net as a client of the tab's Fetch coordinator (registered by `Env::sync_fetch`).
pub struct NetFetch {
    pub env: Env,
}

impl FetchHandler for NetFetch {
    fn on_paused<'a>(&'a self, req: &'a PausedRequest) -> BoxFuture<'a, Verdict> {
        Box::pin(on_paused(&self.env, req))
    }

    fn done(&self, req: &PausedRequest, answered: &Answered<'_>) {
        let env = &self.env;
        if answered.by == CLIENT {
            if let Some(error) = answered.error {
                let warning = if answered.released {
                    "the change could not be applied; the request was continued unchanged"
                } else {
                    "the request could not be answered (it may be gone already)"
                };
                env.events.emit(
                    "net.intercept_error",
                    json!({ "tab": env.tab, "request_id": req.request_id, "call": answered.method,
                            "error": error, "warning": warning }),
                );
            }
        } else if answered.held {
            // Released without us (tab closed, interception reset): forget the stale pause.
            env.tn
                .lock()
                .paused
                .retain(|_, p| p.request_id != req.request_id);
        }
    }
}

/// Request stage or response stage? Response-stage pauses carry the response status.
pub fn stage_of(params: &Value) -> Stage {
    if params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some() {
        Stage::Response
    } else {
        Stage::Request
    }
}

/// What to do with one paused request, decided under the lock.
enum Decision {
    Pause(Paused),
    Apply(Vec<Action>),
}

/// Decide about one paused request (the coordinator calls this after adblock let it pass).
pub async fn on_paused(env: &Env, req: &PausedRequest) -> Verdict {
    let params = &req.params;
    let stage = stage_of(params);
    let (url, method) = (req.url(), req.method());
    let network_id = params["networkId"].as_str().unwrap_or_default().to_string();

    let decision = {
        let mut guard = env.tn.lock();
        // Borrow the fields separately (rules, paused, store) through one `&mut`.
        let inner = &mut *guard;
        // Walk the rules in the order they were added: collect the actions up to the first
        // block/mock (it answers the request, later rules never see it) and remember the first
        // matching breakpoint met on the way.
        let mut breakpoint = None;
        let mut actions = Vec::new();
        let mut action_rules = Vec::new();
        for (i, rule) in inner.rules.iter().enumerate() {
            if !rule.matcher.matches(url, method, stage) {
                continue;
            }
            match &rule.kind {
                RuleKind::Breakpoint => {
                    breakpoint.get_or_insert(i);
                }
                RuleKind::Action(action) => {
                    action_rules.push(i);
                    actions.push(action.clone());
                    if matches!(action, Action::Block | Action::Mock { .. }) {
                        break;
                    }
                }
            }
        }
        for &i in &action_rules {
            inner.rules[i].hits += 1;
        }
        match breakpoint {
            // A breakpoint came before any block/mock: hold the request.
            Some(b) => {
                let bp = &mut inner.rules[b];
                bp.hits += 1;
                let pause = Paused {
                    pause_id: env.net.new_id("p"),
                    tab: env.tab,
                    rule_id: bp.id.clone(),
                    actions,
                    request_id: req.request_id.clone(),
                    stage,
                    params: params.clone(),
                    deadline: tokio::time::Instant::now() + PAUSE_TIMEOUT,
                };
                inner.store.note(&network_id, format!("{} paused", bp.id));
                inner.paused.insert(pause.pause_id.clone(), pause.clone());
                Decision::Pause(pause)
            }
            None => {
                for &i in &action_rules {
                    let rule = &inner.rules[i];
                    if let RuleKind::Action(action) = &rule.kind {
                        let note = format!("{} {}", rule.id, action.short());
                        inner.store.note(&network_id, note);
                    }
                }
                Decision::Apply(actions)
            }
        }
    };

    match decision {
        Decision::Pause(pause) => {
            env.events.emit("net.paused", pause.info());
            tokio::spawn(auto_continue(env.clone(), pause.pause_id.clone()));
            Verdict::Hold
        }
        // No rule of ours matches: the coordinator continues it.
        Decision::Apply(actions) if actions.is_empty() => Verdict::Pass,
        Decision::Apply(actions) => match plan(env, params, stage, &actions).await {
            // The rules changed nothing (e.g. a rewrite of a binary body).
            Outcome::Continue => Verdict::Pass,
            outcome => Verdict::Answer(outcome.to_answer()),
        },
    }
}

/// Turn the matching rules' actions into one answer. Block and mock end the list; header
/// and body changes add up.
pub async fn plan(env: &Env, params: &Value, stage: Stage, actions: &[Action]) -> Outcome {
    let request = &params["request"];
    let mut url = None;
    let mut req_headers = headers_from(&request["headers"]);
    let mut res_headers = headers_from_cdp(&params["responseHeaders"]);
    let mut headers_changed = false;
    let mut rewrites = Vec::new();

    for action in actions {
        match action {
            Action::Block => return Outcome::Fail("BlockedByClient"),
            Action::Mock {
                status,
                headers,
                body,
            } => {
                return mock_response(
                    request,
                    status.unwrap_or(200),
                    headers.clone().unwrap_or_default(),
                    body.clone().unwrap_or_default(),
                );
            }
            Action::Redirect { url: to } => url = Some(to.clone()),
            Action::SetHeaders(set) => {
                let target = match stage {
                    Stage::Request => &mut req_headers,
                    Stage::Response => &mut res_headers,
                };
                for (name, value) in set {
                    set_header(target, name, value);
                }
                headers_changed = true;
            }
            Action::RemoveHeaders(names) => {
                let target = match stage {
                    Stage::Request => &mut req_headers,
                    Stage::Response => &mut res_headers,
                };
                for name in names {
                    target.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                }
                headers_changed = true;
            }
            Action::RewriteBody { find, replace } => rewrites.push((find, replace)),
        }
    }

    match stage {
        Stage::Request => {
            let post_data = match request["postData"].as_str() {
                Some(body) if !rewrites.is_empty() => Some(apply_rewrites(body, &rewrites)),
                _ => None,
            };
            if url.is_none() && !headers_changed && post_data.is_none() {
                return Outcome::Continue;
            }
            Outcome::ContinueRequest {
                url,
                method: None,
                headers: headers_changed.then_some(req_headers),
                post_data,
            }
        }
        Stage::Response => {
            let status = params["responseStatusCode"].as_u64().unwrap_or(200) as u16;
            let phrase = params["responseStatusText"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let failed = params.get("responseErrorReason").is_some();
            if !rewrites.is_empty() && !failed {
                // Only text bodies are rewritten; binary ones go through unchanged.
                if let Ok(Some(text)) = response_text(env, params).await {
                    strip_length_headers(&mut res_headers);
                    return Outcome::Fulfill {
                        status,
                        phrase,
                        headers: res_headers,
                        body: apply_rewrites(&text, &rewrites).into_bytes(),
                    };
                }
            }
            if headers_changed && !failed {
                Outcome::ContinueResponse {
                    status,
                    phrase,
                    headers: res_headers,
                }
            } else {
                Outcome::Continue
            }
        }
    }
}

/// The paused response's body as text (`None` = binary).
async fn response_text(env: &Env, params: &Value) -> Result<Option<String>, CmdError> {
    let reply = env
        .cdp(
            "Fetch.getResponseBody",
            json!({ "requestId": params["requestId"] }),
        )
        .await?;
    let body = reply["body"].as_str().unwrap_or_default();
    if reply["base64Encoded"].as_bool() == Some(true) {
        let bytes = B64
            .decode(body)
            .map_err(|e| CmdError::internal(format!("bad base64 body from CDP: {e}")))?;
        Ok(String::from_utf8(bytes).ok())
    } else {
        Ok(Some(body.to_string()))
    }
}

fn apply_rewrites(text: &str, rewrites: &[(&String, &String)]) -> String {
    let mut text = text.to_string();
    for (find, replace) in rewrites {
        text = text.replace(find.as_str(), replace);
    }
    text
}

/// A mocked response. Guesses content-type and, for cross-origin requests, adds the CORS
/// headers the page needs to read the answer.
fn mock_response(
    request: &Value,
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
) -> Outcome {
    let mut list: Headers = headers.into_iter().collect();
    if header(&list, "content-type").is_none() {
        let trimmed = body.trim_start();
        let kind = if trimmed.starts_with('{') || trimmed.starts_with('[') {
            "application/json"
        } else {
            "text/plain; charset=utf-8"
        };
        list.push(("content-type".into(), kind.into()));
    }
    let req_headers = headers_from(&request["headers"]);
    if let Some(origin) = header(&req_headers, "origin")
        && header(&list, "access-control-allow-origin").is_none()
    {
        list.push(("access-control-allow-origin".into(), origin.to_string()));
        list.push(("access-control-allow-credentials".into(), "true".into()));
    }
    Outcome::Fulfill {
        status,
        phrase: String::new(),
        headers: list,
        body: body.into_bytes(),
    }
}

/// Changes for `net.resume`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Edits {
    /// Request stage: send to this URL instead.
    pub url: Option<String>,
    /// Request stage: use this HTTP method.
    pub method: Option<String>,
    /// Headers to set; a null value removes the header. Request headers at the request
    /// stage, response headers at the response stage.
    pub headers: Option<BTreeMap<String, Option<String>>>,
    /// Request stage: new request body.
    pub body: Option<String>,
    /// Answer with this status (response stage: replaces the real one; request stage: the
    /// request is not sent and this is returned instead, default 200).
    pub status: Option<u16>,
    /// Answer with this body instead of the real one.
    pub response_body: Option<String>,
}

impl Edits {
    fn apply_headers(&self, headers: &mut Headers) -> bool {
        let Some(edits) = &self.headers else {
            return false;
        };
        for (name, value) in edits {
            match value {
                Some(value) => set_header(headers, name, value),
                None => headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name)),
            }
        }
        !edits.is_empty()
    }
}

/// Answer a paused request for `net.resume`. Returns what was done. The request is always
/// answered, also when this fails (then with a plain continue).
pub async fn resume(
    env: &Env,
    paused: &Paused,
    edits: &Edits,
    drop: bool,
) -> Result<&'static str, CmdError> {
    let outcome = match resume_outcome(env, paused, edits, drop).await {
        Ok(outcome) => outcome,
        Err(err) => {
            // E.g. the original body could not be read: never leave the request hanging.
            let _ = finish(env, &paused.request_id, &Outcome::Continue).await;
            return Err(err);
        }
    };
    finish(env, &paused.request_id, &outcome).await?;
    Ok(match outcome {
        Outcome::Fail(_) => "dropped",
        Outcome::Fulfill { .. } => "fulfilled",
        _ => "continued",
    })
}

/// Why these edits cannot work on this response-stage pause, if they cannot: a new `status`
/// without `response_body` keeps the original body, and failed responses and redirects have
/// none.
pub fn body_problem(paused: &Paused, edits: &Edits) -> Option<String> {
    if paused.stage != Stage::Response || edits.status.is_none() || edits.response_body.is_some() {
        return None;
    }
    let params = &paused.params;
    if let Some(reason) = params["responseErrorReason"].as_str() {
        return Some(format!(
            "the response failed ({reason}), so it has no body to keep"
        ));
    }
    match params["responseStatusCode"].as_u64() {
        Some(code @ 300..=399) => Some(format!("a {code} redirect has no body to keep")),
        _ => None,
    }
}

/// What `net.resume` sends. Without edits: what the pending rules would have done.
async fn resume_outcome(
    env: &Env,
    paused: &Paused,
    edits: &Edits,
    drop: bool,
) -> Result<Outcome, CmdError> {
    let params = &paused.params;
    let request = &params["request"];
    Ok(if drop {
        Outcome::Fail("Aborted")
    } else {
        match paused.stage {
            Stage::Request if edits.status.is_some() || edits.response_body.is_some() => {
                let mut headers = BTreeMap::new();
                for (name, value) in edits.headers.iter().flatten() {
                    if let Some(value) = value {
                        headers.insert(name.clone(), value.clone());
                    }
                }
                mock_response(
                    request,
                    edits.status.unwrap_or(200),
                    headers,
                    edits.response_body.clone().unwrap_or_default(),
                )
            }
            Stage::Request => {
                let mut headers = headers_from(&request["headers"]);
                let headers_changed = edits.apply_headers(&mut headers);
                if edits.url.is_none()
                    && edits.method.is_none()
                    && edits.body.is_none()
                    && !headers_changed
                {
                    plan(env, params, paused.stage, &paused.actions).await
                } else {
                    Outcome::ContinueRequest {
                        url: edits.url.clone(),
                        method: edits.method.clone(),
                        headers: headers_changed.then_some(headers),
                        post_data: edits.body.clone(),
                    }
                }
            }
            Stage::Response => {
                let status = params["responseStatusCode"].as_u64().unwrap_or(200) as u16;
                let phrase = params["responseStatusText"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let mut headers = headers_from_cdp(&params["responseHeaders"]);
                let headers_changed = edits.apply_headers(&mut headers);
                if edits.status.is_some() || edits.response_body.is_some() {
                    let body = match &edits.response_body {
                        Some(body) => body.clone().into_bytes(),
                        None => original_body(env, params).await?,
                    };
                    strip_length_headers(&mut headers);
                    Outcome::Fulfill {
                        status: edits.status.unwrap_or(status),
                        // The old phrase belongs to the old status.
                        phrase: if edits.status.is_some() {
                            String::new()
                        } else {
                            phrase
                        },
                        headers,
                        body,
                    }
                } else if headers_changed {
                    Outcome::ContinueResponse {
                        status,
                        phrase,
                        headers,
                    }
                } else {
                    plan(env, params, paused.stage, &paused.actions).await
                }
            }
        }
    })
}

/// The real response body of a response-stage pause, as bytes.
async fn original_body(env: &Env, params: &Value) -> Result<Vec<u8>, CmdError> {
    let reply = env
        .cdp(
            "Fetch.getResponseBody",
            json!({ "requestId": params["requestId"] }),
        )
        .await?;
    let body = reply["body"].as_str().unwrap_or_default();
    if reply["base64Encoded"].as_bool() == Some(true) {
        B64.decode(body)
            .map_err(|e| CmdError::internal(format!("bad base64 body from CDP: {e}")))
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

/// Release a breakpoint's request when nobody resumed it in time, with what the pending rules
/// would have done (see the module docs).
pub async fn auto_continue(env: Env, pause_id: String) {
    let deadline = match env.tn.lock().paused.get(&pause_id) {
        Some(p) => p.deadline,
        None => return,
    };
    tokio::time::sleep_until(deadline).await;
    let taken = {
        let mut inner = env.tn.lock();
        let taken = inner.paused.remove(&pause_id);
        inner.stop_listener_if_idle();
        taken
    };
    if let Some(paused) = taken {
        let outcome = plan(&env, &paused.params, paused.stage, &paused.actions).await;
        let _ = finish(&env, &paused.request_id, &outcome).await;
        env.events.emit(
            "net.pause_timeout",
            json!({
                "pause_id": pause_id,
                "tab": env.tab,
                "url": paused.url(),
                "warning": format!(
                    "paused request was not resumed within {} s and was {}",
                    PAUSE_TIMEOUT.as_secs(),
                    if paused.actions.is_empty() {
                        "continued unchanged"
                    } else {
                        "released with its matching rules applied"
                    }
                ),
            }),
        );
    }
}

/// Replace a header (all spellings of the name) or add it.
fn set_header(headers: &mut Headers, name: &str, value: &str) {
    headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value.to_string()));
}

/// A new body makes the old length/encoding headers wrong (CDP gives us the decoded body).
fn strip_length_headers(headers: &mut Headers) {
    headers.retain(|(n, _)| {
        !n.eq_ignore_ascii_case("content-length") && !n.eq_ignore_ascii_case("content-encoding")
    });
}

/// `[{name, value}]` (Fetch's `HeaderEntry` list) → header list.
fn headers_from_cdp(v: &Value) -> Headers {
    v.as_array()
        .map(|items| {
            items
                .iter()
                .map(|h| {
                    (
                        h["name"].as_str().unwrap_or_default().to_string(),
                        h["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn headers_to_cdp(headers: &Headers) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}
