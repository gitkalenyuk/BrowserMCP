//! The [`Registry`]: all commands, type-erased to "JSON in, JSON out".
//!
//! Every call goes through the same pipeline (the "middleware"):
//!
//! 1. find the command by dotted name (`tabs.list`) or wire name (`tabs_list`);
//! 2. deserialize the JSON arguments into `C::Args` (errors → `INVALID_PARAMS` with the field path);
//! 3. lock check through the [`LockPolicy`] if the command mutates a tab (→ `TAB_LOCKED`);
//! 4. emit `command.started`, run, emit `command.finished` (with `duration_ms` and the error code).
//!    While it runs, the [`CommandGuard`]s may cancel it (e.g. `DIALOG_OPEN`).
//!
//! A call refused in steps 1–3 emits a single `command.rejected` event instead.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{
    AllowAll, CmdError, Command, CommandGuard, Ctx, ErrorCode, LockPolicy, Sensitivity, Toolset,
    args_schema, output_schema, wire_name,
};
use crate::backend::BoxFuture;
use crate::events::topics;
use crate::extensions::Extensions;

/// A type-erased command: takes the context and raw JSON args, returns JSON output.
type Handler = Box<dyn Fn(Ctx, Value) -> BoxFuture<'static, Result<Value, CmdError>> + Send + Sync>;

/// Everything an API surface needs to expose a command (tool list, OpenAPI, palette, docs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandInfo {
    /// Dotted name, e.g. `"tabs.list"`.
    pub name: &'static str,
    /// Name without dots for MCP / the Anthropic API, e.g. `"tabs_list"`.
    pub wire_name: String,
    pub description: &'static str,
    pub toolset: Toolset,
    pub mutates_tab: bool,
    pub sensitivity: Sensitivity,
    pub palette: Option<&'static str>,
    pub default_keys: &'static [&'static str],
    /// JSON Schema (draft 2020-12) of the arguments; always `"type": "object"`.
    pub args_schema: Value,
    /// JSON Schema of the output; always `"type": "object"`.
    pub output_schema: Value,
}

struct Entry {
    info: CommandInfo,
    handler: Handler,
}

/// The set of all commands. Build it once at startup, then share it as `Arc<Registry>`.
pub struct Registry {
    /// Keyed by dotted name; a `BTreeMap` keeps `list()` sorted.
    commands: BTreeMap<&'static str, Entry>,
    /// Wire name → dotted name.
    by_wire: HashMap<String, &'static str>,
    lock: Arc<dyn LockPolicy>,
    /// Watchers that may cancel a running command (see [`CommandGuard`]).
    guards: Arc<Vec<Arc<dyn CommandGuard>>>,
    /// Shared services handed to every call as `ctx.extensions`.
    extensions: Arc<Extensions>,
    next_call_id: AtomicU64,
}

impl Registry {
    /// An empty registry with the [`AllowAll`] lock policy.
    /// Add the built-in commands with [`crate::commands::register_builtin`].
    pub fn new() -> Self {
        Registry {
            commands: BTreeMap::new(),
            by_wire: HashMap::new(),
            lock: Arc::new(AllowAll),
            guards: Arc::new(Vec::new()),
            extensions: Arc::new(Extensions::new()),
            next_call_id: AtomicU64::new(0),
        }
    }

    /// Register command `C`.
    ///
    /// # Panics
    /// On programming mistakes, so they show up on the first test run: a malformed `NAME`, a
    /// duplicate name (dotted or wire), or `Args`/`Output` that are not JSON objects.
    pub fn add<C: Command>(&mut self) -> &mut Self {
        let name = C::NAME;
        check_name(name);
        let wire = wire_name(name);
        assert!(
            !self.commands.contains_key(name),
            "command {name} registered twice"
        );
        assert!(
            !self.by_wire.contains_key(&wire),
            "command {name}: wire name {wire} is already used by {}",
            self.by_wire[&wire]
        );

        let args_schema = args_schema::<C::Args>();
        let output_schema = output_schema::<C::Output>();
        check_object_schema(name, "Args", &args_schema);
        check_object_schema(name, "Output", &output_schema);

        let meta = C::meta();
        let info = CommandInfo {
            name,
            wire_name: wire.clone(),
            description: meta.description,
            toolset: meta.toolset,
            mutates_tab: meta.mutates_tab,
            sensitivity: meta.sensitivity,
            palette: meta.palette,
            default_keys: meta.default_keys,
            args_schema,
            output_schema,
        };
        // The only place where the concrete type `C` is known: wrap it in a closure.
        let handler: Handler = Box::new(|ctx, args| Box::pin(invoke::<C>(ctx, args)));

        self.by_wire.insert(wire, name);
        self.commands.insert(name, Entry { info, handler });
        self
    }

    /// Replace the tab-lock policy (default: [`AllowAll`]).
    pub fn set_lock_policy(&mut self, policy: Arc<dyn LockPolicy>) {
        self.lock = policy;
    }

    /// Add a [`CommandGuard`]; it sees every call from now on.
    pub fn add_guard(&mut self, guard: Arc<dyn CommandGuard>) {
        Arc::make_mut(&mut self.guards).push(guard);
    }

    /// The services map every call gets as `ctx.extensions`. Command crates put their
    /// long-lived state here when they register (e.g. the per-tab ref registries).
    pub fn extensions(&self) -> &Arc<Extensions> {
        &self.extensions
    }

    /// Commands sorted by name.
    ///
    /// * `None` → every command except the [`Toolset::Hidden`] ones;
    /// * `Some(&[Toolset::Core, Toolset::Net])` → only those toolsets (Hidden only if listed).
    pub fn list(&self, toolsets: Option<&[Toolset]>) -> Vec<&CommandInfo> {
        self.commands
            .values()
            .map(|e| &e.info)
            .filter(|info| match toolsets {
                None => info.toolset != Toolset::Hidden,
                Some(wanted) => wanted.contains(&info.toolset),
            })
            .collect()
    }

    /// Look up a command by dotted or wire name.
    pub fn get(&self, name: &str) -> Option<&CommandInfo> {
        self.entry(name).map(|e| &e.info)
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Run a command by dotted (`"nav.goto"`) or wire (`"nav_goto"`) name.
    ///
    /// `args` must be a JSON object; `null` is treated as `{}`. The returned future does not
    /// borrow the registry, so it can be `tokio::spawn`ed.
    pub fn call(
        &self,
        mut ctx: Ctx,
        name: &str,
        args: Value,
    ) -> BoxFuture<'static, Result<Value, CmdError>> {
        let args = if args.is_null() { json!({}) } else { args };
        ctx.call_id = self.next_call_id.fetch_add(1, Ordering::Relaxed) + 1;

        let Some(entry) = self.entry(name) else {
            let err = self.unknown_command(name);
            emit_rejected(&ctx, name, &args, &err);
            return Box::pin(std::future::ready(Err(err)));
        };
        if !args.is_object() {
            let err = CmdError::invalid_params(format!(
                "arguments for {} must be a JSON object, got {args}",
                entry.info.name
            ));
            emit_rejected(&ctx, entry.info.name, &args, &err);
            return Box::pin(std::future::ready(Err(err)));
        }
        ctx.lock = self.lock.clone();
        ctx.guards = self.guards.clone();
        ctx.extensions = self.extensions.clone();
        ctx.mutates_tab = entry.info.mutates_tab;
        (entry.handler)(ctx, args)
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        let dotted = match self.commands.get_key_value(name) {
            Some((key, _)) => *key,
            None => *self.by_wire.get(name)?,
        };
        self.commands.get(dotted)
    }

    /// `NOT_FOUND`, with commands of the same group (`tabs` for `tabs_lst`) as candidates.
    fn unknown_command(&self, name: &str) -> CmdError {
        let group = name.split(['.', '_']).next().unwrap_or_default();
        let wants_wire = !name.contains('.');
        let candidates = self
            .commands
            .values()
            .filter(|e| e.info.name.split('.').next() == Some(group))
            .map(|e| {
                if wants_wire {
                    json!(e.info.wire_name)
                } else {
                    json!(e.info.name)
                }
            })
            .take(20)
            .collect();
        CmdError::new(ErrorCode::NotFound, format!("unknown command '{name}'"))
            .with_retry(super::Retry::No)
            .with_hint("List the available commands (MCP: tools/list) and use one of those names.")
            .with_candidates(candidates)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The typed half of the pipeline for one command type `C` (steps 2–4).
async fn invoke<C: Command>(ctx: Ctx, raw_args: Value) -> Result<Value, CmdError> {
    let args: C::Args = match parse_args(C::NAME, &raw_args) {
        Ok(args) => args,
        Err(err) => {
            emit_rejected(&ctx, C::NAME, &raw_args, &err);
            return Err(err);
        }
    };

    let tab = C::target_tab(&args);
    if ctx.mutates_tab
        && let Err(err) = ctx.lock.check(&ctx.caller, tab)
    {
        emit_rejected(&ctx, C::NAME, &raw_args, &err);
        return Err(err);
    }

    let caller = serde_json::to_value(&ctx.caller).unwrap_or(Value::Null);
    let call_id = ctx.call_id;
    let events = ctx.events.clone();
    events.emit(
        topics::COMMAND_STARTED,
        json!({ "call_id": call_id, "name": C::NAME, "caller": caller, "args": raw_args, "tab": tab }),
    );

    let started = Instant::now();
    // Ask the guards before running: their futures may cancel the command (see `CommandGuard`).
    let watches: Vec<_> = ctx
        .guards
        .iter()
        .filter_map(|guard| guard.watch(&ctx, C::NAME, &raw_args))
        .collect();
    let run = C::run(ctx, args);
    let outcome = if watches.is_empty() {
        run.await
    } else {
        tokio::select! {
            // `biased`: the guards are polled first, so one that already fires stops the
            // command before it does anything.
            biased;
            err = first_of(watches) => Err(err),
            out = run => out,
        }
    };
    let result = match outcome {
        Ok(output) => serde_json::to_value(output).map_err(|e| {
            CmdError::internal(format!("cannot serialize output of {}: {e}", C::NAME))
        }),
        Err(err) => Err(err),
    };

    events.emit(
        topics::COMMAND_FINISHED,
        json!({
            "call_id": call_id,
            "name": C::NAME,
            "caller": caller,
            "duration_ms": started.elapsed().as_millis() as u64,
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.code),
        }),
    );
    result
}

/// Resolves with the error of whichever watch future finishes first.
async fn first_of(mut watches: Vec<BoxFuture<'static, CmdError>>) -> CmdError {
    std::future::poll_fn(move |cx| {
        for watch in &mut watches {
            if let std::task::Poll::Ready(err) = watch.as_mut().poll(cx) {
                return std::task::Poll::Ready(err);
            }
        }
        std::task::Poll::Pending
    })
    .await
}

/// JSON → `T`, with the path of the bad field in the message, e.g.
/// ``invalid arguments for nav.goto: `tab`: invalid type: string "x", expected i32``.
fn parse_args<T: DeserializeOwned>(name: &str, raw: &Value) -> Result<T, CmdError> {
    serde_path_to_error::deserialize(raw).map_err(|err| {
        let path = err.path().to_string();
        let inner = err.into_inner();
        let message = if path == "." {
            format!("invalid arguments for {name}: {inner}")
        } else {
            format!("invalid arguments for {name}: `{path}`: {inner}")
        };
        CmdError::invalid_params(message).with_hint(format!(
            "Check the argument schema of {} in the tool list.",
            wire_name(name)
        ))
    })
}

fn emit_rejected(ctx: &Ctx, name: &str, args: &Value, err: &CmdError) {
    ctx.events.emit(
        topics::COMMAND_REJECTED,
        json!({
            "call_id": ctx.call_id,
            "name": name,
            "caller": ctx.caller,
            "args": args,
            "error": err.code,
            "message": err.message,
        }),
    );
}

/// `group.action[.more]`, lowercase ASCII letters, digits and `_`; wire name ≤ 64 chars
/// (the Anthropic tool-name limit).
fn check_name(name: &str) {
    let segments: Vec<&str> = name.split('.').collect();
    let valid = segments.len() >= 2
        && segments.iter().all(|s| {
            s.starts_with(|c: char| c.is_ascii_lowercase())
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        && name.len() <= 64;
    assert!(
        valid,
        "invalid command name '{name}': use lowercase dotted names like \"tabs.list\""
    );
}

fn check_object_schema(name: &str, what: &str, schema: &Value) {
    assert!(
        schema.get("type") == Some(&json!("object")),
        "command {name}: {what} must be a struct (JSON object) because MCP requires object \
         schemas; got {schema}"
    );
}
