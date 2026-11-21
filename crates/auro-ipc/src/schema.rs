// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command metadata: the input/output JSON Schema stored by a `CommandHandler`.

use serde::Serialize;
use serde_json::{Map, Value};

/// The schema advertised for one command.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandSchema {
    pub name: &'static str,
    pub input: Value,
    pub output: Value,
}

/// Builds a command input schema from named argument schemas.
///
/// Arguments whose schema accepts `null` (the shape `schemars` emits for `Option<T>`) are optional
/// and omitted from `required`; every other argument is required. Per-argument
/// `$defs`/`definitions` are hoisted to the root so `#/$defs/...` references resolve; colliding
/// definition names are last-wins in this slice.
pub fn input_schema(args: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut defs = Map::new();
    let mut legacy_defs = Map::new();

    for (name, mut schema) in args {
        if let Value::Object(map) = &mut schema {
            if let Some(Value::Object(found)) = map.remove("$defs") {
                defs.extend(found);
            }
            if let Some(Value::Object(found)) = map.remove("definitions") {
                legacy_defs.extend(found);
            }
        }
        let optional = accepts_null(&schema);
        properties.insert(name.to_owned(), schema);
        if !optional {
            required.push(Value::String(name.to_owned()));
        }
    }

    let mut root = Map::new();
    root.insert("type".to_owned(), Value::String("object".to_owned()));
    root.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        root.insert("required".to_owned(), Value::Array(required));
    }
    if !defs.is_empty() {
        root.insert("$defs".to_owned(), Value::Object(defs));
    }
    if !legacy_defs.is_empty() {
        root.insert("definitions".to_owned(), Value::Object(legacy_defs));
    }
    Value::Object(root)
}

/// Whether `serde_json` can deserialize `null` into this type.
///
/// Covers `Option<T>` (a null type or an `anyOf` variant) and unconstrained schemas such as
/// `serde_json::Value`, whose schema has no `type` or composition keyword. A `$ref`,
/// `allOf`/`oneOf`, `const` or a non-null `enum` is treated as constrained and therefore required.
fn accepts_null(schema: &Value) -> bool {
    let Value::Object(map) = schema else {
        return matches!(schema, Value::Bool(true));
    };
    if let Some(kind) = map.get("type") {
        return match kind {
            Value::String(kind) => kind == "null",
            Value::Array(kinds) => kinds.iter().any(|kind| kind == "null"),
            _ => false,
        };
    }
    if let Some(variants) = map.get("anyOf").and_then(Value::as_array) {
        return variants.iter().any(accepts_null);
    }
    if let Some(constant) = map.get("const") {
        return constant.is_null();
    }
    if let Some(values) = map.get("enum").and_then(Value::as_array) {
        return values.iter().any(Value::is_null);
    }
    !map.contains_key("$ref")
        && !map.contains_key("allOf")
        && !map.contains_key("oneOf")
        && !map.contains_key("not")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hoists_definitions_and_marks_every_argument_required() {
        let arg = json!({
            "type": "object",
            "$defs": { "Name": { "type": "string" } },
            "properties": { "name": { "$ref": "#/$defs/Name" } }
        });

        let schema = input_schema([("user", arg)]);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["user"]));
        assert_eq!(schema["$defs"]["Name"]["type"], "string");
        assert_eq!(schema["properties"]["user"]["$defs"], Value::Null);
    }

    #[test]
    fn a_nullable_argument_is_not_required() {
        let optional = json!({ "type": ["integer", "null"] });
        let schema = input_schema([("value", optional), ("other", json!({ "type": "string" }))]);
        assert_eq!(schema["required"], json!(["other"]));
    }

    #[test]
    fn an_unconstrained_argument_is_not_required() {
        let schema = input_schema([
            ("value", json!({ "title": "AnyValue" })),
            ("referenced", json!({ "$ref": "#/$defs/Name" })),
        ]);
        assert_eq!(schema["required"], json!(["referenced"]));
    }

    #[test]
    fn omits_required_and_defs_when_there_are_no_arguments() {
        let schema = input_schema([]);
        assert_eq!(schema["properties"], json!({}));
        assert!(schema.get("required").is_none());
        assert!(schema.get("$defs").is_none());
    }
}
