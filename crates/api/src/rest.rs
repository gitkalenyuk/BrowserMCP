//! REST API generated from the command registry.
//!
//! * `GET  /v1/health` — `{"ok":true,"name":"BrowserMCP","version":"…","commands":N}`
//! * `GET  /v1/commands?toolsets=core,net` — `{"commands":[CommandInfo…]}` (with JSON Schemas).
//!   Without `toolsets`: every command except the `hidden` toolset.
//! * `POST /v1/cmd/{name}` — body = the arguments as a JSON object (empty body = `{}`); `name`
//!   is dotted (`tabs.list`) or the wire name (`tabs_list`). 200 + output JSON, or an error
//!   status (see [`status_for`]) + `CmdError` JSON `{code,message,retry,hint?,candidates?}`.
//! * `GET  /openapi.json` — OpenAPI 3.1 document, see [`crate::openapi`].

use std::str::FromStr;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bmcp_core::{Caller, CmdError, ErrorCode, Toolset};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::ApiState;

pub fn router() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/v1/health", get(health))
        .route("/v1/commands", get(list_commands))
        .route("/v1/cmd/{name}", post(call_command))
        .route("/openapi.json", get(openapi))
}

/// HTTP status for each error code. The match has no `_` arm on purpose: adding a new
/// `ErrorCode` fails to compile until it gets a status here.
pub fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidParams => StatusCode::BAD_REQUEST,
        ErrorCode::NeedsApproval | ErrorCode::BlockedInStealth => StatusCode::FORBIDDEN,
        ErrorCode::NotFound | ErrorCode::TabNotFound => StatusCode::NOT_FOUND,
        // The request was fine but the page/browser state doesn't allow it right now.
        ErrorCode::StaleRef
        | ErrorCode::AmbiguousTarget
        | ErrorCode::NoTabs
        | ErrorCode::NavigationInterrupted => StatusCode::CONFLICT,
        ErrorCode::TabClosed => StatusCode::GONE,
        ErrorCode::NotInteractable => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::TabLocked => StatusCode::LOCKED,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        // The renderer (our "upstream") failed.
        ErrorCode::PageCrashed => StatusCode::BAD_GATEWAY,
        ErrorCode::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
    }
}

/// `CmdError` → HTTP response with the matching status and the error as JSON body.
pub fn error_response(err: CmdError) -> Response {
    (status_for(err.code), Json(err)).into_response()
}

async fn health(State(state): State<ApiState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "name": "BrowserMCP",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": state.registry.len(),
    }))
}

#[derive(Deserialize)]
struct CommandsQuery {
    /// Comma separated toolset names, e.g. `core,net`.
    toolsets: Option<String>,
}

async fn list_commands(
    State(state): State<ApiState>,
    Query(query): Query<CommandsQuery>,
) -> Response {
    let toolsets = match parse_toolsets(query.toolsets.as_deref()) {
        Ok(toolsets) => toolsets,
        Err(err) => return error_response(err),
    };
    let commands = state.registry.list(toolsets.as_deref());
    Json(json!({ "commands": commands })).into_response()
}

/// `None`/`""` → `None` (all but hidden); `"core,net"` → those. Unknown names are an error that
/// lists the valid ones (unlike MCP, which ignores them: a REST caller can read the message).
fn parse_toolsets(raw: Option<&str>) -> Result<Option<Vec<Toolset>>, CmdError> {
    let names: Vec<&str> = raw
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(None);
    }
    let toolsets = names
        .into_iter()
        .map(Toolset::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            err.with_candidates(Toolset::ALL.iter().map(|t| json!(t.as_str())).collect())
        })?;
    Ok(Some(toolsets))
}

async fn call_command(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    // Any content type is accepted (curl -d sends form-urlencoded); the body must be JSON.
    let args = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null // the registry treats null as {}
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(args) => args,
            Err(err) => {
                return error_response(
                    CmdError::invalid_params(format!("request body is not valid JSON: {err}"))
                        .with_hint("Send the arguments as a JSON object, e.g. {\"url\":\"…\"}."),
                );
            }
        }
    };

    let ctx = state.ctx(Caller::Rest);
    // Run the command in its own task: if the HTTP client disconnects, axum drops this
    // handler, but the command still finishes cleanly (and emits `command.finished`).
    let call = tokio::spawn(state.registry.call(ctx, &name, args));
    match call.await {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => error_response(err),
        Err(join_err) => error_response(CmdError::internal(format!(
            "command {name} panicked: {join_err}"
        ))),
    }
}

async fn openapi(State(state): State<ApiState>, headers: HeaderMap) -> Json<Value> {
    // The guard already checked that Host is a loopback name, so it is safe to echo it back.
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1:47800");
    Json(crate::openapi::document(
        &state.registry,
        &format!("http://{host}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolset_parsing() {
        assert_eq!(parse_toolsets(None).unwrap(), None);
        assert_eq!(parse_toolsets(Some(" ")).unwrap(), None);
        assert_eq!(
            parse_toolsets(Some("core, net")).unwrap(),
            Some(vec![Toolset::Core, Toolset::Net])
        );
        let err = parse_toolsets(Some("core,nope")).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(err.candidates.contains(&json!("net")));
    }
}
