// SPDX-License-Identifier: Apache-2.0
//! A small JSON-Schema subset. The contract is checked by the kernel before any plugin code runs,
//! so every handler can trust the shape of its input.
//!
//! Supported: type (string or list), enum, required, properties, additionalProperties (false, or a
//! schema for the other keys), minLength, maxLength, minimum, maximum, items, minItems, maxItems.

use crate::error::{Error, Result};
use serde_json::Value;

pub fn validate(schema: &Value, value: &Value) -> Result<()> {
    check(schema, value, "")
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_string() } else { format!("{path}.{key}") }
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "number" => v.is_number(),
        // 7.0 is a float even though it is whole; handlers read integers with as_i64.
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        _ => true,
    }
}

fn check(schema: &Value, v: &Value, path: &str) -> Result<()> {
    let Some(obj) = schema.as_object() else { return Ok(()) };
    let name = if path.is_empty() { "input" } else { path };
    if let Some(t) = obj.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
            _ => vec![],
        };
        if !types.is_empty() && !types.iter().any(|t| type_matches(t, v)) {
            return Err(Error::invalid(name, &format!("expected {}", types.join(" or "))));
        }
    }
    if let Some(options) = obj.get("enum").and_then(|e| e.as_array()) {
        if !options.contains(v) {
            let names: Vec<String> = options.iter().map(|o| o.to_string()).collect();
            return Err(Error::invalid(name, &format!("must be one of {}", names.join(", "))));
        }
    }
    match v {
        Value::Object(map) => {
            if let Some(req) = obj.get("required").and_then(|r| r.as_array()) {
                for key in req.iter().filter_map(|k| k.as_str()) {
                    if !map.contains_key(key) {
                        return Err(Error::invalid(&join(path, key), "is required"));
                    }
                }
            }
            let props = obj.get("properties").and_then(|p| p.as_object());
            if obj.get("additionalProperties") == Some(&Value::Bool(false)) {
                if let Some(p) = props {
                    for key in map.keys() {
                        if !p.contains_key(key) {
                            return Err(Error::invalid(&join(path, key), "is not allowed"));
                        }
                    }
                }
            }
            if let Some(extra @ Value::Object(_)) = obj.get("additionalProperties") {
                for (key, child) in map {
                    if !props.is_some_and(|p| p.contains_key(key)) {
                        check(extra, child, &join(path, key))?;
                    }
                }
            }
            if let Some(p) = props {
                for (key, sub) in p {
                    if let Some(child) = map.get(key) {
                        check(sub, child, &join(path, key))?;
                    }
                }
            }
        }
        Value::String(s) => {
            let len = s.chars().count() as u64;
            if let Some(min) = obj.get("minLength").and_then(|m| m.as_u64()) {
                if len < min {
                    return Err(Error::invalid(name, &format!("must be at least {min} characters")));
                }
            }
            if let Some(max) = obj.get("maxLength").and_then(|m| m.as_u64()) {
                if len > max {
                    return Err(Error::invalid(name, &format!("must be at most {max} characters")));
                }
            }
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if let Some(min) = obj.get("minimum").and_then(|m| m.as_f64()) {
                if f < min {
                    return Err(Error::invalid(name, &format!("must be at least {min}")));
                }
            }
            if let Some(max) = obj.get("maximum").and_then(|m| m.as_f64()) {
                if f > max {
                    return Err(Error::invalid(name, &format!("must be at most {max}")));
                }
            }
        }
        Value::Array(items) => {
            if let Some(min) = obj.get("minItems").and_then(|m| m.as_u64()) {
                if (items.len() as u64) < min {
                    return Err(Error::invalid(name, &format!("needs at least {min} items")));
                }
            }
            if let Some(max) = obj.get("maxItems").and_then(|m| m.as_u64()) {
                if (items.len() as u64) > max {
                    return Err(Error::invalid(name, &format!("allows at most {max} items")));
                }
            }
            if let Some(sub) = obj.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(sub, item, &format!("{name}[{i}]"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_and_additional_properties() {
        let s = json!({"type":"object","required":["a"],"properties":{"a":{"type":"string","minLength":2}},"additionalProperties":false});
        assert!(validate(&s, &json!({"a":"ok"})).is_ok());
        assert_eq!(validate(&s, &json!({})).unwrap_err().params["field"], "a");
        assert!(validate(&s, &json!({"a":"x"})).is_err());
        assert!(validate(&s, &json!({"a":"ok","b":1})).is_err());
    }

    #[test]
    fn integer_bounds_and_union_types() {
        let s = json!({"type":"object","properties":{"p":{"type":"integer","minimum":0,"maximum":10},"d":{"type":["string","null"]}}});
        assert!(validate(&s, &json!({"p":10,"d":null})).is_ok());
        assert!(validate(&s, &json!({"p":11})).is_err());
        assert!(validate(&s, &json!({"p":1.5})).is_err());
        assert!(validate(&s, &json!({"p":7.0})).is_err(), "7.0 is a float, not an integer");
        assert!(validate(&s, &json!({"d":3})).is_err());
    }
}
