//! Canonical JSON, as specified by `proto/specs/canonicalization.md`.
//!
//! Object keys sort recursively, arrays keep their order, separators are
//! compact, and structurally equal values always produce byte-identical output.
//! Catalog signatures are taken over this form, so an independent
//! implementation is exactly what a publisher outside the Draft repository
//! needs. `draft-core` carries a conformance test asserting this implementation
//! agrees with its own, byte for byte.

use serde_json::Value;

/// Serialize `value` in canonical form.
pub fn canonical_json(value: &Value) -> String {
    let mut rendered = String::new();
    write_canonical(value, &mut rendered);
    rendered
}

/// Canonical bytes for any serializable document — the message a catalog role
/// signature is computed over.
pub fn canonical_bytes<T: serde::Serialize>(value: &T) -> crate::FormatResult<Vec<u8>> {
    let value = serde_json::to_value(value)
        .map_err(|error| crate::FormatError::Encoding(format!("canonical encoding: {error}")))?;
    Ok(canonical_json(&value).into_bytes())
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_json_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn write_json_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32))
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_recursively_and_arrays_keep_order() {
        let value = json!({"b": 1, "a": {"d": [3, 1, 2], "c": true}});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":true,"d":[3,1,2]},"b":1}"#
        );
    }

    #[test]
    fn escapes_and_control_characters_are_stable() {
        let raw = "a\"b\\c\nd\te\r\u{1}";
        let value = json!({ "k": raw });
        assert_eq!(canonical_json(&value), r#"{"k":"a\"b\\c\nd\te\r\u0001"}"#);
    }

    #[test]
    fn structurally_equal_values_are_byte_identical() {
        let left: Value = serde_json::from_str(r#"{"x":1,"y":[{"b":2,"a":1}]}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"y":[{"a":1,"b":2}],"x":1}"#).unwrap();
        assert_eq!(canonical_json(&left), canonical_json(&right));
    }
}
