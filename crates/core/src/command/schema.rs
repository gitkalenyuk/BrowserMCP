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
    strip_numeric_formats(&mut value);
    value
}

/// Remove schemars' Rust-specific number formats (`uint64`, `int32`, `uint`, `double`, ...).
/// They are not standard JSON Schema formats: MCP clients print a warning for every one of
/// them (Claude Code: `unknown format "uint64" ignored`) and they cost tokens. The `type`
/// (`integer`/`number`) and `minimum` that schemars emits alongside already say enough.
fn strip_numeric_formats(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let is_number = matches!(
                map.get("type"),
                Some(Value::String(t)) if t == "integer" || t == "number"
            ) || matches!(
                map.get("type"),
                Some(Value::Array(types)) if types.iter().any(|t| t == "integer" || t == "number")
            );
            if is_number {
                map.remove("format");
            }
            for child in map.values_mut() {
                strip_numeric_formats(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_numeric_formats),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_formats_are_removed_but_string_formats_kept() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "tab": {"type": ["integer", "null"], "format": "int32"},
                "limit": {"type": "integer", "format": "uint", "minimum": 0},
                "when": {"type": "string", "format": "date-time"},
                "list": {"type": "array", "items": {"type": "number", "format": "double"}}
            }
        });
        strip_numeric_formats(&mut schema);
        let text = schema.to_string();
        for gone in ["int32", "\"uint\"", "double"] {
            assert!(!text.contains(gone), "{gone} still in {text}");
        }
        assert_eq!(schema["properties"]["when"]["format"], "date-time");
        assert_eq!(schema["properties"]["limit"]["minimum"], 0);
    }
}
