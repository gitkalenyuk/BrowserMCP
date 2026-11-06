//! `cargo xtask gen-sdk [--check]`: TypeScript types and wrappers from the command registry.
//!
//! 1. Runs `bmcp-schema --hidden` (crate `crates/schema-dump`), which prints every registered
//!    command with its JSON Schemas — the same data `GET /v1/commands` serves.
//! 2. Converts each command's `args_schema` / `output_schema` to TypeScript (a small converter
//!    below: it only needs the subset of JSON Schema that `schemars` emits).
//! 3. Writes
//!    * `sdk/protocol/src/generated/commands.ts` — `XxxArgs` / `XxxOutput`, the `Commands` map,
//!      `COMMANDS` metadata, the `Browser` interface and `createBrowser(transport)`;
//!    * `crates/script/types/browser.d.ts` — the same types as globals for scripts.
//!
//! `--check` writes nothing and fails when a file on disk differs (CI uses it).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{Result, workspace_root};

/// Generated files, relative to the workspace root.
const PROTOCOL_FILE: &str = "sdk/protocol/src/generated/commands.ts";
const SCRIPT_TYPES_FILE: &str = "crates/script/types/browser.d.ts";

/// Names the SDK puts next to the command namespaces on the `browser` object.
const RESERVED_NAMESPACES: &[&str] = &["call", "on", "close", "events", "transport"];

pub fn gen_sdk(check: bool) -> Result {
    let dump = schema_dump()?;
    let commands = parse_commands(&dump)?;
    let files = [
        (PROTOCOL_FILE, protocol_ts(&commands)?),
        (SCRIPT_TYPES_FILE, script_dts(&commands)?),
    ];

    let root = workspace_root();
    let mut stale = Vec::new();
    for (path, content) in &files {
        let full = root.join(path);
        // Compare without '\r' so a CRLF checkout does not count as stale.
        let current = std::fs::read_to_string(&full).unwrap_or_default();
        if current.replace('\r', "") == *content {
            continue;
        }
        if check {
            stale.push(*path);
        } else {
            write_file(&full, content)?;
            println!("wrote {path}");
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "generated SDK files are stale: {}\nrun `cargo xtask gen-sdk` and commit the result",
            stale.join(", ")
        ));
    }
    println!("{} commands, SDK files up to date", commands.len());
    Ok(())
}

fn write_file(path: &Path, content: &str) -> Result {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Build and run `bmcp-schema --hidden`; returns its JSON.
fn schema_dump() -> Result<Value> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "--package",
            "bmcp-schema-dump",
            "--bin",
            "bmcp-schema",
        ])
        .args(["--", "--hidden"])
        .current_dir(workspace_root())
        .output()
        .map_err(|e| format!("cannot start cargo: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "bmcp-schema failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("bmcp-schema printed bad JSON: {e}"))
}

// ---------------------------------------------------------------------------------------------
// The command list

/// The parts of a `CommandInfo` the generator uses.
struct Cmd {
    /// Dotted name, `"tabs.list"`.
    name: String,
    /// `"TabsList"`: prefix of the generated type names.
    type_name: String,
    description: String,
    wire_name: String,
    toolset: String,
    sensitivity: String,
    mutates_tab: bool,
    palette: Option<String>,
    default_keys: Vec<String>,
    args: Value,
    output: Value,
}

impl Cmd {
    fn args_type(&self) -> String {
        format!("{}Args", self.type_name)
    }

    fn output_type(&self) -> String {
        format!("{}Output", self.type_name)
    }

    /// True when every argument is optional, so `browser.tabs.list()` needs no `{}`.
    fn args_optional(&self) -> bool {
        self.args["required"]
            .as_array()
            .is_none_or(|r| r.is_empty())
    }

    /// JSDoc lines for the wrapper method.
    fn doc(&self) -> Vec<String> {
        let mut doc = lines(&self.description);
        let mut facts = vec![
            format!("toolset `{}`", self.toolset),
            self.sensitivity.clone(),
        ];
        if self.mutates_tab {
            facts.push("mutates the tab".into());
        }
        doc.push(String::new());
        doc.push(format!(
            "`{}` (MCP: `{}`): {}.",
            self.name,
            self.wire_name,
            facts.join(", ")
        ));
        if self.toolset == "hidden" {
            doc.push("Internal (hidden toolset): not listed to agents by default.".into());
        }
        doc
    }
}

fn parse_commands(dump: &Value) -> Result<Vec<Cmd>> {
    let list = dump["commands"]
        .as_array()
        .ok_or("bmcp-schema: no `commands` array")?;
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
    let mut commands: Vec<Cmd> = list
        .iter()
        .map(|c| Cmd {
            name: text(c, "name"),
            type_name: pascal(&text(c, "name")),
            description: text(c, "description"),
            wire_name: text(c, "wire_name"),
            toolset: text(c, "toolset"),
            sensitivity: text(c, "sensitivity"),
            mutates_tab: c["mutates_tab"].as_bool().unwrap_or(false),
            palette: c["palette"].as_str().map(str::to_owned),
            default_keys: c["default_keys"]
                .as_array()
                .map(|keys| {
                    keys.iter()
                        .filter_map(|k| k.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            args: c["args_schema"].clone(),
            output: c["output_schema"].clone(),
        })
        .collect();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    for cmd in &commands {
        let ns = cmd.name.split('.').next().unwrap_or_default();
        if RESERVED_NAMESPACES.contains(&ns) {
            return Err(format!(
                "command {}: namespace `{ns}` is reserved by the SDK's browser object",
                cmd.name
            ));
        }
    }
    Ok(commands)
}

/// `"page.act_batch"` → `"PageActBatch"`.
fn pascal(name: &str) -> String {
    name.split(['.', '_', '-'])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The two output files

const HEADER: &str = "// GENERATED by `cargo xtask gen-sdk` from the command registry. Do not edit.\n\
// Source of truth: the Rust `Command` types (see crates/registry); regenerate after changing them.\n";

/// `sdk/protocol/src/generated/commands.ts`.
fn protocol_ts(commands: &[Cmd]) -> Result<String> {
    let mut out = String::from(HEADER);
    out.push_str("\nimport type { CallOptions, Transport } from \"../transport.js\";\n");
    out.push_str(&type_declarations(commands, 0));
    out.push_str(&commands_map(commands, 0));
    out.push_str(&command_meta(commands));

    out.push_str(
        "\n/** Arguments of `browser.call(name, …)`: optional when the command has no required argument. */\n\
         export type CallArgs<N extends CommandName> = {} extends CommandArgs<N>\n  \
         ? [args?: CommandArgs<N>, options?: CallOptions]\n  \
         : [args: CommandArgs<N>, options?: CallOptions];\n",
    );
    let tree = namespace_tree(commands, true)?;
    out.push_str(
        "\n/** Every command as `browser.<group>.<action>(args, options?)`. Built by {@link createBrowser}. */\n\
         export interface Browser {\n  \
         /** Call a command by its dotted name: `browser.call(\"tabs.list\")`. */\n  \
         call<N extends CommandName>(name: N, ...args: CallArgs<N>): Promise<CommandOutput<N>>;\n",
    );
    out.push_str(&interface_members(&tree, commands, 1, true));
    out.push_str("}\n");

    out.push_str(
        "\n/** The typed `browser` object on top of any transport (WebSocket, HTTP, a mock…). */\n\
         export function createBrowser(transport: Transport): Browser {\n  \
         const call = <T>(name: string, args?: object, options?: CallOptions): Promise<T> =>\n    \
         transport.call(name, args ?? {}, options) as Promise<T>;\n  \
         return {\n    \
         call: ((name: string, args?: object, options?: CallOptions) =>\n      call(name, args, options)) as Browser[\"call\"],\n",
    );
    out.push_str(&object_members(&tree, commands, 2));
    out.push_str("  };\n}\n");
    Ok(out)
}

/// `crates/script/types/browser.d.ts`: ambient (global) types for the script host.
fn script_dts(commands: &[Cmd]) -> Result<String> {
    let mut out = String::from(HEADER);
    out.push_str(
        "//\n\
         // Ambient types for BrowserMCP scripts, for editors (VS Code / Monaco). The script host\n\
         // only strips types; it does not type-check.\n\
         //\n\
         // Use from a script:  /// <reference path=\"../../crates/script/types/browser.d.ts\" />\n\
         //\n\
         // All argument/output types live in the `BrowserMCP` namespace, e.g. `BrowserMCP.TabsListOutput`.\n\
         \n\
         declare namespace BrowserMCP {",
    );
    out.push_str(&type_declarations(commands, 1));
    out.push_str(&commands_map(commands, 1));
    // The prelude maps `browser.<ns>.<method>` to `"<ns>.<method>"` (two levels only), so deeper
    // names are reachable through `browser.call` alone.
    let tree = namespace_tree(commands, false)?;
    out.push_str(
        "\n  /** The global `browser`: `browser.<group>.<action>(args)` sends that command. */\n  \
         export interface Browser {\n    \
         /** Low level: `browser.call(\"tabs.list\", {})`. Also accepts wire names (`tabs_list`). */\n    \
         call<N extends keyof Commands>(name: N, args?: Commands[N][\"args\"]): Promise<Commands[N][\"output\"]>;\n",
    );
    out.push_str(&interface_members(&tree, commands, 2, false));
    out.push_str("  }\n}\n");
    out.push_str(
        "\n/** `browser.<ns>.<method>(args)` sends command \"<ns>.<method>\" to the browser. */\n\
         declare const browser: BrowserMCP.Browser;\n\
         \n\
         /** Values from `--input key=value` (always strings). */\n\
         declare const inputs: Readonly<Record<string, string | undefined>>;\n\
         \n\
         /** Wait `ms` milliseconds. */\n\
         declare function sleep(ms: number): Promise<void>;\n\
         \n\
         /** log/info/debug go to the script host's stdout, warn/error to stderr. */\n\
         interface Console {\n  \
         log(...data: unknown[]): void;\n  \
         info(...data: unknown[]): void;\n  \
         debug(...data: unknown[]): void;\n  \
         warn(...data: unknown[]): void;\n  \
         error(...data: unknown[]): void;\n\
         }\n\
         // `var` (not `const`) so it merges with the DOM / Node declarations when those are loaded.\n\
         declare var console: Console;\n",
    );
    Ok(out)
}

/// `XxxArgs` / `XxxOutput` (and their `$defs`, if any) for every command.
fn type_declarations(commands: &[Cmd], ind: usize) -> String {
    let mut out = String::new();
    for cmd in commands {
        let ts = TsGen {
            defs_prefix: cmd.type_name.clone(),
        };
        out.push_str(&format!("\n{}// ---- {} ----\n", indent(ind), cmd.name));
        for (type_name, schema, what) in [
            (cmd.args_type(), &cmd.args, "Arguments"),
            (cmd.output_type(), &cmd.output, "Result"),
        ] {
            for defs_key in ["$defs", "definitions"] {
                if let Some(defs) = schema[defs_key].as_object() {
                    for (def_name, def) in defs {
                        let name = format!("{}{}", cmd.type_name, pascal(def_name));
                        out.push('\n');
                        out.push_str(&ts.declare(&name, def, Vec::new(), ind));
                    }
                }
            }
            let mut doc = vec![format!("{what} of `{}`.", cmd.name)];
            doc.extend(lines(schema["description"].as_str().unwrap_or_default()));
            out.push('\n');
            out.push_str(&ts.declare(&type_name, schema, doc, ind));
        }
    }
    out
}

/// `export interface Commands { "tabs.list": { args: TabsListArgs; output: TabsListOutput }; … }`.
fn commands_map(commands: &[Cmd], ind: usize) -> String {
    let i = indent(ind);
    let mut out = format!(
        "\n{i}/** Every command by dotted name, with its argument and result types. */\n\
         {i}export interface Commands {{\n"
    );
    for cmd in commands {
        out.push_str(&jsdoc(&lines(&cmd.description), ind + 1));
        out.push_str(&format!(
            "{}{}: {{ args: {}; output: {} }};\n",
            indent(ind + 1),
            literal(&cmd.name),
            cmd.args_type(),
            cmd.output_type()
        ));
    }
    out.push_str(&format!("{i}}}\n"));
    if ind == 0 {
        out.push_str(
            "\n/** A dotted command name such as `\"tabs.list\"`. */\n\
             export type CommandName = keyof Commands;\n\
             export type CommandArgs<N extends CommandName> = Commands[N][\"args\"];\n\
             export type CommandOutput<N extends CommandName> = Commands[N][\"output\"];\n",
        );
    }
    out
}

/// `COMMANDS`: static facts about every command, for palettes, docs and tool lists.
fn command_meta(commands: &[Cmd]) -> String {
    let mut out = String::from(
        "\n/** Static facts about a command (from its Rust `Meta`). */\n\
         export interface CommandMeta {\n  \
         /** MCP / Anthropic tool name (no dots), e.g. `tabs_list`. */\n  \
         wireName: string;\n  \
         description: string;\n  \
         toolset: string;\n  \
         sensitivity: \"read\" | \"act\" | \"secret\";\n  \
         /** True when it changes an existing tab (goes through the tab-lock check). */\n  \
         mutatesTab: boolean;\n  \
         /** Title in the Ctrl+K palette, or null when not shown there. */\n  \
         palette: string | null;\n  \
         defaultKeys: readonly string[];\n\
         }\n\
         \n\
         /** Metadata of every command, by dotted name. */\n\
         export const COMMANDS: { readonly [N in CommandName]: CommandMeta } = {\n",
    );
    for cmd in commands {
        let keys: Vec<String> = cmd.default_keys.iter().map(|k| literal(k)).collect();
        out.push_str(&format!(
            "  {}: {{\n    wireName: {},\n    description: {},\n    toolset: {},\n    \
             sensitivity: {},\n    mutatesTab: {},\n    palette: {},\n    defaultKeys: [{}],\n  }},\n",
            literal(&cmd.name),
            literal(&cmd.wire_name),
            literal(&cmd.description),
            literal(&cmd.toolset),
            literal(&cmd.sensitivity),
            cmd.mutates_tab,
            cmd.palette.as_deref().map_or("null".into(), literal),
            keys.join(", ")
        ));
    }
    out.push_str("};\n");
    out
}

// ---------------------------------------------------------------------------------------------
// Namespaces: `tabs.list` → browser.tabs.list

/// A group (`tabs`) holds commands and, for names with 3+ parts, sub-groups.
enum Node {
    /// Index into the command list.
    Command(usize),
    Group(BTreeMap<String, Node>),
}

/// `deep = false` skips names with more than two parts (the script prelude cannot call them).
fn namespace_tree(commands: &[Cmd], deep: bool) -> Result<BTreeMap<String, Node>> {
    let mut root = BTreeMap::new();
    for (index, cmd) in commands.iter().enumerate() {
        let parts: Vec<&str> = cmd.name.split('.').collect();
        if !deep && parts.len() > 2 {
            continue;
        }
        let (last, groups) = parts.split_last().ok_or("empty command name")?;
        let mut level = &mut root;
        for group in groups {
            let node = level
                .entry(group.to_string())
                .or_insert_with(|| Node::Group(BTreeMap::new()));
            level = match node {
                Node::Group(children) => children,
                Node::Command(_) => {
                    return Err(format!("{} is both a command and a group", cmd.name));
                }
            };
        }
        if level
            .insert(last.to_string(), Node::Command(index))
            .is_some()
        {
            return Err(format!("{} is both a command and a group", cmd.name));
        }
    }
    Ok(root)
}

/// Members of `interface Browser`: `tabs: { list(args?: …): Promise<…>; };`.
fn interface_members(
    tree: &BTreeMap<String, Node>,
    commands: &[Cmd],
    ind: usize,
    with_options: bool,
) -> String {
    let i = indent(ind);
    let mut out = String::new();
    for (key, node) in tree {
        match node {
            Node::Command(index) => {
                let cmd = &commands[*index];
                let optional = if cmd.args_optional() { "?" } else { "" };
                let options = if with_options {
                    ", options?: CallOptions"
                } else {
                    ""
                };
                out.push_str(&jsdoc(&cmd.doc(), ind));
                out.push_str(&format!(
                    "{i}{}(args{optional}: {}{options}): Promise<{}>;\n",
                    property_name(key),
                    cmd.args_type(),
                    cmd.output_type()
                ));
            }
            Node::Group(children) => {
                out.push_str(&format!("{i}/** Commands `{key}.*`. */\n"));
                out.push_str(&format!("{i}{}: {{\n", property_name(key)));
                out.push_str(&interface_members(
                    children,
                    commands,
                    ind + 1,
                    with_options,
                ));
                out.push_str(&format!("{i}}};\n"));
            }
        }
    }
    out
}

/// The object literal inside `createBrowser`: `tabs: { list: (args, options) => call(…) },`.
fn object_members(tree: &BTreeMap<String, Node>, commands: &[Cmd], ind: usize) -> String {
    let i = indent(ind);
    let mut out = String::new();
    for (key, node) in tree {
        match node {
            Node::Command(index) => {
                let cmd = &commands[*index];
                out.push_str(&format!(
                    "{i}{}: (args, options) => call<{}>({}, args, options),\n",
                    property_name(key),
                    cmd.output_type(),
                    literal(&cmd.name)
                ));
            }
            Node::Group(children) => {
                out.push_str(&format!("{i}{}: {{\n", property_name(key)));
                out.push_str(&object_members(children, commands, ind + 1));
                out.push_str(&format!("{i}}},\n"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// JSON Schema → TypeScript
//
// Handles what schemars produces: `type` (also `["integer","null"]`), `properties`/`required`,
// `additionalProperties`, `items`, `prefixItems`, `enum`, `const`, `oneOf`/`anyOf` (unions),
// `allOf` (intersection), `$ref` to `$defs`, `description` → JSDoc. Unknown shapes → `unknown`.

struct TsGen {
    /// `$ref: "#/$defs/Step"` becomes `<prefix>Step`.
    defs_prefix: String,
}

impl TsGen {
    /// `export interface Name { … }` for plain objects, else `export type Name = …;`.
    fn declare(&self, name: &str, schema: &Value, mut doc: Vec<String>, ind: usize) -> String {
        doc.extend(self.extra_doc(schema));
        let i = indent(ind);
        let mut out = jsdoc(&doc, ind);
        let is_plain_object = schema["type"] == "object"
            && schema["properties"]
                .as_object()
                .is_some_and(|p| !p.is_empty())
            && !has_union(schema);
        if is_plain_object {
            out.push_str(&format!(
                "{i}export interface {name} {}\n",
                self.object(schema, ind)
            ));
        } else {
            out.push_str(&format!(
                "{i}export type {name} = {};\n",
                self.ts_type(schema, ind)
            ));
        }
        out
    }

    /// The type as one string: union members joined with ` | `.
    fn ts_type(&self, schema: &Value, ind: usize) -> String {
        self.members(schema, ind).join(" | ")
    }

    /// The union members of a schema (one entry when it is not a union).
    fn members(&self, schema: &Value, ind: usize) -> Vec<String> {
        let obj = match schema {
            Value::Bool(true) => return vec!["unknown".into()],
            Value::Bool(false) => return vec!["never".into()],
            Value::Object(obj) => obj,
            _ => return vec!["unknown".into()],
        };
        let mut members: Vec<String> = Vec::new();
        if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
            let def = reference.rsplit('/').next().unwrap_or(reference);
            members.push(format!("{}{}", self.defs_prefix, pascal(def)));
        } else if let Some(value) = obj.get("const") {
            members.push(literal_json(value));
        } else if let Some(values) = obj.get("enum").and_then(Value::as_array) {
            members.extend(values.iter().map(literal_json));
        } else if let Some(variants) = obj
            .get("oneOf")
            .or_else(|| obj.get("anyOf"))
            .and_then(Value::as_array)
        {
            for variant in variants {
                members.extend(self.members(variant, ind));
            }
            // `{"type": "null", "oneOf": […]}`-style nullability next to the union.
            if type_names(schema).contains(&"null") {
                members.push("null".into());
            }
        } else if let Some(parts) = obj.get("allOf").and_then(Value::as_array) {
            let parts: Vec<String> = parts
                .iter()
                .map(|p| parenthesize(self.members(p, ind)))
                .collect();
            members.push(parts.join(" & "));
        } else {
            let types = type_names(schema);
            if types.is_empty() {
                // No `type`: guess from the keywords, else anything goes.
                if obj.contains_key("properties") {
                    members.push(self.object(schema, ind));
                } else if obj.contains_key("items") {
                    members.push(self.array(schema, ind));
                } else {
                    members.push("unknown".into());
                }
            }
            for t in types {
                members.push(match t {
                    "string" => "string".into(),
                    "integer" | "number" => "number".into(),
                    "boolean" => "boolean".into(),
                    "null" => "null".into(),
                    "object" => self.object(schema, ind),
                    "array" => self.array(schema, ind),
                    _ => "unknown".into(),
                });
            }
        }
        // Dedupe (e.g. two `null`s); `unknown` swallows everything else.
        let mut unique: Vec<String> = Vec::new();
        for m in members {
            if !unique.contains(&m) {
                unique.push(m);
            }
        }
        if unique.iter().any(|m| m == "unknown") {
            return vec!["unknown".into()];
        }
        unique
    }

    fn array(&self, schema: &Value, ind: usize) -> String {
        if let Some(items) = schema["prefixItems"].as_array() {
            let items: Vec<String> = items.iter().map(|s| self.ts_type(s, ind)).collect();
            return format!("[{}]", items.join(", "));
        }
        match schema.get("items") {
            Some(items) => format!("{}[]", parenthesize(self.members(items, ind))),
            None => "unknown[]".into(),
        }
    }

    /// An object literal type, written over several lines with JSDoc per property.
    fn object(&self, schema: &Value, ind: usize) -> String {
        let empty = serde_json::Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        // `additionalProperties`: absent/true = any extra keys, false = none, schema = typed.
        let extra = match schema.get("additionalProperties") {
            None | Some(Value::Bool(true)) => Some("unknown".to_owned()),
            Some(Value::Bool(false)) => None,
            Some(other) => Some(self.ts_type(other, ind + 1)),
        };
        if properties.is_empty() {
            return format!("Record<string, {}>", extra.as_deref().unwrap_or("never"));
        }

        let inner = indent(ind + 1);
        let mut out = String::from("{\n");
        for (key, prop) in properties {
            let mut doc = lines(prop["description"].as_str().unwrap_or_default());
            doc.extend(self.extra_doc(prop));
            out.push_str(&jsdoc(&doc, ind + 1));
            let optional = if required.contains(&key.as_str()) {
                ""
            } else {
                "?"
            };
            out.push_str(&format!(
                "{inner}{}{optional}: {};\n",
                property_name(key),
                self.ts_type(prop, ind + 1)
            ));
        }
        // Only typed extra keys get an index signature; schemars objects without
        // `additionalProperties` are plain structs, so listing their fields is enough.
        if let Some(Value::Object(_)) = schema.get("additionalProperties") {
            out.push_str(&format!(
                "{inner}[key: string]: {};\n",
                extra.unwrap_or_default()
            ));
        }
        out.push_str(&indent(ind));
        out.push('}');
        out
    }

    /// JSDoc beyond the description: one line per documented enum value, `@default`.
    fn extra_doc(&self, schema: &Value) -> Vec<String> {
        let mut doc = Vec::new();
        let mut variants = Vec::new();
        collect_variant_docs(schema, &mut variants);
        if !variants.is_empty() {
            doc.push(String::new());
            doc.extend(variants);
        }
        if let Some(default) = schema.get("default") {
            doc.push(format!("@default {default}"));
        }
        doc
    }
}

/// `- "click": Left click …` for every `{const, description}` variant inside oneOf/anyOf.
fn collect_variant_docs(schema: &Value, out: &mut Vec<String>) {
    for key in ["oneOf", "anyOf"] {
        for variant in schema[key].as_array().into_iter().flatten() {
            match (variant.get("const"), variant["description"].as_str()) {
                (Some(value), Some(desc)) => {
                    out.push(format!(
                        "- `{}`: {}",
                        literal_json(value),
                        desc.replace('\n', " ")
                    ));
                }
                _ => collect_variant_docs(variant, out),
            }
        }
    }
}

fn has_union(schema: &Value) -> bool {
    ["oneOf", "anyOf", "allOf", "enum", "const", "$ref"]
        .iter()
        .any(|k| schema.get(k).is_some())
}

/// `"type": "x"` or `"type": ["x", "null"]` → `["x", "null"]`.
fn type_names(schema: &Value) -> Vec<&str> {
    match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(types) => types.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Union members as one type usable inside `T[]` or `A & B`.
fn parenthesize(members: Vec<String>) -> String {
    if members.len() > 1 {
        format!("({})", members.join(" | "))
    } else {
        members.into_iter().next().unwrap_or_else(|| "never".into())
    }
}

// ---------------------------------------------------------------------------------------------
// Text helpers

fn indent(level: usize) -> String {
    "  ".repeat(level)
}

/// A description as lines (empty description → no lines).
fn lines(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines().map(str::to_owned).collect()
}

/// `/** one line */` or a multi-line `/** … */` block; nothing for no lines.
fn jsdoc(doc: &[String], ind: usize) -> String {
    // Trim blank lines at the ends (e.g. a separator before an empty section).
    let start = doc.iter().position(|l| !l.trim().is_empty());
    let end = doc.iter().rposition(|l| !l.trim().is_empty());
    let (Some(start), Some(end)) = (start, end) else {
        return String::new();
    };
    // `*/` inside a description would end the comment early.
    let doc: Vec<String> = doc[start..=end]
        .iter()
        .map(|l| l.replace("*/", "*\\/"))
        .collect();
    let i = indent(ind);
    if doc.len() == 1 {
        return format!("{i}/** {} */\n", doc[0]);
    }
    let mut out = format!("{i}/**\n");
    for line in &doc {
        if line.is_empty() {
            out.push_str(&format!("{i} *\n"));
        } else {
            out.push_str(&format!("{i} * {line}\n"));
        }
    }
    out.push_str(&format!("{i} */\n"));
    out
}

/// A TS string literal (JSON escaping is valid TypeScript).
fn literal(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// Any JSON value as a TS literal type (`"x"`, `3`, `true`, `null`).
fn literal_json(value: &Value) -> String {
    value.to_string()
}

/// `foo` stays bare; `"content-type"` needs quotes.
fn property_name(key: &str) -> String {
    let mut chars = key.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if valid { key.to_owned() } else { literal(key) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ts(schema: Value) -> String {
        TsGen {
            defs_prefix: "X".into(),
        }
        .ts_type(&schema, 0)
    }

    #[test]
    fn scalars_nullable_and_arrays() {
        assert_eq!(ts(json!({"type": ["integer", "null"]})), "number | null");
        assert_eq!(
            ts(json!({"type": "array", "items": {"type": "string"}})),
            "string[]"
        );
        assert_eq!(
            ts(json!({"type": "array", "items": {"type": ["string", "null"]}})),
            "(string | null)[]"
        );
        assert_eq!(ts(json!({"description": "anything"})), "unknown");
        assert_eq!(ts(json!({"enum": ["a", "b"]})), "\"a\" | \"b\"");
        assert_eq!(ts(json!({"$ref": "#/$defs/step_info"})), "XStepInfo");
    }

    #[test]
    fn unions_of_consts_with_null() {
        let schema = json!({"anyOf": [
            {"oneOf": [{"const": "diff", "description": "D."}, {"const": "none"}]},
            {"type": "null"}
        ]});
        assert_eq!(ts(schema.clone()), "\"diff\" | \"none\" | null");
        let mut docs = Vec::new();
        collect_variant_docs(&schema, &mut docs);
        assert_eq!(docs, ["- `\"diff\"`: D."]);
    }

    #[test]
    fn objects_with_optional_fields_and_jsdoc() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to load."},
                "for": {"type": ["integer", "null"]},
                "bad-name": {"type": "boolean"}
            },
            "required": ["url"],
            "additionalProperties": false
        });
        assert_eq!(
            ts(schema),
            "{\n  \"bad-name\"?: boolean;\n  for?: number | null;\n  /** URL to load. */\n  url: string;\n}"
        );
        assert_eq!(
            ts(json!({"type": "object", "additionalProperties": false})),
            "Record<string, never>"
        );
        assert_eq!(
            ts(json!({"type": "object", "additionalProperties": {"type": "number"}})),
            "Record<string, number>"
        );
    }

    #[test]
    fn names() {
        assert_eq!(pascal("page.act_batch"), "PageActBatch");
        assert_eq!(property_name("tab"), "tab");
        assert_eq!(property_name("2x"), "\"2x\"");
        assert_eq!(jsdoc(&["a */ b".into()], 1), "  /** a *\\/ b */\n");
    }
}
