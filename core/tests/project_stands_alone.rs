//! Scenario BP: a project's own state does not depend on what is installed.
//!
//! `core::project` owns where a project is stored, its security state, its
//! control plane and its protections. None of that may depend on `extension`
//! or `trust`, for a reason that is easy to state and easy to lose:
//!
//! > A project's protections must hold whether or not any extension is
//! > installed, and its security state must not be a function of trust
//! > evaluation that sits above it.
//!
//! If `project` could reach those layers, an installed package could widen
//! what a project considers protected, and trust evaluation could feed back
//! into the state it is supposed to be evaluating. Both are the kind of cycle
//! that reads as fine in one file and is unfixable across twenty.
//!
//! Enforced structurally here rather than by convention: the dependency
//! direction is checked as a property of the source, so the guarantee survives
//! anyone adding a convenient import later.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn project_sources() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/project");
    let mut sources = Vec::new();
    let mut pending = vec![directory];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).expect("project sources are readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    assert!(!sources.is_empty(), "found no project sources to check");
    sources
}

#[test]
fn project_does_not_depend_on_extension_or_trust() {
    let mut offenders = BTreeSet::new();
    for source in project_sources() {
        let text = std::fs::read_to_string(&source).expect("a readable source file");
        for (number, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for forbidden in ["crate::extension", "crate::trust"] {
                if trimmed.contains(forbidden) {
                    offenders.insert(format!(
                        "{}:{}: {forbidden}",
                        source.file_name().unwrap().to_string_lossy(),
                        number + 1
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "core::project must stand alone; a project's protections and security state cannot \
         depend on what happens to be installed or on trust evaluation above them:\n{}",
        offenders.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn the_predicate_evaluator_project_relies_on_sits_below_both() {
    // `project` decides what is protected by evaluating predicates. That
    // evaluator therefore has to live below the extension layer — otherwise
    // the check above could only be satisfied by duplicating it, and two
    // copies of a matcher is how the two answers start to differ.
    let support = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/support/predicate.rs");
    assert!(
        support.exists(),
        "the predicate evaluator must live in support, below both project and extension"
    );

    let text = std::fs::read_to_string(&support).expect("a readable source file");
    for forbidden in ["crate::extension", "crate::trust", "crate::project"] {
        assert!(
            !text.contains(forbidden),
            "the shared predicate evaluator must not reach {forbidden}"
        );
    }
}
