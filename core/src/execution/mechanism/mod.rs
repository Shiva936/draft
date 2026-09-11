//! The single place a contributed operation is performed.
//!
//! This sits deliberately *above* the `extension` port. That port describes
//! what a contribution is and holds no ability to act; this domain turns a
//! declaration into an execution, which is why it may reach the workspace, the
//! Change layer and the process runner while the port may not.
//!
//! Every mechanism — comparison, extraction, verification, a tool action, an
//! adapter's enumeration or its recovery capture and restore — is declared as
//! one [`MechanismOperation`] with one request contract, one response contract
//! and one bound on what Draft will accept back. This module executes exactly
//! that declaration, and nothing else in Draft launches a process on an
//! extension's behalf.
//!
//! The boundary is authority, budget, provenance and evidence — not an OS
//! sandbox. See [`crate::support::runtime_scope`] for what that does and does not
//! buy.

pub mod engine;
pub mod proposal;

use serde_json::Value;

use crate::extension::provenance::ProducerRef;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::runtime_scope::RuntimeScope;
use draft_extension_contract::{EngineId, Executor, MechanismOperation};
use engine as engines;

/// What Draft materialized for one invocation.
#[derive(Debug, Clone, Default)]
pub struct MechanismInputs {
    /// Named byte inputs, materialized into the scope's `input/` directory.
    pub files: Vec<(String, Vec<u8>)>,
}

/// What one command-backed invocation produced.
#[derive(Debug, Clone)]
pub struct MechanismResponse {
    /// The decoded response document, already bounds- and schema-checked.
    pub payload: Value,
    /// What actually ran, for the evidence record.
    pub executable_identity: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

/// Everything Draft needs to authorize and attribute one invocation.
#[derive(Debug, Clone)]
pub struct MechanismContext {
    pub workspace_id: String,
    pub operation_id: String,
    pub producer: ProducerRef,
    /// The decision digest that permitted this run. Required for a
    /// command-backed operation: Draft does not execute on trust alone.
    pub authorization_decision: Option<String>,
}

/// Run one declared command-backed operation.
///
/// Refuses before spawning anything when the operation is not authorized: an
/// artifact may be perfectly trusted and still have no grant to execute, and
/// those are separate facts.
pub fn invoke_command(
    operation: &MechanismOperation,
    request: &Value,
    inputs: &MechanismInputs,
    context: &MechanismContext,
) -> DraftResult<MechanismResponse> {
    let Executor::Command { command } = &operation.executor else {
        return Err(DraftError::invalid_config(
            "invoke_command was given an engine-backed operation",
        ));
    };
    if context.authorization_decision.is_none() {
        return Err(DraftError::new(
            DraftErrorKind::CapabilityNotAuthorized,
            format!(
                "extension {} is not authorized to execute this operation",
                context.producer.extension_id
            ),
        )
        .with_suggestion(format!(
            "run `draft extension authorize {} --permission process.execute`",
            context.producer.extension_id
        )));
    }

    let scope = RuntimeScope::create(&context.workspace_id, &context.operation_id)?;
    for (name, bytes) in &inputs.files {
        scope.materialize(name, bytes)?;
    }
    scope.write_request(request)?;

    // The only variables a declared command may reference, and they are all
    // paths into its own runtime scope. A placeholder Draft does not bind is an
    // error rather than a literal `{{name}}` handed to the process: a package
    // that asked for something it did not get should be told so, not run with a
    // nonsense argument.
    let bindings = std::collections::BTreeMap::from([
        (
            "request".to_string(),
            scope.request_file().to_string_lossy().into_owned(),
        ),
        (
            "input_dir".to_string(),
            scope.input_dir().to_string_lossy().into_owned(),
        ),
        (
            "output_dir".to_string(),
            scope.output_dir().to_string_lossy().into_owned(),
        ),
        (
            "metadata_dir".to_string(),
            scope.metadata_dir().to_string_lossy().into_owned(),
        ),
    ]);
    let arguments = command
        .args
        .iter()
        .map(|argument| crate::execution::process::interpolate(argument, &bindings))
        .collect::<DraftResult<Vec<String>>>()?;

    let working_directory = scope.working_directory(command.cwd.as_deref())?;
    let limits = crate::execution::process::ProcessLimits {
        timeout_ms: command.timeout_ms.or(Some(DEFAULT_TIMEOUT_MS)),
        env_allowlist: Vec::new(),
    };
    let outcome =
        crate::execution::process::run(&command.program, &arguments, &working_directory, &limits)?;

    if outcome.timed_out {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!(
                "declared operation '{}' exceeded its time limit",
                outcome.display
            ),
        ));
    }
    if !outcome.succeeded() {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!(
                "declared operation '{}' exited {}",
                outcome.display, outcome.exit_code
            ),
        ));
    }
    if outcome.stdout.len() as u64 > operation.max_response_bytes {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "declared operation '{}' returned {} bytes, over its declared bound of {}",
                outcome.display,
                outcome.stdout.len(),
                operation.max_response_bytes
            ),
        ));
    }

    let payload: Value = serde_json::from_slice(&outcome.stdout).map_err(|error| {
        DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "declared operation '{}' did not return a valid response document: {error}",
                outcome.display
            ),
        )
    })?;

    Ok(MechanismResponse {
        payload,
        executable_identity: outcome.display.clone(),
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
    })
}

/// The time limit a declared operation inherits when it sets none.
pub const DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Run one declared engine-backed comparison.
///
/// The engine is Draft's own code; the configuration is the extension's. This
/// refuses a contribution authored against a different engine revision rather
/// than running it under semantics it never agreed to.
pub fn compare_with_engine(
    operation: &MechanismOperation,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
    before_state_digest: Option<&str>,
    after_state_digest: Option<&str>,
) -> DraftResult<engines::EngineOutput> {
    let Executor::Engine {
        engine: engine_id,
        engine_revision,
        config,
    } = &operation.executor
    else {
        return Err(DraftError::invalid_config(
            "compare_with_engine was given a command-backed operation",
        ));
    };
    engines::check_revision(*engine_id, *engine_revision)?;

    match engine_id {
        EngineId::WholeResource => Ok(engines::whole::compare(
            before_state_digest,
            after_state_digest,
        )),
        EngineId::SequenceAlignment => {
            let config = engines::alignment::AlignmentConfig::parse(config)?;
            engines::alignment::compare(&config, before, after)
        }
        EngineId::KeyedRecordSet => {
            let config = engines::keyed::KeyedConfig::parse(config)?;
            engines::keyed::compare(&config, before, after)
        }
        EngineId::AttributeProjection | EngineId::ResourceEnumeration => {
            Err(DraftError::invalid_config(format!(
                "engine '{}' does not compare resources",
                engine_id.as_str()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{SchemaRef, StructuredCommand};

    fn producer() -> ProducerRef {
        ProducerRef {
            extension_id: "example.tool".into(),
            extension_version: "1.0.0".into(),
            package_digest: "sha256:pkg".into(),
            attestation_digest: "sha256:att".into(),
        }
    }

    fn schema(name: &str) -> SchemaRef {
        SchemaRef {
            schema_id: draft_extension_contract::NamespacedId::parse(name).unwrap(),
            revision: 1,
        }
    }

    fn command_operation(program: &str, args: &[&str]) -> MechanismOperation {
        MechanismOperation {
            request_contract: schema("example.tool/request"),
            response_contract: schema("example.tool/response"),
            max_response_bytes: 64 * 1024,
            executor: Executor::Command {
                command: StructuredCommand {
                    program: program.into(),
                    args: args.iter().map(|arg| arg.to_string()).collect(),
                    cwd: None,
                    timeout_ms: Some(30_000),
                },
            },
        }
    }

    fn context(authorized: bool) -> MechanismContext {
        MechanismContext {
            workspace_id: "prj_mechanism".into(),
            operation_id: format!("op_{}", uuid_like()),
            producer: producer(),
            authorization_decision: authorized.then(|| "sha256:decision".to_string()),
        }
    }

    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
    }

    #[test]
    fn an_unauthorized_operation_is_refused_before_anything_runs() {
        let operation = command_operation("definitely-not-a-real-program", &[]);
        let error = invoke_command(
            &operation,
            &serde_json::json!({}),
            &MechanismInputs::default(),
            &context(false),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CapabilityNotAuthorized);
        assert!(error.suggestion.is_some(), "refusal must say how to fix it");
    }

    #[cfg(unix)]
    #[test]
    fn an_authorized_operation_runs_and_its_response_is_decoded() {
        let operation = command_operation("printf", &[r#"{"ok":true}"#.to_string().as_str()]);
        let response = invoke_command(
            &operation,
            &serde_json::json!({"subject": "res_1"}),
            &MechanismInputs::default(),
            &context(true),
        )
        .unwrap();
        assert_eq!(response.payload, serde_json::json!({"ok": true}));
        assert_eq!(response.exit_code, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_response_over_the_declared_bound_is_refused() {
        let mut operation = command_operation("printf", &[r#"{"padding":"aaaaaaaaaa"}"#]);
        operation.max_response_bytes = 4;
        let error = invoke_command(
            &operation,
            &serde_json::json!({}),
            &MechanismInputs::default(),
            &context(true),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_document_response_never_enters_draft_state() {
        let operation = command_operation("printf", &["not json at all"]);
        let error = invoke_command(
            &operation,
            &serde_json::json!({}),
            &MechanismInputs::default(),
            &context(true),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[cfg(unix)]
    #[test]
    fn the_command_never_runs_in_the_project_or_the_control_plane() {
        // `pwd` reports where the mechanism actually landed.
        let operation = MechanismOperation {
            max_response_bytes: 64 * 1024,
            executor: Executor::Command {
                command: StructuredCommand {
                    program: "sh".into(),
                    // A single argv, no shell interpretation of anything Draft
                    // supplied: the program is `sh` only because the fixture
                    // needs to print its own working directory as JSON.
                    args: vec!["-c".into(), r#"printf '{"cwd":"%s"}' "$PWD""#.into()],
                    cwd: None,
                    timeout_ms: Some(30_000),
                },
            },
            ..command_operation("sh", &[])
        };
        let response = invoke_command(
            &operation,
            &serde_json::json!({}),
            &MechanismInputs::default(),
            &context(true),
        )
        .unwrap();
        let cwd = response.payload["cwd"].as_str().unwrap().to_string();
        assert!(
            !cwd.contains("/.draft"),
            "a mechanism must never run inside the control plane: {cwd}"
        );
        assert!(crate::support::runtime_scope::is_runtime_path(
            std::path::Path::new(&cwd)
        ));
    }

    #[test]
    fn an_engine_revision_mismatch_is_refused() {
        let operation = MechanismOperation {
            request_contract: schema("draft.core/comparison-request"),
            response_contract: schema("draft.core/comparison-result"),
            max_response_bytes: 1024,
            executor: Executor::Engine {
                engine: EngineId::WholeResource,
                engine_revision: 99,
                config: serde_json::json!({}),
            },
        };
        let error = compare_with_engine(&operation, None, None, Some("sha256:a"), Some("sha256:b"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::UnsupportedSchema);
    }

    #[test]
    fn engine_comparison_dispatches_on_the_declared_engine() {
        let operation = MechanismOperation {
            request_contract: schema("draft.core/comparison-request"),
            response_contract: schema("draft.core/comparison-result"),
            max_response_bytes: 1024,
            executor: Executor::Engine {
                engine: EngineId::SequenceAlignment,
                engine_revision: engines::alignment::REVISION,
                config: serde_json::json!({
                    "tokenizer": { "kind": "delimited", "delimiter_bytes": [10] },
                    "coordinate_space": "example/line",
                }),
            },
        };
        let output = compare_with_engine(
            &operation,
            Some(b"a\nb"),
            Some(b"a\nX"),
            Some("sha256:a"),
            Some("sha256:b"),
        )
        .unwrap();
        assert_eq!(output.claims.len(), 1);
    }

    #[test]
    fn an_unbound_placeholder_is_refused_rather_than_passed_through() {
        // A package that wrote `{{prompt}}` asked for something Draft does not
        // bind. Handing the process the literal text would run the command with
        // a nonsense argument and look like the tool misbehaved.
        let operation = MechanismOperation {
            request_contract: schema("draft.core/tool-action-request"),
            response_contract: schema("draft.core/tool-action-result"),
            max_response_bytes: 1024,
            executor: Executor::Command {
                command: draft_extension_contract::StructuredCommand {
                    program: "true".into(),
                    args: vec!["{{prompt}}".into()],
                    cwd: None,
                    timeout_ms: Some(1000),
                },
            },
        };
        let context = MechanismContext {
            workspace_id: "wsp_interpolation".into(),
            operation_id: "op_interpolation".into(),
            producer: ProducerRef {
                extension_id: "ex.pub".into(),
                extension_version: "1.0.0".into(),
                package_digest: "sha256:pkg".into(),
                attestation_digest: "sha256:att".into(),
            },
            authorization_decision: Some("authorized:ex.pub".into()),
        };
        let error = invoke_command(
            &operation,
            &serde_json::json!({}),
            &MechanismInputs::default(),
            &context,
        )
        .unwrap_err();
        assert!(
            error.message.contains("prompt"),
            "the error must name what was unbound: {}",
            error.message
        );
    }
}
