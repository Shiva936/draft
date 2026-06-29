//! Terminal output helpers. Provider-neutral language throughout (FR-CLI-004).

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
