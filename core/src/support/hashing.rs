//! Deterministic hashing primitives.
//!
//! See `proto/specs/canonicalization.md`.
//!
//! Two things live here:
//! - [`canonical_json`] / [`sha256_hex`] — the canonical serialization used to
//!   hash events, receipts, and transparency entries so a hash is stable across
//!   machines and serde versions (object keys sorted, no insignificant
//!   whitespace).
//!
//! Workspace/source hashing lives in `dcg::source_view`.
use sha2::{Digest, Sha256};

/// Hex-encode a SHA-256 digest of `bytes`, prefixed `sha256:`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(7 + 64);
    s.push_str("sha256:");
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Hash length-framed fields under a stable domain separator.
pub fn domain_hash<'a>(domain: &str, fields: impl IntoIterator<Item = &'a [u8]>) -> String {
    let fields = fields.into_iter().collect::<Vec<_>>();
    let mut framed = Vec::new();
    framed.extend_from_slice(&(domain.len() as u64).to_be_bytes());
    framed.extend_from_slice(domain.as_bytes());
    framed.extend_from_slice(&(fields.len() as u64).to_be_bytes());
    for field in fields {
        framed.extend_from_slice(&(field.len() as u64).to_be_bytes());
        framed.extend_from_slice(field);
    }
    sha256_hex(&framed)
}

/// Serialize a JSON value canonically: object keys sorted recursively, arrays
/// preserved in order, compact separators. Two structurally equal values always
/// produce byte-identical output.
pub fn canonical_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

/// Convenience: canonicalize `value` then hash it.
pub fn canonical_hash<T: serde::Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value)
        .expect("canonical contract values must be representable as JSON");
    sha256_hex(canonical_json(&v).as_bytes())
}

/// Fallible canonical hashing for persistence and wire boundaries.
pub fn try_canonical_hash<T: serde::Serialize>(
    value: &T,
) -> crate::support::error::DraftResult<String> {
    let value = serde_json::to_value(value).map_err(|error| {
        crate::support::error::DraftError::storage(format!(
            "canonical JSON serialization failed: {error}"
        ))
    })?;
    Ok(sha256_hex(canonical_json(&value).as_bytes()))
}

pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn hex_decode(input: &str) -> crate::support::error::DraftResult<Vec<u8>> {
    use crate::support::error::DraftError;

    if !input.len().is_multiple_of(2) {
        return Err(DraftError::storage("invalid hex length"));
    }
    let mut output = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks_exact(2) {
        let text =
            std::str::from_utf8(pair).map_err(|_| DraftError::storage("invalid hex byte"))?;
        output.push(
            u8::from_str_radix(text, 16).map_err(|_| DraftError::storage("invalid hex byte"))?,
        );
    }
    Ok(output)
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_json_string(s, out),
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys() {
        let a = json!({"b": 1, "a": 2, "nested": {"z": 1, "y": 2}});
        assert_eq!(
            canonical_json(&a),
            r#"{"a":2,"b":1,"nested":{"y":2,"z":1}}"#
        );
    }

    #[test]
    fn canonical_json_is_order_independent() {
        let a = json!({"x": 1, "y": [1, 2, 3]});
        let b = json!({"y": [1, 2, 3], "x": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }
}
