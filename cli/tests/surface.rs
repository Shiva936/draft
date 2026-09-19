//! Frozen inventory of the externally observable CLI surface.
//!
//! Every command, subcommand, argument and flag reachable through `--help` is
//! walked recursively and rendered into a stable listing that is compared
//! against a committed golden.
//!
//! The rule this enforces is *exactness*, not monotonic growth. Draft has one
//! way to spell each command: a renamed command's old spelling is removed
//! rather than kept as an alias, so "a command disappeared" is a legitimate
//! diff and cannot be the failure condition. What the golden defends is that
//! the surface only ever changes *deliberately* — the diff is the product
//! change, reviewed as one.
//!
//! Regenerate with `DRAFT_UPDATE_SURFACE_GOLDEN=1 cargo test -p draft-cli
//! --test surface`, and read the diff before committing it.

use assert_cmd::Command as Assert;

const GOLDEN: &str = "tests/golden/cli-surface.txt";

fn help(path: &[String]) -> String {
    let mut command = Assert::cargo_bin("draft").unwrap();
    command.args(path).arg("--help");
    let output = command.output().expect("draft --help must run");
    assert!(
        output.status.success(),
        "`draft {} --help` failed: {}",
        path.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("help output is UTF-8")
}

/// Split clap help into `(section name, lines)` pairs.
fn sections(help: &str) -> Vec<(String, Vec<String>)> {
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for line in help.lines() {
        if !line.starts_with(char::is_whitespace) && line.trim_end().ends_with(':') {
            let name = line.trim_end().trim_end_matches(':').to_string();
            sections.push((name, Vec::new()));
        } else if let Some(current) = sections.last_mut() {
            current.1.push(line.to_string());
        }
    }
    sections
}

/// The leading declaration of an entry line, before clap's description gutter.
fn declaration(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let declaration = trimmed.split("  ").next().unwrap_or(trimmed).trim_end();
    if declaration.is_empty() {
        None
    } else {
        Some(declaration)
    }
}

fn section_lines<'a>(sections: &'a [(String, Vec<String>)], name: &str) -> &'a [String] {
    sections
        .iter()
        .find(|(section, _)| section == name)
        .map(|(_, lines)| lines.as_slice())
        .unwrap_or(&[])
}

fn usage(help: &str) -> String {
    for (index, line) in help.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("Usage:") {
            let mut usage = rest.trim().to_string();
            // clap wraps long usage lines onto continuation lines.
            for continuation in help.lines().skip(index + 1) {
                if continuation.trim().is_empty() || !continuation.starts_with("       ") {
                    break;
                }
                usage.push(' ');
                usage.push_str(continuation.trim());
            }
            return usage;
        }
    }
    String::new()
}

fn arguments(sections: &[(String, Vec<String>)]) -> Vec<String> {
    section_lines(sections, "Arguments")
        .iter()
        .filter_map(|line| declaration(line))
        .filter(|declaration| declaration.starts_with('<') || declaration.starts_with('['))
        .map(str::to_string)
        .collect()
}

fn flags(sections: &[(String, Vec<String>)]) -> Vec<String> {
    let mut flags: Vec<String> = section_lines(sections, "Options")
        .iter()
        .filter_map(|line| declaration(line))
        .filter(|declaration| declaration.starts_with('-'))
        .flat_map(|declaration| {
            declaration
                .split([',', ' '])
                .filter(|token| token.starts_with('-'))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

fn subcommands(sections: &[(String, Vec<String>)]) -> Vec<String> {
    section_lines(sections, "Commands")
        .iter()
        .filter_map(|line| declaration(line))
        .filter(|declaration| declaration.starts_with(|first: char| first.is_ascii_alphanumeric()))
        // clap renders aliases as `name, alias`; the canonical name comes first.
        .filter_map(|declaration| declaration.split(',').next())
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "help")
        .map(str::to_string)
        .collect()
}

fn walk(path: &mut Vec<String>, rendered: &mut Vec<String>) {
    let help = help(path);
    let sections = sections(&help);
    let label = if path.is_empty() {
        "draft".to_string()
    } else {
        format!("draft {}", path.join(" "))
    };

    rendered.push(label);
    rendered.push(format!("  usage: {}", usage(&help)));
    for argument in arguments(&sections) {
        rendered.push(format!("  arg: {argument}"));
    }
    for flag in flags(&sections) {
        rendered.push(format!("  flag: {flag}"));
    }
    rendered.push(String::new());

    for subcommand in subcommands(&sections) {
        path.push(subcommand);
        walk(path, rendered);
        path.pop();
    }
}

fn render_surface() -> String {
    let mut rendered = Vec::new();
    walk(&mut Vec::new(), &mut rendered);
    rendered.join("\n")
}

/// Exit codes are part of the contract for scripted use, so freeze the mapping
/// alongside the command surface rather than trusting prose.
fn render_exit_codes() -> String {
    let source = std::fs::read_to_string("src/main.rs").expect("cli source is readable");
    let mut rendered = vec!["exit codes".to_string()];
    let mut inside = false;
    for line in source.lines() {
        if line.contains("fn main() -> ExitCode") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with("fn ") {
            break;
        }
        let trimmed = line.trim();
        // Arms may span several lines, so keep every line that names either a
        // mapped error kind or the code it maps to.
        if trimmed.contains("ExitCode::") || trimmed.contains("DraftErrorKind::") {
            rendered.push(format!("  {trimmed}"));
        }
    }
    rendered.join("\n")
}

#[test]
fn cli_surface_matches_the_frozen_inventory() {
    let observed = format!("{}\n{}\n", render_surface(), render_exit_codes());

    // Only an explicit opt-in regenerates; a stray "0" must not silently
    // rewrite the baseline this test exists to defend.
    if matches!(
        std::env::var("DRAFT_UPDATE_SURFACE_GOLDEN").as_deref(),
        Ok("1") | Ok("true")
    ) {
        std::fs::write(GOLDEN, &observed).expect("golden is writable");
        return;
    }

    let expected = std::fs::read_to_string(GOLDEN).unwrap_or_else(|error| {
        panic!(
            "missing {GOLDEN}: {error}. Regenerate with \
             DRAFT_UPDATE_SURFACE_GOLDEN=1 cargo test -p draft-cli --test surface"
        )
    });

    if observed != expected {
        let observed_lines: Vec<&str> = observed.lines().collect();
        let expected_lines: Vec<&str> = expected.lines().collect();
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for line in &expected_lines {
            if !observed_lines.contains(line) {
                removed.push(*line);
            }
        }
        for line in &observed_lines {
            if !expected_lines.contains(line) {
                added.push(*line);
            }
        }
        panic!(
            "CLI surface drifted from {GOLDEN}.\nremoved:\n{}\nadded:\n{}\n\
             Neither is a regression by itself — a rename removes the old spelling \
             and adds the new one. What must not happen is either arriving by \
             accident. Read the diff, then regenerate with \
             DRAFT_UPDATE_SURFACE_GOLDEN=1 once it is what you meant.",
            removed.join("\n"),
            added.join("\n")
        );
    }
}
