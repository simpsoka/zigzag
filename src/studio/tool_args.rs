//! Cached validation of the schemas advertised by tools/list.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

pub fn validate(name: &str, args: &Value, specs: &Value) -> Result<(), String> {
    static VALIDATORS: OnceLock<Result<HashMap<String, jsonschema::Validator>, String>> =
        OnceLock::new();
    let validators = VALIDATORS
        .get_or_init(|| {
            specs
                .as_array()
                .ok_or("tool catalog must be an array")?
                .iter()
                .map(|spec| {
                    let name = spec["name"].as_str().ok_or("tool must have a name")?;
                    let validator = jsonschema::validator_for(&spec["inputSchema"])
                        .map_err(|error| format!("invalid schema for {name}: {error}"))?;
                    Ok((name.to_string(), validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let Some(validator) = validators.get(name) else {
        return Ok(());
    };
    validator.validate(args).map_err(|error| {
        let received = match error.instance().as_ref() {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let constraint = error.schema_path().to_string();
        let expected = specs
            .as_array()
            .and_then(|tools| tools.iter().find(|t| t["name"] == name))
            .and_then(|spec| spec["inputSchema"].pointer(&constraint))
            .map(Value::to_string)
            .unwrap_or_else(|| constraint.clone());
        // Describe the value class, not arbitrary payloads or file contents.
        let expected: String = expected.chars().take(240).collect();
        format!(
            "{name}: arguments{}: received {received}; expected {constraint} = {expected}",
            error.instance_path()
        )
    })
}
