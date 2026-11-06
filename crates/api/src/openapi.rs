//! OpenAPI 3.1 document generated from the command registry (`GET /openapi.json`).
//!
//! One path per command: `POST /v1/cmd/{dotted.name}` whose request body is the command's
//! `args_schema` and whose 200 response is its `output_schema`. OpenAPI 3.1 uses JSON Schema
//! 2020-12 directly, so the registry's schemas are embedded as they are (they are generated
//! with `inline_subschemas`, i.e. without `$defs`/`$ref`).

use bmcp_core::{CommandInfo, ErrorCode, Registry, Sensitivity};
use serde_json::{Map, Value, json};

use crate::rest::status_for;

/// Every error code, for the `CmdError.code` enum. Keep in sync with `bmcp_core::ErrorCode`
/// (`rest::status_for` is an exhaustive match, so a new code is noticed there first).
const ERROR_CODES: [ErrorCode; 18] = [
    ErrorCode::StaleRef,
    ErrorCode::AmbiguousTarget,
    ErrorCode::Timeout,
    ErrorCode::NotInteractable,
    ErrorCode::NotFound,
    ErrorCode::TabLocked,
    ErrorCode::TabNotFound,
    ErrorCode::NoTabs,
    ErrorCode::NavigationInterrupted,
    ErrorCode::PageCrashed,
    ErrorCode::TabClosed,
    ErrorCode::NeedsApproval,
    ErrorCode::BlockedInStealth,
    ErrorCode::InvalidParams,
    ErrorCode::ShuttingDown,
    ErrorCode::NotImplemented,
    ErrorCode::DialogOpen,
    ErrorCode::Internal,
];

/// Build the whole document. `server_url` is e.g. `http://127.0.0.1:47800`.
pub fn document(registry: &Registry, server_url: &str) -> Value {
    let mut paths = Map::new();
    paths.insert("/v1/health".into(), health_path());
    paths.insert("/v1/commands".into(), commands_path());
    paths.insert("/v1/ws".into(), ws_path());
    // `list(None)` skips the hidden toolset, like the tool list does.
    for info in registry.list(None) {
        paths.insert(format!("/v1/cmd/{}", info.name), command_path(info));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "BrowserMCP",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Every BrowserMCP function is a command. Call one with \
                POST /v1/cmd/{name} and its arguments as a JSON object. Errors are CmdError \
                objects. Only reachable from localhost; requests with an Origin header are \
                refused unless they come from the BrowserMCP UI.",
        },
        "servers": [{ "url": server_url }],
        "paths": paths,
        "components": { "schemas": { "CmdError": cmd_error_schema() } },
    })
}

fn command_path(info: &CommandInfo) -> Value {
    // 200 = output; every error status (see `rest::status_for`) has a CmdError body.
    let mut responses = Map::new();
    responses.insert(
        "200".into(),
        json!({
            "description": "Command output.",
            "content": { "application/json": { "schema": info.output_schema } },
        }),
    );
    responses.insert(
        "default".into(),
        json!({
            "description": "The command failed; see `code`, `retry` and `hint`.",
            "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/CmdError" }
            } },
        }),
    );
    let sensitivity = match info.sensitivity {
        Sensitivity::Read => "read",
        Sensitivity::Act => "act",
        Sensitivity::Secret => "secret",
    };
    // A body is only needed when some argument is required; an empty body means `{}`.
    let body_required = info.args_schema["required"]
        .as_array()
        .is_some_and(|r| !r.is_empty());
    json!({
        "post": {
            "operationId": info.wire_name,
            "summary": info.name,
            "description": info.description,
            "tags": [info.toolset.as_str()],
            "requestBody": {
                "required": body_required,
                "content": { "application/json": { "schema": info.args_schema } },
            },
            "responses": responses,
            "x-bmcp-toolset": info.toolset.as_str(),
            "x-bmcp-mutates-tab": info.mutates_tab,
            "x-bmcp-sensitivity": sensitivity,
        }
    })
}

fn health_path() -> Value {
    json!({
        "get": {
            "operationId": "health",
            "summary": "Is the browser running?",
            "responses": { "200": {
                "description": "Running.",
                "content": { "application/json": { "schema": {
                    "type": "object",
                    "properties": {
                        "ok": { "type": "boolean" },
                        "name": { "type": "string" },
                        "version": { "type": "string" },
                        "commands": { "type": "integer", "description": "Number of commands." },
                    },
                    "required": ["ok", "name", "version", "commands"],
                } } },
            } },
        }
    })
}

fn commands_path() -> Value {
    json!({
        "get": {
            "operationId": "commands",
            "summary": "List commands with their argument/output JSON Schemas.",
            "parameters": [{
                "name": "toolsets",
                "in": "query",
                "required": false,
                "description": "Comma separated toolsets (core, net, …). Default: all except hidden.",
                "schema": { "type": "string" },
            }],
            "responses": {
                "200": {
                    "description": "Commands sorted by name.",
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "properties": { "commands": { "type": "array", "items": { "type": "object" } } },
                        "required": ["commands"],
                    } } },
                },
                "400": {
                    "description": "Unknown toolset name.",
                    "content": { "application/json": {
                        "schema": { "$ref": "#/components/schemas/CmdError" }
                    } },
                },
            },
        }
    })
}

fn ws_path() -> Value {
    json!({
        "get": {
            "operationId": "ws",
            "summary": "WebSocket: JSON-RPC 2.0 command calls + event notifications.",
            "description": "Upgrade to a WebSocket. Send {\"jsonrpc\":\"2.0\",\"id\":1,\
                \"method\":\"tabs.list\",\"params\":{}}; every command is a method. \
                events.subscribe {topics:[\"command.*\"]} turns on event notifications.",
            "responses": { "101": { "description": "Switching to the WebSocket protocol." } },
        }
    })
}

fn cmd_error_schema() -> Value {
    let codes: Vec<&str> = ERROR_CODES.iter().map(|c| c.as_str()).collect();
    // Document which HTTP status each code comes with.
    let statuses: Vec<String> = ERROR_CODES
        .iter()
        .map(|c| format!("{} → {}", c.as_str(), status_for(*c).as_u16()))
        .collect();
    json!({
        "type": "object",
        "description": format!("Error of a command. HTTP status by code: {}.", statuses.join(", ")),
        "properties": {
            "code": { "type": "string", "enum": codes },
            "message": { "type": "string" },
            "retry": { "type": "string", "enum": ["now", "after_wait", "after_snapshot", "no"] },
            "hint": { "type": "string" },
            "candidates": { "type": "array", "description": "Possible intended targets/names." },
        },
        "required": ["code", "message", "retry"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_path_per_command() {
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        let doc = document(&registry, "http://127.0.0.1:47800");
        assert_eq!(doc["openapi"], "3.1.0");
        let op = &doc["paths"]["/v1/cmd/nav.goto"]["post"];
        assert_eq!(op["operationId"], "nav_goto");
        assert_eq!(op["requestBody"]["required"], true); // `url` is required
        assert_eq!(
            doc["paths"]["/v1/cmd/tabs.list"]["post"]["requestBody"]["required"],
            false
        );
        assert_eq!(
            op["requestBody"]["content"]["application/json"]["schema"]["type"],
            "object"
        );
        assert!(
            op["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
                .is_object()
        );
        for info in registry.list(None) {
            assert!(doc["paths"][format!("/v1/cmd/{}", info.name)].is_object());
        }
    }

    #[test]
    fn error_codes_are_complete() {
        // The wire names must be unique: a duplicate would mean a missing code.
        let mut names: Vec<&str> = ERROR_CODES.iter().map(|c| c.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ERROR_CODES.len());
    }
}
