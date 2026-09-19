//! The examples are executable documentation, so they are executed.
//!
//! Every local example runs against the built binary in an isolated home;
//! gated examples are reported as skipped rather than silently passing; every
//! example directory must be classified; and no example may invoke a `draft`
//! command path the CLI no longer has.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Class {
    /// Needs only the binary.
    Local,
    /// Needs `DRAFT_EXAMPLE_PROVIDER_DIR`.
    Provider,
    /// Needs `DRAFT_EXAMPLE_EXTENSIONS`.
    Extension,
}

const EXAMPLES: &[(&str, Class)] = &[
    ("basic", Class::Local),
    ("multi-resource", Class::Local),
    ("verification", Class::Local),
    ("review", Class::Local),
    ("composition", Class::Local),
    ("promotion", Class::Local),
    ("recovery", Class::Local),
    ("end-to-end", Class::Local),
    ("publication", Class::Provider),
    ("providers", Class::Provider),
    ("extensions", Class::Extension),
];

/// Not an example: annotated reference material.
const NOT_EXAMPLES: &[&str] = &["reference"];

const TIMEOUT: Duration = Duration::from_secs(600);

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

#[test]
fn every_example_is_classified() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(examples_dir())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .filter(|name| !NOT_EXAMPLES.contains(&name.as_str()))
        .collect();
    let classified: BTreeSet<String> = EXAMPLES.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(
        on_disk, classified,
        "every example directory needs exactly one class"
    );
    for (name, _) in EXAMPLES {
        for file in ["run.sh", "README.md"] {
            assert!(
                examples_dir().join(name).join(file).is_file(),
                "examples/{name}/{file} is missing"
            );
        }
    }
    let index = std::fs::read_to_string(examples_dir().join("README.md")).unwrap();
    for (name, _) in EXAMPLES {
        assert!(
            index.contains(&format!("({name}/README.md)")),
            "examples/README.md does not list {name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn local_examples_run_and_gated_examples_skip() {
    let mut failures = Vec::new();
    for (name, class) in EXAMPLES {
        let sandbox = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(sandbox.path()).unwrap();
        for sub in ["home", "run", "tmp"] {
            std::fs::create_dir_all(base.join(sub)).unwrap();
        }
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(base.join("run"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let mut command = Command::new("sh");
        command
            .arg(examples_dir().join(name).join("run.sh"))
            .current_dir(&base)
            .env("DRAFT_BIN", env!("CARGO_BIN_EXE_draft"))
            .env("HOME", base.join("home"))
            .env("XDG_RUNTIME_DIR", base.join("run"))
            .env("TMPDIR", base.join("tmp"))
            .env("DRAFT_GLOBAL_HOME", base.join("home/.draft"))
            .env_remove("DRAFT_EXAMPLE_PROVIDER_DIR")
            .env_remove("DRAFT_EXAMPLE_EXTENSIONS")
            .env_remove("NO_COLOR")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_bounded(command);
        let Some(output) = output else {
            failures.push(format!("{name}: did not finish within {TIMEOUT:?}"));
            continue;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            failures.push(format!(
                "{name}: {}\n--- stdout\n{stdout}\n--- stderr\n{stderr}",
                output.status
            ));
            continue;
        }
        let skipped = stdout.contains(&format!("SKIPPED {name}:"));
        match class {
            Class::Local => {
                if skipped {
                    failures.push(format!("{name}: a local example must never skip"));
                } else {
                    println!("ran {name}");
                }
            }
            Class::Provider | Class::Extension => {
                if skipped {
                    println!("skipped {name}: its gate variable is unset");
                } else {
                    failures.push(format!("{name}: ran although its gate variable is unset"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The child, or `None` if it outlived the timeout (and was killed).
fn run_bounded(mut command: Command) -> Option<std::process::Output> {
    let mut child = command.spawn().unwrap();
    // Drain the pipes on threads, so a chatty example cannot block on a full pipe.
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || read_all(&mut stdout));
    let err = std::thread::spawn(move || read_all(&mut stderr));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Some(std::process::Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    })
}

fn read_all(reader: &mut impl std::io::Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    bytes
}

/// Every `draft …` the examples run or document must still exist.
#[test]
fn examples_invoke_only_live_command_paths() {
    let golden = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/cli-surface.txt"
    ))
    .unwrap();
    let surface: BTreeSet<&str> = golden
        .lines()
        .filter(|line| line.starts_with("draft"))
        .collect();

    let mut sources = vec![
        examples_dir().join("lib.sh"),
        examples_dir().join("README.md"),
    ];
    for (name, _) in EXAMPLES {
        sources.push(examples_dir().join(name).join("run.sh"));
        sources.push(examples_dir().join(name).join("README.md"));
    }
    let mut stale = Vec::new();
    let mut seen = 0;
    for source in &sources {
        let text = std::fs::read_to_string(source).unwrap();
        for line in text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
        {
            for invocation in invocations(line) {
                seen += 1;
                for path in expand(&invocation) {
                    if !resolves(&surface, &path) {
                        stale.push(format!("{}: `{}`", source.display(), path.join(" ")));
                    }
                }
            }
        }
    }
    assert!(
        seen > 50,
        "the drift guard found only {seen} invocations; it has stopped reading the examples"
    );
    assert!(
        stale.is_empty(),
        "examples use command paths the CLI does not have:\n{}",
        stale.join("\n")
    );
}

/// The command words after each `draft` in command position.
fn invocations(line: &str) -> Vec<Vec<String>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut found = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        // `x="$(draft …` and `` `draft … `` both put `draft` in command position.
        if token.rsplit(['(', '`', '"']).next() != Some("draft") {
            continue;
        }
        let words: Vec<String> = tokens[index + 1..]
            .iter()
            .map(|word| word.trim_end_matches(['`', ',', ')', ';']))
            .take_while(|word| is_word(word) || word.split('|').all(is_word))
            .map(str::to_string)
            .collect();
        if !words.is_empty() {
            found.push(words);
        }
    }
    found
}

fn is_word(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_lowercase())
        && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

/// `pack evidence run|list` documents two paths.
fn expand(words: &[String]) -> Vec<Vec<String>> {
    let mut paths = vec![Vec::new()];
    for word in words {
        paths = paths
            .into_iter()
            .flat_map(|prefix: Vec<String>| {
                word.split('|').map(move |alternative| {
                    let mut path = prefix.clone();
                    path.push(alternative.to_string());
                    path
                })
            })
            .collect();
    }
    paths
}

/// The first word must be a command; later words may be positional arguments.
fn resolves(surface: &BTreeSet<&str>, words: &[String]) -> bool {
    let mut path = String::from("draft");
    let mut matched = 0;
    for (index, word) in words.iter().enumerate() {
        let candidate = format!("{path} {word}");
        if surface.contains(candidate.as_str()) {
            path = candidate;
            matched = index + 1;
        } else {
            break;
        }
    }
    matched > 0 && (matched == words.len() || !has_subcommands(surface, &path))
}

/// A path with subcommands must be followed by one of them, not by a stray word.
fn has_subcommands(surface: &BTreeSet<&str>, path: &str) -> bool {
    let prefix = format!("{path} ");
    surface.iter().any(|line| line.starts_with(&prefix))
}

#[test]
fn the_drift_guard_rejects_a_retired_path() {
    let golden = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/cli-surface.txt"
    ))
    .unwrap();
    let surface: BTreeSet<&str> = golden
        .lines()
        .filter(|line| line.starts_with("draft"))
        .collect();
    let check = |line: &str| {
        invocations(line)
            .iter()
            .flat_map(|words| expand(words))
            .all(|path| resolves(&surface, &path))
    };
    assert!(check("draft pack revision seal \"$change_pack\""));
    assert!(check("- `draft pack evidence run|list`"));
    assert!(!check("draft change new \"x\""));
    assert!(!check("draft pack revision frobnicate"));
    assert!(!check("x=\"$(draft export pack)\""));
}
