//! Conservative secret redaction shared by logs, Doctor, and Console surfaces.

pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut private = false;
    for line in input.lines() {
        if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----") {
            private = true;
            out.push_str("[REDACTED PRIVATE KEY]\n");
            continue;
        }
        if private {
            if line.contains("-----END ") && line.contains("PRIVATE KEY-----") {
                private = false;
            }
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = line.find('=') {
            let key = &lower[..pos];
            if [
                "token",
                "secret",
                "password",
                "passwd",
                "api_key",
                "private_key",
                "authorization",
            ]
            .iter()
            .any(|k| key.contains(k))
            {
                out.push_str(&line[..pos + 1]);
                out.push_str("[REDACTED]\n");
                continue;
            }
        }
        let words = line
            .split_whitespace()
            .map(|w| if looks_like_token(w) { "[REDACTED]" } else { w })
            .collect::<Vec<_>>();
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    if !input.ends_with('\n') {
        out.pop();
    }
    out
}
fn looks_like_token(s: &str) -> bool {
    let trimmed = s.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
    (trimmed.starts_with("ghp_")
        || trimmed.starts_with("github_pat_")
        || trimmed.starts_with("sk-")
        || trimmed.starts_with("Bearer"))
        && trimmed.len() > 12
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_common_secrets() {
        assert!(!redact("API_TOKEN=abc\nghp_abcdefghijklmnopqrstuvwxyz").contains("abc"));
    }
}
