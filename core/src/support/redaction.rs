//! Conservative secret redaction shared by logs, Doctor, and Console surfaces.

use serde_json::Value;

pub fn redact(input: &str) -> String {
    let input = redact_pem_blocks(input);
    input
        .lines()
        .map(redact_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn redact_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(redact(&value)),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if sensitive_key_token(&key.to_ascii_lowercase()) {
                        (key, Value::String("[REDACTED]".to_string()))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

fn redact_secret_line(line: &str) -> String {
    let mut out = Vec::new();
    let mut redact_next = false;
    for token in line.split_whitespace() {
        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if looks_like_standalone_secret(token) {
            out.push("[REDACTED]".to_string());
        } else if let Some(redacted) = redact_assignment_token(token) {
            out.push(redacted);
        } else if sensitive_key_token(&lower) || lower == "bearer" {
            out.push(redact_key_token(token));
            redact_next = true;
        } else {
            out.push(redact_url_credentials(token));
        }
    }
    out.join(" ")
}

fn redact_assignment_token(token: &str) -> Option<String> {
    for separator in ['=', ':'] {
        if let Some((key, _)) = token.split_once(separator) {
            if sensitive_key_token(&key.to_ascii_lowercase()) {
                return Some(format!("{key}{separator}[REDACTED]"));
            }
        }
    }
    None
}

fn redact_key_token(token: &str) -> String {
    let trimmed = token.trim_end_matches([':', '=']);
    if trimmed.len() != token.len() {
        format!("{trimmed}:[REDACTED]")
    } else {
        "[REDACTED]".to_string()
    }
}

fn sensitive_key_token(lower: &str) -> bool {
    [
        "password",
        "passwd",
        "pwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
        "authorization",
    ]
    .iter()
    .any(|key| lower.contains(key))
}

fn looks_like_standalone_secret(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && character != '.' && character != '_'
    });
    (trimmed.starts_with("eyJ") && trimmed.matches('.').count() >= 2)
        || trimmed.starts_with("AKIA")
        || trimmed.starts_with("ASIA")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("xoxb-")
        || trimmed.starts_with("sk-")
        || trimmed.starts_with("github_pat_")
}

fn redact_url_credentials(token: &str) -> String {
    let Some(scheme_index) = token.find("://") else {
        return token.to_string();
    };
    let authority_start = scheme_index + 3;
    let authority_end = token[authority_start..]
        .find('/')
        .map(|index| authority_start + index)
        .unwrap_or(token.len());
    let Some(at_offset) = token[authority_start..authority_end].find('@') else {
        return token.to_string();
    };
    let at = authority_start + at_offset;
    format!("{}[REDACTED]{}", &token[..authority_start], &token[at..])
}

fn redact_pem_blocks(input: &str) -> String {
    let mut out = Vec::new();
    let mut in_pem = false;
    for line in input.lines() {
        if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----") {
            out.push("[REDACTED PEM PRIVATE KEY]".to_string());
            in_pem = true;
            continue;
        }
        if in_pem {
            if line.contains("-----END ") && line.contains("PRIVATE KEY-----") {
                in_pem = false;
            }
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_secrets() {
        let redacted = redact(
            "API_TOKEN=abc\nghp_abcdefghijklmnopqrstuvwxyz\nhttps://user:password@example.test/x",
        );
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("ghp_"));
        assert!(!redacted.contains("user:password"));
    }

    #[test]
    fn redacts_nested_values_by_key_and_content() {
        let redacted = redact_value(serde_json::json!({
            "nested": { "access_token": "raw" },
            "message": "password=hunter2"
        }));
        assert_eq!(redacted["nested"]["access_token"], "[REDACTED]");
        assert_eq!(redacted["message"], "password=[REDACTED]");
    }
}
