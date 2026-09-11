//! A small, total glob matcher over opaque strings.
//!
//! Draft matches patterns against locator bodies, attribute values and — for
//! filesystem resources only — paths. The matcher is deliberately generic: it
//! knows `*`, `**` and `?`, and it knows that `/` separates segments, but it
//! attaches no meaning to what the segments *are*. Nothing here resolves a
//! parent, follows a link or touches a filesystem.
//!
//! `*` matches within one segment, `**` spans segments, and `?` matches one
//! character. Matching is byte-oriented and case-sensitive, so a pattern either
//! matches a value or it does not, on every platform.

/// Whether `value` matches `pattern`.
pub fn matches(pattern: &str, value: &str) -> bool {
    match_from(pattern.as_bytes(), value.as_bytes())
}

/// Recursive descent with backtracking only where a wildcard demands it.
///
/// The pattern is consumed left to right; the only branch point is `*`/`**`,
/// and each one tries successively longer matches. Patterns are short and
/// authored by hand, so the simple form is both fast enough and easy to audit.
fn match_from(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    // A trailing `/**` matches the value itself as well as everything under it:
    // `src/**` covers `src`, which is what anyone writing that pattern means.
    if value.is_empty() && pattern == b"/**" {
        return true;
    }
    match pattern[0] {
        b'*' => {
            // `**` crosses separators; a single `*` stops at one.
            let (rest, crosses_separators) = if pattern.len() > 1 && pattern[1] == b'*' {
                let mut rest = &pattern[2..];
                // `**/` also matches zero segments, so `a/**/b` matches `a/b`.
                if rest.first() == Some(&b'/') && match_from(&rest[1..], value) {
                    return true;
                }
                if rest.first() == Some(&b'/') {
                    rest = &rest[1..];
                }
                (rest, true)
            } else {
                (&pattern[1..], false)
            };
            for index in 0..=value.len() {
                if !crosses_separators && value[..index].contains(&b'/') {
                    break;
                }
                if match_from(rest, &value[index..]) {
                    return true;
                }
            }
            false
        }
        b'?' => !value.is_empty() && value[0] != b'/' && match_from(&pattern[1..], &value[1..]),
        literal => {
            !value.is_empty() && value[0] == literal && match_from(&pattern[1..], &value[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn literals_and_single_wildcards_stay_inside_one_segment() {
        assert!(matches("notes.txt", "notes.txt"));
        assert!(!matches("notes.txt", "notes.txtx"));
        assert!(matches("*.txt", "notes.txt"));
        // A single `*` must not swallow a separator, or `*.txt` would match
        // anything anywhere that happens to end in `.txt`.
        assert!(!matches("*.txt", "deep/notes.txt"));
        assert!(matches("src/*.rs", "src/main.rs"));
        assert!(!matches("src/*.rs", "src/inner/main.rs"));
    }

    #[test]
    fn double_star_crosses_separators_and_may_match_nothing() {
        assert!(matches("**/notes.txt", "notes.txt"));
        assert!(matches("**/notes.txt", "a/b/notes.txt"));
        assert!(matches("src/**", "src"));
        assert!(matches("src/**", "src/a/b.rs"));
        assert!(matches("a/**/b", "a/b"));
        assert!(matches("a/**/b", "a/x/y/b"));
        assert!(!matches("a/**/b", "a/x/y/c"));
    }

    #[test]
    fn question_mark_matches_one_non_separator_character() {
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(!matches("a?c", "a/c"));
    }

    #[test]
    fn matching_is_case_sensitive_and_platform_independent() {
        assert!(!matches("*.TXT", "notes.txt"));
        assert!(!matches("*.txt", "notes.TXT"));
        // Backslashes are ordinary characters: this matcher never treats them
        // as separators, so a pattern means the same thing everywhere.
        assert!(matches("a\\b", "a\\b"));
        assert!(!matches("a/b", "a\\b"));
    }

    #[test]
    fn a_pattern_is_anchored_at_both_ends() {
        assert!(!matches("notes", "notes.txt"));
        assert!(!matches("otes.txt", "notes.txt"));
        assert!(matches("**", "anything/at/all"));
        assert!(matches("", ""));
        assert!(!matches("", "x"));
    }

    #[test]
    fn adjacent_wildcards_terminate() {
        // Pathological patterns must still be total rather than looping.
        assert!(matches("***", "abc"));
        assert!(matches("*/*", "a/b"));
        assert!(!matches("*/*", "a"));
    }
}
