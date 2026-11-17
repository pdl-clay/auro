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
/// Every argument is required. Per-argument `$defs`/`definitions` are hoisted to the root so
/// `#/$defs/...` references resolve; colliding definition names are last-wins in this slice.
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
        properties.insert(name.to_owned(), schema);
        required.push(Value::String(name.to_owned()));
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
    fn omits_required_and_defs_when_there_are_no_arguments() {
        let schema = input_schema([]);
        assert_eq!(schema["properties"], json!({}));
        assert!(schema.get("required").is_none());
        assert!(schema.get("$defs").is_none());
    }
}
