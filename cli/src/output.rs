//! Terminal output helpers for Draft-native CLI responses.

use draft_core::error::DraftError;

pub fn header(title: &str) {
    println!("\n\x1b[1m{title}\x1b[0m");
}

pub fn field(label: &str, value: &str) {
    println!("  {label:<14} {value}");
}

pub fn success(msg: &str) {
    println!("\x1b[32m✓\x1b[0m {msg}");
}

pub fn warn(msg: &str) {
    println!("\x1b[33m!\x1b[0m {msg}");
}

pub fn format_error(err: &DraftError) -> String {
    let mut s = format!("\x1b[31merror[{}]\x1b[0m: {}", err.code(), err.message);
    if let Some(ctx) = &err.context {
        s.push_str(&format!("\n  context: {ctx}"));
    }
    if let Some(sg) = &err.suggestion {
        s.push_str(&format!("\n  try: {sg}"));
    }
    s
}

pub fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("failed to serialize JSON: {e}"),
    }
}

/// Render any serializable value as human-readable `key value` lines
/// (SRS-FR-130/131: default output is human-readable; JSON only via flags).
pub fn print_human<T: serde::Serialize>(value: &T) {
    match serde_json::to_value(value) {
        Ok(v) => print_human_value(&v, 1),
        Err(e) => eprintln!("failed to render output: {e}"),
    }
}

fn print_human_value(value: &serde_json::Value, depth: usize) {
    use serde_json::Value;
    let pad = "  ".repeat(depth);
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                match v {
                    Value::Object(_) | Value::Array(_) if !human_is_empty(v) => {
                        println!("{pad}{k}:");
                        print_human_value(v, depth + 1);
                    }
                    Value::Object(_) | Value::Array(_) => {}
                    scalar => println!("{pad}{k:<24} {}", human_scalar(scalar)),
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        println!("{pad}- [{i}]");
                        print_human_value(item, depth + 1);
                    }
                    scalar => println!("{pad}- {}", human_scalar(scalar)),
                }
            }
        }
        scalar => println!("{pad}{}", human_scalar(scalar)),
    }
}

fn human_is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.is_empty(),
        serde_json::Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

fn human_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}
