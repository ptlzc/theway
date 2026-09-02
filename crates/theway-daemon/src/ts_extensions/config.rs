//! Plugin config (issue #83 §6): manifest `configSchema` validation with
//! default filling and precedence merging (schema default < instance config <
//! session override). The validator reuses the extension payload-schema
//! subset (`dispatcher::matches_schema`) plus `default` filling for object
//! properties, so no new JSON-Schema dependency enters the daemon.

use serde_json::Value;

use super::dispatcher;

/// Validate `value` against `schema` and fill missing object properties with
/// their schema `default`. Returns the merged configuration, or a descriptive
/// error when the value violates the schema (fail loud: invalid config rejects
/// the plugin at load).
pub(super) fn validate_and_default(schema: &Value, value: Value) -> Result<Value, String> {
    if !schema.is_object() {
        return Err("manifest configSchema must be a JSON Schema object".into());
    }
    if !value.is_object() {
        return Err("plugin config must be a JSON object".into());
    }
    let mut merged = value;
    fill_defaults(schema, &mut merged)?;
    if !dispatcher::matches_schema(schema, &merged) {
        return Err("plugin config does not match the manifest configSchema".into());
    }
    Ok(merged)
}

/// Recursively fill `default` values from `schema.properties` into `config`
/// where a key is absent. Unknown keys pass through (the schema subset used by
/// the runtime does not reject them unless `additionalProperties: false`).
fn fill_defaults(schema: &Value, config: &mut Value) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(object) = config.as_object_mut() else {
        return Ok(());
    };
    for (key, property_schema) in properties {
        if object.contains_key(key) {
            continue;
        }
        if let Some(default) = property_schema.get("default") {
            object.insert(key.clone(), default.clone());
        } else if property_schema
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "object")
        {
            // Nested objects: walk into an empty object so their own defaults
            // fill recursively.
            object.insert(key.clone(), Value::Object(serde_json::Map::new()));
            if let Some(nested) = object.get_mut(key) {
                fill_defaults(property_schema, nested)?;
            }
        }
    }
    Ok(())
}

/// Precedence merge: `session_override` wins over `instance`, which wins over
/// schema defaults (already filled by [`validate_and_default`]).
pub(super) fn merge(instance: Value, session_override: Value) -> Value {
    match (instance, session_override) {
        (Value::Object(mut base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                base.insert(key, value);
            }
            Value::Object(base)
        }
        (base, Value::Null) => base,
        (_, override_value) => override_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fills_defaults_and_passes_validation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "greeting": {"type": "string", "default": "Hello"},
                "maxRetries": {"type": "number", "default": 3},
            },
        });
        let merged = validate_and_default(&schema, json!({})).unwrap();
        assert_eq!(merged, json!({"greeting": "Hello", "maxRetries": 3}));
    }

    #[test]
    fn rejects_config_that_violates_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"apiKey": {"type": "string"}},
        });
        assert!(validate_and_default(&schema, json!({"apiKey": 42})).is_err());
    }

    #[test]
    fn merge_prefers_session_override() {
        let merged = merge(json!({"a": 1, "b": 2}), json!({"b": 3, "c": 4}));
        assert_eq!(merged, json!({"a": 1, "b": 3, "c": 4}));
    }

    #[test]
    fn nested_object_defaults_fill() {
        let schema = json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "object",
                    "properties": {"port": {"type": "number", "default": 8080}},
                }
            },
        });
        let merged = validate_and_default(&schema, json!({})).unwrap();
        assert_eq!(merged, json!({"server": {"port": 8080}}));
    }
}
