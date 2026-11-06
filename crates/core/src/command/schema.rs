//! JSON Schema generation for command arguments and outputs.
//!
//! We use schemars' generator directly instead of `schemars::schema_for!` because we need two
//! non-default settings that MCP clients care about:
//! * `inline_subschemas`: nested types are written in place instead of `$defs` + `$ref`
//!   (several MCP clients and the Gemini API do not resolve `$ref`);
//! * no `$schema` / root `title` keys: they cost tokens in every `tools/list` and add nothing.
//!
//! Draft 2020-12 is the dialect the MCP spec asks for.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::Value;

/// Schema of what the command *accepts* (deserialize contract).
pub fn args_schema<T: JsonSchema>() -> Value {
    generate::<T>(SchemaSettings::draft2020_12().for_deserialize())
}

/// Schema of what the command *returns* (serialize contract: e.g. `Option` fields are always
/// present unless `skip_serializing_if` is used).
pub fn output_schema<T: JsonSchema>() -> Value {
    generate::<T>(SchemaSettings::draft2020_12().for_serialize())
}

fn generate<T: JsonSchema>(settings: SchemaSettings) -> Value {
    let settings = settings.with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    });
    let mut value = settings
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value();
    if let Some(root) = value.as_object_mut() {
        root.remove("title");
    }
    value
}
