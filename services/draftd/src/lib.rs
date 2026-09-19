use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use draft_core::app::App;
use draft_core::dcg::resource::ResourceLocator;
use draft_core::execution::operation::{BeginOperation, OperationStatus, OperationStore};
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_ipc::console_application::{
    input_contract_digest, ActionInputField, ActionInputKind, ActionPresentation, ActionTarget,
    ActionTargetKind, CanonicalRevisions, ConsoleActionInvocation, ConsoleActionResult,
    ConsoleHandshakeRequest, ConsoleHandshakeResponse, ConsoleModelRequest, ConsoleOperationPhase,
    ConsoleOperationStatus, ConsoleProtocolVersion, ConsoleReadModel, ConsoleScope, ConsoleSubject,
    ConsoleWatchEvent, ConsoleWatchRequest, ModelFreshness, NextSafeAction, Projection,
    ReadModelWatermark, RequestPrecondition, SelectOption, CONSOLE_CAPABILITIES,
    CONSOLE_PROTOCOL_MAJOR, CONSOLE_PROTOCOL_MINOR,
};
use draft_ipc::{
    ErrorObject, HandshakeRequest, HandshakeResponse, Request, Response, IPC_CAPABILITIES,
    IPC_PROTOCOL,
};
use draft_sessions::{ActionBinding, ApplicationSession, SessionManager};
use draft_store::{ServiceJobRecord, ServiceJobStatus, ServiceStore};

pub fn dispatch(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let id = req.id.clone();
    if let Err(error) = req.validate() {
        return Response::err(id, error);
    }
    if req.method == "service.handshake" {
        return handshake(req);
    }
    if req.method == "console.handshake" {
        return console_handshake(sessions, req);
    }
    if !is_mutation(&req.method) {
        return dispatch_inner(store, sessions, req);
    }

    let operation_id = req
        .operation_id
        .clone()
        .unwrap_or_else(|| format!("op_{}", req.id));
    let operation_id = draft_core::support::common::OperationId::new(operation_id);
    let request_hash = OperationStore::request_hash(&serde_json::json!({
        "method": req.method,
        "params": req.params,
    }));
    let operations = OperationStore::at(store.operations_root());
    let workspace_id = req
        .params
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let record =
        match operations.begin(operation_id, req.method.clone(), request_hash, workspace_id) {
            Ok(BeginOperation::New(record)) => record,
            Ok(BeginOperation::Replay(record)) => {
                return match record.status {
                    OperationStatus::Completed => {
                        Response::ok(id, record.result.expect("validated completed operation"))
                    }
                    OperationStatus::Failed | OperationStatus::Cancelled => Response::err(
                        id,
                        ErrorObject {
                            code: "OPERATION_REPLAY_FAILED".into(),
                            message: "the original operation did not complete".into(),
                            details: record.error.unwrap_or(Value::Null),
                        },
                    ),
                    _ => Response::err(
                        id,
                        ErrorObject::new(
                            "OPERATION_IN_PROGRESS",
                            "operation is already in progress",
                        ),
                    ),
                };
            }
            Err(error) => return Response::err(id, draft_err(&error)),
        };
    let record = match operations.mark_running(record) {
        Ok(record) => record,
        Err(error) => return Response::err(id, draft_err(&error)),
    };
    let record = if has_irreversible_boundary(&req.method) {
        let target_identity = req
            .params
            .get("change_pack_id")
            .or_else(|| req.params.get("target"))
            .or_else(|| req.params.get("workspace_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        match operations.begin_finalization(record, target_identity) {
            Ok(record) => record,
            Err(error) => return Response::err(id, draft_err(&error)),
        }
    } else {
        record
    };
    let response = dispatch_inner(store, sessions, req);
    let persisted = if response.ok {
        let result = response.result.clone().unwrap_or(Value::Null);
        operations.complete(record, result).map(|_| ())
    } else {
        let error = response
            .error
            .as_ref()
            .map(|value| serde_json::to_value(value).expect("IPC errors serialize as JSON"))
            .unwrap_or(Value::Null);
        operations.fail(record, error).map(|_| ())
    };
    if let Err(error) = persisted {
        return Response::err(id, draft_err(&error));
    }
    response
}

fn has_irreversible_boundary(method: &str) -> bool {
    matches!(
        method,
        // Promotion changes what the project accepts and publication causes an
        // effect outside Draft. Both are exactly what a finalization boundary
        // is for: a client that loses the reply must be able to ask what
        // happened rather than repeat it.
        "dcg.promotion.run"
            | "dcg.publication.run"
            | "rollback.run"
            | "workspace.relocate"
            | "workspace.adopt_copy"
    )
}

fn handshake(req: Request) -> Response {
    let requested: Result<HandshakeRequest, _> = serde_json::from_value(req.params);
    let Ok(requested) = requested else {
        return Response::err(
            req.id,
            ErrorObject::new("INVALID_HANDSHAKE", "invalid handshake request"),
        );
    };
    if requested.protocol != IPC_PROTOCOL {
        return Response::err(
            req.id,
            ErrorObject::new("UNSUPPORTED_PROTOCOL", "handshake protocol does not match"),
        );
    }
    if !draft_core::contracts::supports_version(
        draft_core::contracts::ContractId::IpcHandshakeRequest,
        requested.schema_version,
    ) {
        return Response::err(
            req.id,
            ErrorObject::new("UNSUPPORTED_SCHEMA", "handshake schema is unsupported"),
        );
    }
    let capabilities = IPC_CAPABILITIES
        .iter()
        .filter(|capability| {
            requested.requested_capabilities.is_empty()
                || requested
                    .requested_capabilities
                    .iter()
                    .any(|requested| requested == **capability)
        })
        .map(|capability| (*capability).to_string())
        .collect();
    Response::ok(
        req.id,
        serde_json::to_value(HandshakeResponse {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcHandshakeResponse,
            ),
            capabilities,
            daemon_name: "draftd".into(),
            daemon_version: draft_core::DRAFT_VERSION.into(),
        })
        .unwrap_or(Value::Null),
    )
}

fn is_mutation(method: &str) -> bool {
    !matches!(
        method,
        "service.ping"
            | "service.status"
            | "service.telemetry"
            | "workspace.status"
            | "workspace.list"
            | "events.list"
            | "events.verify"
            | "job.list"
            | "job.status"
            | "task.list"
            | "task.show"
            | "execution.list"
            | "execution.show"
            | "receipt.list"
            | "receipt.show"
            | "console.overview"
            | "console.project"
            | "console.project.settings"
            | "console.inbox"
            | "console.doctor"
            | "console.search"
            | "console.settings"
            | "console.snapshot"
            | "console.watch"
            | "operation.status"
            | "extension.list"
            | "extension.show"
            | "extension.source.list"
            | "extension.source.show"
            | "extension.search"
            | "notification.list"
            | "config.list"
            | "hook.list"
            | "ignore.list"
            | "candidate.list"
            | "classification.bundle"
            | "resource.list"
            | "resource.workspace"
            | "resource.get"
            | "resource.search"
            | "resource.workspace.show"
            // Observation reads, and the preview. Previewing a context change
            // deliberately changes nothing — no snapshot, no provenance record,
            // no move of what is in force — so classifying it as a mutation
            // would make looking at the consequences of a change cost the same
            // bookkeeping as making it.
            | "observation.context"
            | "observation.coverage"
            | "observation.provenance"
            | "observation.pending"
            | "observation.preview"
            | "observation.transitions"
            // The DCG read models. Every one of these is a projection of
            // durable records and writes nothing.
            | "dcg.project"
            | "dcg.baseline"
            | "dcg.baseline.list"
            | "dcg.baseline.show"
            | "dcg.change_pack.list"
            | "dcg.change_pack.intent"
            | "dcg.change_pack.scope"
            | "dcg.change_pack.receipts"
            // Reporting where an interrupted promotion stands is a read; the
            // classifier behind it counts nothing, so reading cannot be
            // mistaken for recovering.
            | "dcg.change_pack.recovery"
            | "dcg.change_pack.representation"
            | "dcg.change_pack.conflicts"
            | "dcg.change_pack.coverage"
            | "project.provider.list"
            | "project.provider.show"
            | "dcg.authorization"
            | "dcg.promotion.status"
            | "dcg.publication.list"
    )
}

fn dispatch_inner(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let app = app();
    let id = req.id.clone();
    match req.method.as_str() {
        "service.ping" => Response::ok(id, json!({ "pong": true })),
        "service.shutdown" => Response::ok(id, json!({ "stopping": true })),
        "service.status" => draft_core::project::registry::ProjectRegistry::global()
            .and_then(|registry| registry.list())
            .map(|workspaces| {
                json!({
                    "running": true,
                    "version": draft_core::DRAFT_VERSION,
                    "sessions": sessions.count(),
                    "workspaces": workspaces.len(),
                })
            })
            .into_response(id),
        // Process-local and reset by a restart, which is why the daemon is
        // where they are worth reading: a CLI process reports only the command
        // that just ran. A counter nothing emits comes back `null` rather than
        // 0, so "never happened" is never confused with "nothing is watching".
        "service.telemetry" => Response::ok(
            id,
            json!({
                "counters": draft_core::support::telemetry::snapshot()
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), json!(value)))
                    .collect::<serde_json::Map<_, _>>(),
            }),
        ),
        "workspace.list" => match draft_core::project::registry::ProjectRegistry::global()
            .and_then(|registry| registry.list())
        {
            Ok(entries) => Response::ok(id, serde_json::to_value(entries).unwrap_or(Value::Null)),
            Err(error) => Response::err(id, draft_err(&error)),
        },
        "workspace.init" => with_path(&req, |p| app.init(p)).into_response(id),
        "workspace.status" => with_path(&req, |p| app.status(p)).into_response(id),
        "workspace.register" => register_workspace(&app, &req).into_response(id),
        "workspace.unregister" => unregister_workspace(&req).into_response(id),
        "workspace.relocate" => relocate_workspace(&req).into_response(id),
        "workspace.adopt_copy" => adopt_workspace_copy(&app, &req).into_response(id),
        "console.overview" => console_overview(store, &app).into_response(id),
        "console.project" => console_project(&app, &req).into_response(id),
        "console.project.settings" => with_path(&req, |p| {
            Ok(json!({
                "config": app.config_list(p)?,
                "hooks": app.hook_list(p)?,
                "ignore": app.ignore_list(p)?,
                "candidates": app.candidate_list(p)?,
                "storage": app.storage_stats(p)?,
            }))
        })
        .into_response(id),
        "console.inbox" => console_inbox(&app).into_response(id),
        "console.doctor" => console_doctor(&app).into_response(id),
        "console.search" => console_search(&app, &req).into_response(id),
        "console.settings" => console_settings(&app).into_response(id),
        "console.snapshot" => console_snapshot(store, sessions, &app, &req).into_response(id),
        "console.watch" => console_watch(sessions, &req).into_response(id),
        "console.action.invoke" => console_action_response(store, sessions, &app, &req, id),
        "operation.status" => operation_status(store, &req).into_response(id),
        "operation.cancel" => operation_cancel(store, &req).into_response(id),
        "config.global.update" => nullable_string_param(&req, "name")
            .and_then(|name| {
                nullable_string_param(&req, "email").and_then(|email| {
                    app.config_update_user_global(
                        name.as_ref().map(|value| value.as_deref()),
                        email.as_ref().map(|value| value.as_deref()),
                    )
                })
            })
            .into_response(id),
        "extension.list" => draft_extension_service::extension::list()
            .and_then(draft_extension_service::authorization::views)
            .into_response(id),
        "extension.show" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_extension_service::extension::show(&extension_id))
            .and_then(draft_extension_service::authorization::view)
            .into_response(id),
        "extension.source.list" => {
            draft_extension_service::catalog::source_list().into_response(id)
        }
        "extension.source.add" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "location").and_then(|location| {
                    draft_extension_service::catalog::source_add(&source_id, &location)
                })
            })
            .into_response(id),
        "extension.source.remove" => string_param(&req, "source_id")
            .and_then(|source_id| draft_extension_service::catalog::source_remove(&source_id))
            .into_response(id),
        "extension.source.trust" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "root_json").and_then(|root_json| {
                    string_param(&req, "fingerprint").and_then(|fingerprint| {
                        draft_extension_service::catalog::trust_source_bytes(
                            &source_id,
                            root_json.as_bytes(),
                            &fingerprint,
                            req.params
                                .get("reset")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        )
                    })
                })
            })
            .into_response(id),
        "extension.source.show" => string_param(&req, "source_id")
            .and_then(|source_id| draft_extension_service::catalog::source_show(&source_id))
            .into_response(id),
        "extension.source.enable" => string_param(&req, "source_id")
            .and_then(|source_id| {
                draft_extension_service::catalog::source_set_enabled(&source_id, true)
            })
            .into_response(id),
        "extension.source.disable" => string_param(&req, "source_id")
            .and_then(|source_id| {
                draft_extension_service::catalog::source_set_enabled(&source_id, false)
            })
            .into_response(id),
        // Deleting a source stops discovery and updates from it; installed
        // packages and their provenance are kept.
        "extension.source.delete" => string_param(&req, "source_id")
            .and_then(|source_id| draft_extension_service::catalog::source_remove(&source_id))
            .into_response(id),
        // Refreshing without a source id refreshes every enabled source.
        "extension.source.refresh" => match req.params.get("source_id").and_then(Value::as_str) {
            Some(source_id) => {
                draft_extension_service::catalog::source_refresh(source_id).into_response(id)
            }
            None => draft_extension_service::catalog::source_refresh_all().into_response(id),
        },
        "extension.search" => draft_extension_service::discovery::search(
            &draft_extension_service::discovery::DiscoveryQuery {
                text: req
                    .params
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                source_id: req
                    .params
                    .get("source")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                capability: req
                    .params
                    .get("capability")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                page: usize_param(&req, "page", 1),
                limit: usize_param(
                    &req,
                    "limit",
                    draft_extension_service::discovery::DEFAULT_LIMIT,
                ),
            },
        )
        .into_response(id),
        "extension.install" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "extension_id").and_then(|extension_id| {
                    draft_extension_service::catalog::install_from_source(
                        &source_id,
                        &extension_id,
                        req.params.get("version").and_then(Value::as_str),
                    )
                })
            })
            .into_response(id),
        "extension.update" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "extension_id").and_then(|extension_id| {
                    draft_extension_service::catalog::update_from_source(
                        &source_id,
                        &extension_id,
                        req.params.get("version").and_then(Value::as_str),
                    )
                })
            })
            .into_response(id),
        "extension.update_all" => draft_extension_service::catalog::update_all().into_response(id),
        "extension.uninstall" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_extension_service::extension::uninstall(&extension_id))
            .into_response(id),
        "extension.authorize" => string_param(&req, "extension_id")
            .and_then(|extension_id| {
                let permissions = permission_params(&req)?;
                draft_extension_service::authorization::authorize(
                    &extension_id,
                    &permissions,
                    &operation_id_for(&req),
                )?;
                draft_extension_service::authorization::view(
                    draft_extension_service::extension::show(&extension_id)?,
                )
            })
            .into_response(id),
        "extension.revoke" => string_param(&req, "extension_id")
            .and_then(|extension_id| {
                let permission = match req.params.get("permission").and_then(Value::as_str) {
                    Some(name) => Some(
                        draft_core::extension::ExtensionPermission::parse(name)
                            .map_err(draft_core::extension::from_format_error)?,
                    ),
                    None => None,
                };
                draft_extension_service::authorization::revoke(
                    &extension_id,
                    permission,
                    &operation_id_for(&req),
                )?;
                draft_extension_service::authorization::view(
                    draft_extension_service::extension::show(&extension_id)?,
                )
            })
            .into_response(id),
        "extension.enable" => string_param(&req, "extension_id")
            .and_then(|extension_id| {
                draft_extension_service::extension::set_enabled(&extension_id, true)
            })
            .into_response(id),
        "extension.disable" => string_param(&req, "extension_id")
            .and_then(|extension_id| {
                draft_extension_service::extension::set_enabled(&extension_id, false)
            })
            .into_response(id),
        "notification.list" => draft_core::execution::notification::NotificationStore::global()
            .and_then(|store| {
                store.list(
                    req.params
                        .get("include_resolved")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
            })
            .into_response(id),
        "notification.read" => string_param(&req, "notification_id")
            .and_then(|notification_id| {
                draft_core::execution::notification::NotificationStore::global()?.mark_read(
                    &notification_id,
                    req.params
                        .get("read")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                )
            })
            .into_response(id),
        "notification.dismiss" => string_param(&req, "notification_id")
            .and_then(|notification_id| {
                draft_core::execution::notification::NotificationStore::global()?
                    .dismiss(&notification_id)
            })
            .into_response(id),
        "notification.resolve" => string_param(&req, "notification_id")
            .and_then(|notification_id| {
                draft_core::execution::notification::NotificationStore::global()?
                    .resolve(&notification_id)
            })
            .into_response(id),
        "events.list" => with_path(&req, |p| app.events(p)).into_response(id),
        "events.verify" => with_path(&req, |p| app.verify_events(p)).into_response(id),
        "events.replay" => with_path(&req, |p| app.replay_events(p)).into_response(id),
        "index.rebuild" => with_path(&req, |p| app.index_rebuild(p)).into_response(id),
        "job.submit" => submit_job(store, &app, &req).into_response(id),
        "job.list" => store.list_jobs().into_response(id),
        "job.status" => job_status(store, &req).into_response(id),
        "job.cancel" => job_cancel(store, &req).into_response(id),
        "task.list" => with_path(&req, |p| app.task_list(p)).into_response(id),
        "task.show" => {
            with_path(&req, |p| app.task_show(p, &string_param(&req, "task_id")?)).into_response(id)
        }
        "task.views" => with_path(&req, |p| {
            app.task_list(p).and_then(|tasks| {
                tasks
                    .into_iter()
                    .map(|task| app.task_view(p, task.id.as_str()))
                    .collect::<DraftResult<Vec<_>>>()
            })
        })
        .into_response(id),
        "task.create" => with_path(&req, |p| {
            app.task_create(
                p,
                &string_param(&req, "name")?,
                &string_param(&req, "goal")?,
                optional_string_param(&req, "template"),
                string_vec_param_default(&req, "allowed_zones"),
                string_vec_param_default(&req, "forbidden_zones"),
                string_vec_param_default(&req, "success_criteria"),
                optional_string_param(&req, "risk").as_deref(),
                optional_string_param(&req, "mode").as_deref(),
                optional_string_param(&req, "candidate_preset"),
            )
        })
        .into_response(id),
        "task.update" => with_path(&req, |p| update_task(&app, p, &req)).into_response(id),
        "task.next_action.add" => with_path(&req, |p| {
            app.task_add_next_action(
                p,
                &string_param(&req, "task")?,
                &string_param(&req, "label")?,
            )
        })
        .into_response(id),
        "task.next_action.set" => with_path(&req, |p| {
            app.task_set_next_action(
                p,
                &string_param(&req, "task")?,
                &string_param(&req, "action_id")?,
                req.params
                    .get("completed")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            )
        })
        .into_response(id),
        "task.drop" => with_path(&req, |p| {
            app.task_drop(
                p,
                &string_param(&req, "task")?,
                req.params
                    .get("hard")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        })
        .into_response(id),
        "checkpoint.create" => {
            with_path(&req, |p| app.checkpoint(p, &string_param(&req, "message")?))
                .into_response(id)
        }
        "execution.list" => with_path(&req, |p| {
            let workspace = app.open(p)?;
            draft_core::task::ExecutionStore::for_root(&workspace.root).list_all()
        })
        .into_response(id),
        "execution.show" => with_path(&req, |p| {
            let workspace = app.open(p)?;
            draft_core::task::ExecutionStore::for_root(&workspace.root)
                .read(&string_param(&req, "execution_id")?)
        })
        .into_response(id),
        "intent.list" => with_path(&req, |p| app.intents(p)).into_response(id),
        "observation.context" => with_path(&req, |p| app.observation_context(p)).into_response(id),
        "observation.coverage" => {
            with_path(&req, |p| app.observation_coverage(p)).into_response(id)
        }
        "observation.provenance" => with_path(&req, |p| {
            app.observation_provenance(p, optional_string_param(&req, "snapshot_digest").as_deref())
        })
        .into_response(id),
        "observation.pending" => with_path(&req, |p| app.observation_pending(p)).into_response(id),
        "observation.preview" => with_path(&req, |p| app.observation_preview(p)).into_response(id),
        "observation.adopt" => with_path(&req, |p| app.observation_adopt(p)).into_response(id),
        "observation.transitions" => {
            with_path(&req, |p| app.observation_transitions(p)).into_response(id)
        }
        "tool.list" => with_path(&req, |p| app.tool_list(p)).into_response(id),
        "tool.invoke" => with_path(&req, |p| {
            app.tool_invoke(
                p,
                &string_param(&req, "action_id")?,
                req.params
                    .get("apply")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        })
        .into_response(id),
        "task.templates" => with_path(&req, |p| app.task_templates(p)).into_response(id),
        "presentation.bindings" => with_path(&req, |p| {
            app.presentation_bindings(
                p,
                optional_string_param(&req, "surface")
                    .as_deref()
                    .unwrap_or("resource"),
            )
        })
        .into_response(id),
        // The judgement on record, which is a different question from "evaluate
        // this now". It refuses when the requirements have moved since, so a
        // caller can say the *readiness* is stale without implying the work is.
        "rollback.run" => with_path(&req, |p| {
            app.rollback(p, &string_param(&req, "target")?, true)
        })
        .into_response(id),
        "receipt.list" => with_path(&req, |p| app.receipts(p)).into_response(id),
        "receipt.show" => with_path(&req, |p| {
            app.receipt_show(p, &string_param(&req, "receipt_id")?)
        })
        .into_response(id),
        "inbox.list" => with_path(&req, |p| app.inbox(p)).into_response(id),
        "config.list" => with_path(&req, |p| app.config_list(p)).into_response(id),
        "config.set" => with_path(&req, |p| {
            app.config_set(
                p,
                &string_param(&req, "key")?,
                &string_param(&req, "value")?,
            )
        })
        .into_response(id),
        "config.unset" => {
            with_path(&req, |p| app.config_unset(p, &string_param(&req, "key")?)).into_response(id)
        }
        "hook.list" => with_path(&req, |p| app.hook_list(p)).into_response(id),
        "hook.set" => with_path(&req, |p| {
            app.hook_set(
                p,
                &string_param(&req, "key")?,
                &string_param(&req, "value")?,
            )
        })
        .into_response(id),
        "hook.unset" => {
            with_path(&req, |p| app.hook_unset(p, &string_param(&req, "key")?)).into_response(id)
        }
        "hook.run" => with_path(&req, |p| app.hook_run(p, &string_param(&req, "hook_name")?))
            .into_response(id),
        "ignore.list" => with_path(&req, |p| app.ignore_list(p)).into_response(id),
        "ignore.add" => with_path(&req, |p| app.ignore_add(p, &string_param(&req, "pattern")?))
            .into_response(id),
        "ignore.remove" => with_path(&req, |p| {
            app.ignore_remove(p, &string_param(&req, "pattern")?)
        })
        .into_response(id),
        "candidate.list" => with_path(&req, |p| app.candidate_list(p)).into_response(id),
        "candidate.add" => with_path(&req, |p| {
            app.candidate_add(
                p,
                &string_param(&req, "name")?,
                optional_string_param(&req, "kind").as_deref(),
                string_vec_param(&req, "command")?,
            )
        })
        .into_response(id),
        "candidate.update" => with_path(&req, |p| {
            app.candidate_update(
                p,
                &string_param(&req, "name")?,
                optional_string_param(&req, "kind").as_deref(),
                string_vec_param(&req, "command")?,
            )
        })
        .into_response(id),
        "candidate.remove" => with_path(&req, |p| {
            app.candidate_remove(p, &string_param(&req, "name")?)
        })
        .into_response(id),
        "doctor.project" => with_path(&req, |p| app.doctor(p)).into_response(id),
        "events.canonical" => with_path(&req, |p| app.canonical_events(p)).into_response(id),
        "resource.list" => with_path(&req, |p| app.resource_tree(p)).into_response(id),
        "classification.bundle" => {
            with_path(&req, |p| app.classification_report(p)).into_response(id)
        }
        "resource.workspace" => with_path(&req, |p| app.resource_workspace(p)).into_response(id),
        "resource.get" => with_path(&req, |p| {
            app.resource_read(p, &locator_param(&req, "resource_locator")?)
        })
        .into_response(id),
        "resource.search" => with_path(&req, |p| {
            app.resource_search(
                p,
                &string_param(&req, "query")?,
                req.params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100) as usize,
            )
        })
        .into_response(id),
        "resource.workspace.show" => with_path(&req, |p| {
            draft_core::execution::workspace::WorkspaceStore::for_workspace(p, app.protections(p)?)
                .load(&string_param(&req, "workspace_id")?)
        })
        .into_response(id),
        "resource.workspace.save" => {
            with_path(&req, |p| stage_in_change_workspace(&app, p, &req)).into_response(id)
        }
        "resource.workspace.stage" => {
            with_path(&req, |p| stage_in_change_workspace(&app, p, &req)).into_response(id)
        }
        "resource.workspace.commit" => {
            with_path(&req, |p| commit_change_workspace(&app, p, &req)).into_response(id)
        }
        "resource.create" => with_path(&req, |p| {
            app.resource_create(
                p,
                &locator_param(&req, "resource_locator")?,
                optional_string_param(&req, "content")
                    .as_deref()
                    .unwrap_or_default(),
            )
        })
        .into_response(id),
        "resource.relocate" => with_path(&req, |p| {
            app.resource_relocate(
                p,
                &locator_param(&req, "from")?,
                &locator_param(&req, "to")?,
            )
        })
        .into_response(id),
        "resource.delete" => with_path(&req, |p| {
            app.resource_delete(p, &locator_param(&req, "resource_locator")?)
        })
        .into_response(id),
        // -----------------------------------------------------------------
        // The DCG surface.
        //
        // Every arm calls one `App` operation and translates its inputs. None
        // of them decides whether a gate is satisfied, whether a decision
        // authorizes, what a promotion may commit, or how a publication
        // recovers: those are Core's, and a handler that answered them here
        // would be a second opinion about the project's authority.
        // -----------------------------------------------------------------
        "dcg.project" => with_path(&req, |p| app.dcg_project(p)).into_response(id),
        "dcg.baseline" => with_path(&req, |p| app.dcg_baseline(p)).into_response(id),
        "dcg.change_pack.list" => with_path(&req, |p| app.dcg_change_packs(p)).into_response(id),
        // The §8.3 ChangePack views that have their own authority. Each is one
        // call into the application API that owns the question; `draftd`
        // composes, it does not compute.
        "dcg.change_pack.intent" => with_path(&req, |p| {
            app.dcg_change_pack_intent(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.scope" => with_path(&req, |p| {
            app.dcg_change_pack_scope(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        // Where an interrupted promotion of this ChangePack stands, classified by
        // the restart table. Read-only: reporting a position must never be
        // mistaken for performing the recovery.
        "dcg.change_pack.recovery" => with_path(&req, |p| {
            app.dcg_change_pack_recovery(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.receipts" => with_path(&req, |p| {
            app.dcg_change_pack_receipts(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        // Baselines, in full. The accepted lineage and one exact node.
        "dcg.baseline.list" => with_path(&req, |p| app.dcg_baseline_details(p)).into_response(id),
        "dcg.baseline.show" => with_path(&req, |p| {
            app.dcg_baseline_detail(p, &string_param(&req, "baseline")?)
        })
        .into_response(id),
        // Provider state, read-only. Bindings are mutable and definitions and
        // profiles are not, and the catalog reports all three so a reader can
        // audit what an accepted Baseline was composed under even after the
        // binding moved off it.
        "project.provider.list" => with_path(&req, |p| app.provider_catalog(p)).into_response(id),
        "project.provider.show" => with_path(&req, |p| {
            app.provider_show(p, &string_param(&req, "binding")?)
        })
        .into_response(id),
        // The derived explanation of a revision, and what it stands to. Reads
        // only: the explanation was recorded when the revision was sealed, and
        // re-deriving it now would explain a workspace that has since moved.
        "dcg.change_pack.representation" => with_path(&req, |p| {
            app.dcg_representation(p, &string_param(&req, "revision_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.conflicts" => with_path(&req, |p| {
            app.dcg_conflicts(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.coverage" => with_path(&req, |p| {
            app.dcg_coverage(p, &string_param(&req, "revision_pack_id")?)
        })
        .into_response(id),
        // Deliberately absent: impact. Extraction crosses the process boundary
        // to an authorized extractor, so it is a mutation of the impact index
        // rather than a projection, and a "read model" that ran external
        // programs would be a read model in name only.
        "dcg.change_pack.impact" => with_path(&req, |p| {
            app.dcg_impact(p, &string_param(&req, "revision_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.open" => with_path(&req, |p| {
            app.dcg_open_change_pack(
                p,
                &string_param(&req, "intent")?,
                &string_vec_param(&req, "scope")?,
            )
        })
        .into_response(id),
        // Stopping and resuming work on a ChangePack. Exposed here so the Console
        // can do what the CLI can: a lifecycle transition reachable from one
        // surface and invisible to the other is how the two disagree about
        // what a project contains.
        "dcg.change_pack.abandon" => with_path(&req, |p| {
            app.dcg_abandon_change_pack(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        "dcg.change_pack.reopen" => with_path(&req, |p| {
            app.dcg_reopen_change_pack(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        // Recording that somebody looked. Not a Decision, and never treated as
        // one.
        "dcg.review.record" => with_path(&req, |p| {
            app.dcg_review(
                p,
                &string_param(&req, "revision_pack_id")?,
                &string_vec_param_default(&req, "comments"),
            )
        })
        .into_response(id),
        "dcg.revision_pack.seal" => with_path(&req, |p| {
            app.dcg_seal(p, &string_param(&req, "change_pack_id")?)
        })
        .into_response(id),
        "dcg.evidence.record" => with_path(&req, |p| {
            app.dcg_verify(p, &string_param(&req, "revision_pack_id")?)
        })
        .into_response(id),
        "dcg.assessment.record" => with_path(&req, |p| {
            app.dcg_assess(
                p,
                &string_param(&req, "revision_pack_id")?,
                &string_param(&req, "risk")?,
                optional_string_param(&req, "rationale")
                    .as_deref()
                    .unwrap_or("assessed through the console"),
            )
        })
        .into_response(id),
        "dcg.gate.evaluate" => with_path(&req, |p| {
            app.dcg_evaluate_gate(
                p,
                &string_param(&req, "revision_pack_id")?,
                &string_vec_param_default(&req, "waivers"),
            )
        })
        .into_response(id),
        // Excusing one gate condition on one exact revision. Never exposed
        // before this: the CLI could waive and the Console could not, so a
        // waiver was reachable from one surface and invisible to the other.
        "dcg.gate.waive" => with_path(&req, |p| {
            app.dcg_waive(
                p,
                &string_param(&req, "revision_pack_id")?,
                &string_param(&req, "condition")?,
                &string_param(&req, "reason")?,
                req.params.get("days").and_then(Value::as_u64).unwrap_or(7) as u32,
            )
        })
        .into_response(id),
        "dcg.authorization" => with_path(&req, |p| {
            app.dcg_authorization(
                p,
                &string_param(&req, "change_pack_id")?,
                &string_param(&req, "revision_pack_id")?,
            )
        })
        .into_response(id),
        "dcg.decision.record" => with_path(&req, |p| {
            app.dcg_decide(
                p,
                &string_param(&req, "revision_pack_id")?,
                optional_string_param(&req, "gate").as_deref(),
                req.params
                    .get("approve")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                optional_string_param(&req, "reason").as_deref(),
            )
        })
        .into_response(id),
        // `expected_baseline` is required, not optional. It is the precondition
        // token: a client acting on a view of the project that has since moved
        // must fail deterministically rather than promote onto a parent nobody
        // judged the work against.
        "dcg.promotion.run" => with_path(&req, |p| {
            app.dcg_promote(
                p,
                &string_param(&req, "change_pack_id")?,
                &string_param(&req, "revision_pack_id")?,
                &string_param(&req, "decision")?,
                &string_param(&req, "gate")?,
                Some(string_param(&req, "expected_baseline")?.as_str()),
            )
        })
        .into_response(id),
        "dcg.promotion.status" => with_path(&req, |p| {
            app.dcg_promotion(p, &string_param(&req, "promotion")?)
        })
        .into_response(id),
        // The caller chooses what to deliver and what for. It cannot choose
        // the delivery semantics, the recovery class, or the attempt identity:
        // the first is the provider's, the second follows from it, and the
        // third is derived from the operation id so a retry converges instead
        // of delivering twice.
        "dcg.publication.run" => with_path(&req, |p| {
            app.dcg_publish(
                p,
                optional_string_param(&req, "baseline").as_deref(),
                optional_string_param(&req, "purpose")
                    .as_deref()
                    .unwrap_or("draft.publish/export"),
                operation_id_for(&req).as_str(),
                optional_string_param(&req, "retry_authorization").as_deref(),
            )
        })
        .into_response(id),
        "dcg.publication.list" => with_path(&req, |p| app.dcg_publications(p)).into_response(id),
        // Publishing is its own capability, so it is granted explicitly rather
        // than implied by the authority to promote.
        // The recorded decision that a delivery Draft could not establish may
        // be attempted again. One authorization, one attempt.
        "dcg.publication.authorize_retry" => with_path(&req, |p| {
            app.dcg_authorize_retry(
                p,
                optional_string_param(&req, "purpose")
                    .as_deref()
                    .unwrap_or("draft.publish/export"),
                operation_id_for(&req).as_str(),
                &string_param(&req, "rationale")?,
            )
            .map(|digest| json!({ "retry_authorization": digest }))
        })
        .into_response(id),
        // Frees a Publication held by an attempt that stalled before it was
        // sent. Refuses one that was already dispatched.
        "dcg.publication.withdraw_attempt" => with_path(&req, |p| {
            app.dcg_withdraw_attempt(
                p,
                optional_string_param(&req, "purpose")
                    .as_deref()
                    .unwrap_or("draft.publish/export"),
                &string_param(&req, "attempt")?,
                &string_param(&req, "reason")?,
            )
        })
        .into_response(id),
        "dcg.publication.grant" => with_path(&req, |p| app.dcg_grant_publish(p)).into_response(id),
        other => Response::err(
            id,
            ErrorObject::new("UNKNOWN_METHOD", format!("unknown method: {other}")),
        ),
    }
}

fn stage_in_change_workspace(app: &App, root: &Path, req: &Request) -> DraftResult<Value> {
    let attribution: draft_core::execution::workspace::EditAttribution =
        serde_json::from_value(req.params.get("attribution").cloned().ok_or_else(|| {
            DraftError::invalid_config("a ChangePack workspace attribution is required")
        })?)
        .map_err(|error| {
            DraftError::invalid_config(format!("invalid ChangePack workspace attribution: {error}"))
        })?;
    validate_workspace_attribution(app, root, &attribution)?;
    let operation_id = draft_core::support::common::OperationId::new(
        req.operation_id
            .clone()
            .unwrap_or_else(|| format!("op_{}", req.id)),
    );
    let store = draft_core::execution::workspace::WorkspaceStore::for_workspace(
        root,
        app.protections(root)?,
    );
    let session = if let Some(workspace_id) = optional_string_param(req, "change_workspace") {
        let existing = store.load(&workspace_id)?;
        if existing.attribution != attribution {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "attribution cannot change within an open ChangePack workspace",
            ));
        }
        existing
    } else {
        store.open(attribution, operation_id.clone())?
    };
    let edit_kind = optional_string_param(req, "edit_kind").unwrap_or_else(|| "set_content".into());
    match edit_kind.as_str() {
        "set_content" | "write" | "create_file" => to_value(store.stage_content(
            &session.id,
            &locator_param(req, "resource_locator")?,
            optional_string_param(req, "content").unwrap_or_default(),
            operation_id,
        )?),
        "create_collection" | "create_directory" => to_value(store.stage_create_collection(
            &session.id,
            &locator_param(req, "resource_locator")?,
            operation_id,
        )?),
        "relocate" | "rename" | "move" => to_value(store.stage_relocate(
            &session.id,
            &locator_param(req, "from")?,
            &locator_param(req, "to")?,
            operation_id,
        )?),
        "remove" | "delete" => to_value(
            store.stage_remove(
                &session.id,
                &locator_param(req, "resource_locator")?,
                req.params
                    .get("recursive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                operation_id,
            )?,
        ),
        _ => Err(DraftError::invalid_config(
            "unknown staged resource operation",
        )),
    }
}

fn commit_change_workspace(app: &App, root: &Path, req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "change_workspace")?;
    let session = draft_core::execution::workspace::WorkspaceStore::for_workspace(
        root,
        app.protections(root)?,
    )
    .load(&workspace_id)?;
    validate_workspace_attribution(app, root, &session.attribution)?;
    let operation_id = draft_core::support::common::OperationId::new(
        req.operation_id
            .clone()
            .unwrap_or_else(|| format!("op_{}", req.id)),
    );
    to_value(app.workspace_commit(root, &workspace_id, operation_id)?)
}

fn validate_workspace_attribution(
    app: &App,
    root: &Path,
    attribution: &draft_core::execution::workspace::EditAttribution,
) -> DraftResult<()> {
    use draft_core::execution::workspace::EditAttribution;
    match attribution {
        EditAttribution::Task { id } => {
            app.task_show(root, id)?;
        }
        EditAttribution::ChangePack { id } | EditAttribution::Review { id } => {
            let change = app
                .dcg_change_packs(root)?
                .into_iter()
                .find(|view| view.change_pack.as_str() == id)
                .ok_or_else(|| {
                    DraftError::invalid_config(format!(
                        "ChangePack '{id}' is not in this project's graph"
                    ))
                })?;
            if !change.lifecycle.accepts_work() {
                return Err(DraftError::invalid_config(
                    "a ChangePack that accepts no further work cannot receive workspace mutations",
                ));
            }
        }
        EditAttribution::CandidateExecution { id } => {
            let workspace = app.open(root)?;
            draft_core::task::ExecutionStore::for_root(&workspace.root).read(id)?;
        }
    }
    Ok(())
}

fn submit_job(store: &ServiceStore, _app: &App, req: &Request) -> DraftResult<ServiceJobRecord> {
    let kind = string_param(req, "kind")?;
    let workspace_id = optional_string_param(req, "workspace_id");
    let global_job = matches!(
        kind.as_str(),
        "extension-install" | "extension-update" | "extension-update-all"
    );
    let path = if global_job {
        String::new()
    } else if let Some(path) = optional_string_param(req, "path") {
        path
    } else if let Some(workspace_id) = &workspace_id {
        draft_core::project::registry::ProjectRegistry::global()?
            .resolve(workspace_id)?
            .project_path
    } else {
        return Err(DraftError::new(
            DraftErrorKind::IpcError,
            "job submission requires workspace_id",
        ));
    };
    let job = ServiceJobRecord {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::ServiceJob,
        ),
        id: format!("job_{}", &uuid_like()[..12]),
        kind: kind.clone(),
        workspace_path: path.clone(),
        status: ServiceJobStatus::Queued,
        submitted_at: chrono::Utc::now(),
        started_at: None,
        ended_at: None,
        result: None,
        error: None,
        operation_id: req.operation_id.clone(),
        workspace_id,
        phase: "queued".into(),
        progress_completed: 0,
        progress_total: None,
        cancellation_requested: false,
        params: req.params.clone(),
        correlation_id: req.correlation_id.clone(),
        attempt: 0,
        recovered_at: None,
    };
    store
        .save_job(&job)
        .map_err(|e| DraftError::storage(format!("failed to save queued job: {e}")))?;
    if let Err(error) = spawn_job(store.clone(), job.id.clone()) {
        let mut failed = job.clone();
        failed.status = ServiceJobStatus::Failed;
        failed.phase = "start_failed".into();
        failed.error = Some(error.to_string());
        failed.ended_at = Some(chrono::Utc::now());
        store.save_job(&failed)?;
        return Err(error);
    }
    Ok(job)
}

fn spawn_job(store: ServiceStore, job_id: String) -> DraftResult<()> {
    std::thread::Builder::new()
        .name(format!("draft-job-{job_id}"))
        .spawn(move || execute_job(&store, &job_id))
        .map(|_| ())
        .map_err(|error| DraftError::storage(format!("failed to start durable job: {error}")))
}

fn execute_job(store: &ServiceStore, job_id: &str) {
    let mut job = match store.load_job(job_id) {
        Ok(Some(job)) => job,
        Ok(None) => return,
        Err(error) => {
            store.log(&format!("cannot load durable job {job_id}: {error}"));
            return;
        }
    };
    if job.cancellation_requested || job.status == ServiceJobStatus::Cancelled {
        return;
    }

    job.status = ServiceJobStatus::Running;
    job.phase = "executing".into();
    job.started_at.get_or_insert_with(chrono::Utc::now);
    job.attempt = job.attempt.saturating_add(1);
    job.progress_total = Some(1);
    if let Err(error) = store.save_job(&job) {
        store.log(&format!("cannot persist running job {job_id}: {error}"));
        return;
    }

    let request = Request {
        protocol: IPC_PROTOCOL.into(),
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::IpcRequest,
        ),
        id: job.id.clone(),
        correlation_id: job.correlation_id.clone(),
        operation_id: job.operation_id.clone(),
        method: "job.resume".into(),
        params: job.params.clone(),
    };
    let result = run_job(&app(), Path::new(&job.workspace_path), &request, &job.kind);

    // Cancellation is a durable flag. Reload it before finalization so a
    // concurrent cancellation can never be overwritten by a late worker.
    let mut current = match store.load_job(job_id) {
        Ok(Some(job)) => job,
        Ok(None) => return,
        Err(error) => {
            store.log(&format!("cannot reload durable job {job_id}: {error}"));
            return;
        }
    };
    if current.cancellation_requested || current.status == ServiceJobStatus::Cancelled {
        current.status = ServiceJobStatus::Cancelled;
        current.phase = "cancelled".into();
        current.ended_at.get_or_insert_with(chrono::Utc::now);
        if let Err(error) = store.save_job(&current) {
            store.log(&format!("cannot persist cancelled job {job_id}: {error}"));
        }
        return;
    }

    current.ended_at = Some(chrono::Utc::now());
    match result {
        Ok(value) => {
            current.status = ServiceJobStatus::Completed;
            current.result = Some(value);
            current.error = None;
            current.phase = "completed".into();
            current.progress_completed = 1;
            current.progress_total = Some(1);
        }
        Err(error) => {
            current.status = ServiceJobStatus::Failed;
            current.error = Some(error.to_string());
            current.phase = "failed".into();
        }
    }
    if let Err(error) = store.save_job(&current) {
        store.log(&format!("cannot persist finalized job {job_id}: {error}"));
    }
}

/// Resume queued or interrupted jobs after a daemon restart. Each job retains
/// its original operation and correlation IDs and is finalized by a fenced
/// reload of the current durable record.
pub fn recover_jobs(store: &ServiceStore) -> DraftResult<usize> {
    let mut recovered = 0;
    for mut job in store.list_jobs()? {
        if !matches!(
            job.status,
            ServiceJobStatus::Queued | ServiceJobStatus::Running
        ) {
            continue;
        }
        if job.params.is_null() {
            job.status = ServiceJobStatus::Failed;
            job.phase = "recovery_failed".into();
            job.error = Some("job has no canonical resumable parameters".into());
            job.ended_at = Some(chrono::Utc::now());
            store.save_job(&job)?;
            continue;
        }
        job.status = ServiceJobStatus::Queued;
        job.phase = "recovering".into();
        job.recovered_at = Some(chrono::Utc::now());
        job.started_at = None;
        job.ended_at = None;
        job.result = None;
        job.error = None;
        job.progress_completed = 0;
        store.save_job(&job)?;
        spawn_job(store.clone(), job.id.clone())?;
        recovered += 1;
    }
    Ok(recovered)
}

fn run_job(app: &App, path: &Path, req: &Request, kind: &str) -> DraftResult<Value> {
    match kind {
        "scan" => to_value(app.status(path)?),
        "rollback" => to_value(app.rollback(path, &string_param(req, "target")?, true)?),
        "index-rebuild" => to_value(app.index_rebuild(path)?),
        "extension-install" => to_value(draft_extension_service::catalog::install_from_source(
            &string_param(req, "source_id")?,
            &string_param(req, "extension_id")?,
            req.params.get("version").and_then(Value::as_str),
        )?),
        "extension-update" => to_value(draft_extension_service::catalog::update_from_source(
            &string_param(req, "source_id")?,
            &string_param(req, "extension_id")?,
            req.params.get("version").and_then(Value::as_str),
        )?),
        "extension-update-all" => to_value(draft_extension_service::catalog::update_all()?),
        other => Err(DraftError::new(
            DraftErrorKind::IpcError,
            format!("unknown job kind: {other}"),
        )),
    }
}

fn job_status(store: &ServiceStore, req: &Request) -> DraftResult<ServiceJobRecord> {
    let id = string_param(req, "job_id")?;
    store
        .load_job(&id)?
        .ok_or_else(|| DraftError::not_found(format!("unknown job: {id}")))
}

fn job_cancel(store: &ServiceStore, req: &Request) -> DraftResult<ServiceJobRecord> {
    let mut job = job_status(store, req)?;
    if matches!(
        job.status,
        ServiceJobStatus::Queued | ServiceJobStatus::Running
    ) {
        job.status = ServiceJobStatus::Cancelled;
        job.phase = "cancelled".into();
        job.cancellation_requested = true;
        job.ended_at = Some(chrono::Utc::now());
        store
            .save_job(&job)
            .map_err(|e| DraftError::storage(format!("failed to save cancelled job: {e}")))?;
    }
    Ok(job)
}

fn to_value<T: Serialize>(value: T) -> DraftResult<Value> {
    serde_json::to_value(value)
        .map_err(|e| DraftError::storage(format!("failed to encode job result: {e}")))
}

fn uuid_like() -> String {
    format!(
        "{:x}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn register_workspace(app: &App, req: &Request) -> DraftResult<Value> {
    let path = string_param(req, "path")?;
    let ws = app.open(Path::new(&path))?;
    draft_core::project::registry::ProjectRegistry::global()?.upsert(
        ws.workspace_id.as_str(),
        &ws.root,
        None,
        draft_core::dcg::source_view::WorkspaceRevision::derive(&ws.root)
            .ok()
            .map(|revision| revision.content_digest),
    )?;
    Ok(json!({ "registered": true, "workspace_id": ws.workspace_id.to_string() }))
}

fn unregister_workspace(req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "workspace_id")?;
    let removed =
        draft_core::project::registry::ProjectRegistry::global()?.remove(&workspace_id)?;
    Ok(json!({ "workspace_id": workspace_id, "unregistered": removed }))
}

fn relocate_workspace(req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "workspace_id")?;
    let destination = string_param(req, "destination")?;
    to_value(
        draft_core::project::registry::ProjectRegistry::global()?
            .relocate(&workspace_id, Path::new(&destination))?,
    )
}

fn adopt_workspace_copy(_app: &App, req: &Request) -> DraftResult<Value> {
    let path = string_param(req, "path")?;
    let receipt = draft_core::app::adoption::adopt_copy(Path::new(&path))?;
    to_value(receipt)
}

fn with_path<T, F>(req: &Request, f: F) -> DraftResult<T>
where
    F: FnOnce(&Path) -> DraftResult<T>,
{
    let path = if let Some(path) = optional_string_param(req, "path") {
        path
    // The *project*, not a ChangePack workspace. The Console gateway addresses every
    // project call by this id, so naming it after the ChangePack-workspace
    // parameter would make each of them resolve to nothing.
    } else if let Some(project_id) = optional_string_param(req, "workspace_id") {
        let registry = draft_core::project::registry::ProjectRegistry::global()?;
        if is_mutation(&req.method) {
            let blocking = registry.inspect()?.into_iter().any(|issue| {
                issue.workspace_id == project_id
                    && (matches!(
                        issue.kind.as_str(),
                        "workspace_identity_corrupt" | "path_reused_by_different_workspace"
                    ) || issue.kind.starts_with("identity_conflict_with:"))
            });
            if blocking {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    "project identity conflict blocks mutations",
                ));
            }
        }
        registry.resolve(&project_id)?.project_path
    } else {
        return Err(DraftError::new(
            DraftErrorKind::IpcError,
            "missing 'workspace_id' parameter",
        ));
    };
    if path.contains("..") {
        return Err(DraftError::new(
            DraftErrorKind::IpcError,
            "path traversal is not allowed",
        ));
    }
    f(Path::new(&path))
}

fn console_handshake(sessions: &SessionManager, req: Request) -> Response {
    let requested = match serde_json::from_value::<ConsoleHandshakeRequest>(req.params) {
        Ok(requested) => requested,
        Err(error) => {
            return Response::err(
                req.id,
                ErrorObject::new("INVALID_CONSOLE_HANDSHAKE", error.to_string()),
            )
        }
    };
    if requested.protocol.major != CONSOLE_PROTOCOL_MAJOR {
        let mut error = ErrorObject::new(
            "INCOMPATIBLE_CONSOLE_PROTOCOL_MAJOR",
            format!(
                "Console protocol major {} is incompatible with server major {}",
                requested.protocol.major, CONSOLE_PROTOCOL_MAJOR
            ),
        );
        error.details = json!({
            "client": requested.protocol,
            "server": ConsoleProtocolVersion::default(),
        });
        return Response::err(req.id, error);
    }
    if requested.client_name.trim().is_empty() || requested.client_instance_id.trim().is_empty() {
        return Response::err(
            req.id,
            ErrorObject::new(
                "INVALID_CONSOLE_HANDSHAKE",
                "client_name and client_instance_id are required",
            ),
        );
    }
    let negotiated_capabilities = CONSOLE_CAPABILITIES
        .iter()
        .filter(|capability| {
            requested.requested_capabilities.is_empty()
                || requested
                    .requested_capabilities
                    .iter()
                    .any(|candidate| candidate == **capability)
        })
        .map(|capability| (*capability).to_string())
        .collect::<Vec<_>>();
    let session = sessions.open_application(
        requested.client_instance_id,
        "local-user".into(),
        negotiated_capabilities.clone(),
    );
    let registry_revision = draft_core::project::registry::ProjectRegistry::global()
        .and_then(|registry| registry.envelope())
        .map(|envelope| envelope.revision)
        .unwrap_or_default();
    Response::ok(
        req.id,
        serde_json::to_value(ConsoleHandshakeResponse {
            protocol: ConsoleProtocolVersion {
                major: CONSOLE_PROTOCOL_MAJOR,
                minor: CONSOLE_PROTOCOL_MINOR,
            },
            client_version: requested.client_version,
            server_version: draft_core::DRAFT_VERSION.into(),
            supported_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
            negotiated_capabilities,
            registry_revision,
            application_session_id: session.id,
        })
        .unwrap_or(Value::Null),
    )
}

fn console_snapshot(
    store: &ServiceStore,
    sessions: &SessionManager,
    app: &App,
    req: &Request,
) -> ConsoleResult<ConsoleReadModel> {
    let requested: ConsoleModelRequest = serde_json::from_value(req.params.clone())
        .map_err(|error| DraftError::invalid_config(error.to_string()))?;
    let session = sessions
        .application(&requested.application_session_id)
        .ok_or(ConsoleError::SessionUnknown(
            "Console application session is missing or was replaced; reconnect and refresh",
        ))?;
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    let mut revisions = CanonicalRevisions {
        registry: registry.envelope()?.revision,
        ..CanonicalRevisions::default()
    };
    // Carried out of the match so the action presentations below are gated by
    // the same answer the content shows. Two computations of "may I promote?"
    // could disagree, and the visible one would be the wrong one.
    let mut dcg_availability: Vec<draft_core::app::workflow::ActionAvailability> = Vec::new();
    let mut dcg_next_action: Option<String> = None;
    let (content, health, freshness, read_only) = match requested.subject.scope() {
        ConsoleScope::Global => (
            json!({
                "overview": console_overview(store, app)?,
                "inbox": console_inbox(app)?,
                "doctor": console_doctor(app)?,
                "extensions": {
                    // The same projection `extension.list` returns, so the
                    // model carries declared/authorized/pending permissions
                    // rather than the bare installed record. Without this the
                    // TUI and the browser would see strictly less than the CLI.
                    "installed": draft_extension_service::extension::list()
                        .and_then(draft_extension_service::authorization::views)?,
                    "sources": draft_extension_service::catalog::source_list()?,
                },
                "settings": console_settings(app)?,
            }),
            "healthy".to_string(),
            ModelFreshness::Fresh,
            false,
        ),
        ConsoleScope::Project => {
            let workspace_id =
                required_subject_value(requested.subject.workspace_id(), "workspace_id")?;
            if requested.subject.change_pack_id().is_some() {
                return Err(DraftError::invalid_config(
                    "PROJECT subjects cannot include change_pack_id",
                )
                .into());
            }
            let entry = registry.resolve(workspace_id)?;
            let root = Path::new(&entry.project_path);
            if !root.exists() {
                (
                    json!({ "project": entry, "error": { "code": "PATH_MISSING", "message": "registered project path is unavailable" } }),
                    "unavailable".into(),
                    ModelFreshness::Unavailable,
                    true,
                )
            } else {
                match draft_core::dcg::source_view::WorkspaceRevision::derive(root) {
                    Ok(revision) => {
                        revisions.workspace = Some(revision.content_digest);
                        let proxy = Request::new(
                            "console-project-model",
                            "console.project",
                            json!({"workspace_id": workspace_id}),
                        );
                        match console_project(app, &proxy) {
                            Ok(model) => {
                                let settings_proxy = Request::new(
                                    "console-project-settings-model",
                                    "console.project.settings",
                                    json!({"workspace_id": workspace_id}),
                                );
                                let settings = with_path(&settings_proxy, |path| {
                                    Ok(json!({
                                        "config": app.config_list(path)?,
                                        "hooks": app.hook_list(path)?,
                                        "ignore": app.ignore_list(path)?,
                                        "candidates": app.candidate_list(path)?,
                                        "storage": app.storage_stats(path)?,
                                    }))
                                })?;
                                let graph = app.dcg_project(root)?;
                                // Availability for the newest revision of each
                                // ChangePack, computed here so no frontend has to
                                // work out whether promoting is legal by
                                // reading lifecycle enums.
                                let authorizations = dcg_authorizations(app, root, &graph);
                                dcg_availability = graph.actions.clone();
                                dcg_next_action = graph.next_action.clone();
                                // Keyed by the §8.3 sections, so the
                                // structure a frontend renders and the
                                // structure the authority serves are the same
                                // structure. Authorization is not a section of
                                // its own: it is what a ChangePack's own views
                                // show, and it is carried here only so the
                                // project-level ChangePacks list can say which
                                // ChangePack may advance without asking again.
                                (
                                    json!({
                                        "overview": model,
                                        "work": {
                                            "tasks": app.task_list(root)?,
                                            "packs": console_change_pack_summaries(app, root)?,
                                            "authorizations": authorizations,
                                        },
                                        "resources": {
                                            "resources": {
                                                "workspace": app.resource_workspace(root)?,
                                                "tree": app.resource_tree(root)?,
                                                // What the project is made of,
                                                // as installed extensions
                                                // describe it. The Console
                                                // renders this; it does not
                                                // derive it.
                                                "classification": app.classification_report(root)?,
                                            },
                                            // Observation re-observes the
                                            // workspace, so it stays on its own
                                            // routes and is not folded in here.
                                            // A model every screen waits for is
                                            // a model no screen can afford.
                                        },
                                        // Publication sits beside the
                                        // Baselines rather than inside one:
                                        // delivering a Baseline somewhere is
                                        // not part of what the project
                                        // accepts, and folding it in would
                                        // make an external failure look like
                                        // a Baseline that failed.
                                        //
                                        // The accepted Baseline and the
                                        // deliveries from it; the full lineage
                                        // with roots, composition and
                                        // recoverability is `dcg.baseline.list`,
                                        // which walks every snapshot's anchors.
                                        "baselines": {
                                            "current": graph.baseline.clone(),
                                            "publications": app.dcg_publications(root)?,
                                        },
                                        "activity": app.events(root)?,
                                        "providers": app.provider_catalog(root)?,
                                        // Installed extensions, as the CLI
                                        // sees them. Contributed tools match
                                        // their selectors against every
                                        // Resource, so they stay on their own
                                        // route: a model every screen waits
                                        // for is a model no screen can afford.
                                        "extensions": {
                                            "extensions": draft_extension_service::extension::list()
                                                .and_then(draft_extension_service::authorization::views)?,
                                        },
                                        "settings": settings,
                                    }),
                                    entry.health,
                                    ModelFreshness::Fresh,
                                    false,
                                )
                            }
                            Err(error) => (
                                json!({ "project": entry, "error": { "code": error.code(), "message": error.message } }),
                                "degraded".into(),
                                ModelFreshness::Partial,
                                true,
                            ),
                        }
                    }
                    Err(error) => (
                        json!({ "project": entry, "error": { "code": error.code(), "message": error.message } }),
                        "degraded".into(),
                        ModelFreshness::Partial,
                        true,
                    ),
                }
            }
        }
        ConsoleScope::ChangePack => {
            let workspace_id =
                required_subject_value(requested.subject.workspace_id(), "workspace_id")?;
            let change_pack_id =
                required_subject_value(requested.subject.change_pack_id(), "change_pack_id")?;
            let entry = registry.resolve(workspace_id)?;
            let root = Path::new(&entry.project_path);
            revisions.workspace =
                Some(draft_core::dcg::source_view::WorkspaceRevision::derive(root)?.content_digest);

            // One ChangePack, as the Change Graph sees it: its newest sealed
            // revision and everything decided about it. The availability comes
            // from the same computation the project scope uses, so the two
            // scopes can never disagree about whether promoting is legal.
            let change = app
                .dcg_change_packs(root)?
                .into_iter()
                .find(|view| view.change_pack.as_str() == change_pack_id)
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        format!("ChangePack '{change_pack_id}' is not in this project's graph"),
                    )
                })?;
            let authorization = match change.revisions.first() {
                Some(revision) => {
                    revisions.change_pack = Some(revision.id.to_string());
                    Some(app.dcg_authorization(root, change_pack_id, revision.id.as_str())?)
                }
                None => None,
            };
            dcg_availability = authorization
                .as_ref()
                .map(|view| view.actions.clone())
                .unwrap_or_default();
            // Every §8.3 ChangePack view, each from the authoritative application
            // API that owns it. Impact and representation are per-revision and
            // are absent — not empty — when nothing has been sealed yet: an
            // empty impact report would claim a revision touches nothing.
            let newest = change.revisions.first().map(|revision| revision.id.clone());
            let (impact, coverage, representation) = match &newest {
                Some(revision) => (
                    Some(app.dcg_impact(root, revision.as_str())?),
                    Some(app.dcg_coverage(root, revision.as_str())?),
                    app.dcg_representation(root, revision.as_str())?,
                ),
                None => (None, None, None),
            };
            (
                json!({
                    "project": entry,
                    "summary": change,
                    "intent": app.dcg_change_pack_intent(root, change_pack_id)?,
                    "scope": app.dcg_change_pack_scope(root, change_pack_id)?,
                    "revisions": change.revisions,
                    "impact": impact,
                    "coverage": coverage,
                    "representations": representation,
                    "authorization": authorization,
                    "receipts": app.dcg_change_pack_receipts(root, change_pack_id)?,
                    "recovery": app.dcg_change_pack_recovery(root, change_pack_id)?,
                    "activity": app.canonical_events(root)?.into_iter()
                        .filter(|event| event.subject.as_deref() == Some(change_pack_id))
                        .collect::<Vec<_>>(),
                }),
                "healthy".into(),
                ModelFreshness::Fresh,
                false,
            )
        }
        ConsoleScope::Baseline => {
            let workspace_id =
                required_subject_value(requested.subject.workspace_id(), "workspace_id")?;
            let baseline_id =
                required_subject_value(requested.subject.baseline_id(), "baseline_id")?;
            let entry = registry.resolve(workspace_id)?;
            let root = Path::new(&entry.project_path);
            revisions.workspace =
                Some(draft_core::dcg::source_view::WorkspaceRevision::derive(root)?.content_digest);
            // One accepted historical node. Read-only: nothing about a
            // Baseline is mutable, and the acts that produce one live on the
            // ChangePack that was promoted.
            (
                json!({
                    "project": entry,
                    "baseline": app.dcg_baseline_detail(root, baseline_id)?,
                }),
                "healthy".into(),
                ModelFreshness::Fresh,
                true,
            )
        }
    };
    // Read once, from authoritative state, and stamped onto every capability
    // this model issues. A project that has no readable state — a global
    // subject, or a registered path that has gone missing — yields the empty
    // watermark, and a projection depending on any store is then stale on its
    // first invocation rather than quietly permitted.
    let watermark = subject_watermark(app, &requested.subject).unwrap_or_default();
    let actions = issue_action_presentations(
        sessions,
        &session,
        &requested.subject,
        &revisions,
        &watermark,
        read_only,
        &dcg_availability,
    );

    // Capability shortfalls are attached after the actions exist, because a
    // remedy may only point at an action that was actually issued. A frontend
    // then renders the link rather than inferring one.
    let mut content = content;
    let capability_gaps = if requested.subject.scope() == ConsoleScope::Global {
        withheld_capability_gaps(&actions)
    } else {
        Vec::new()
    };
    if let Some(object) = content.as_object_mut() {
        if !capability_gaps.is_empty() {
            object.insert(
                "capability_gaps".into(),
                serde_json::to_value(&capability_gaps).unwrap_or(Value::Array(Vec::new())),
            );
        }
    }

    // A shortfall the user can actually resolve is the next safe thing to do,
    // ahead of whatever else happens to be enabled.
    let next_safe_actions: Vec<NextSafeAction> = capability_gaps
        .iter()
        .filter(|gap| gap.remediation_action_id.is_some())
        .map(|gap| NextSafeAction {
            label: format!("Resolve: {}", gap.reason),
            reason: gap.reason.clone(),
            action_id: gap.remediation_action_id.clone(),
        })
        // What the Change Graph itself says comes next. Named by the server so
        // both frontends point at the same step, rather than each guessing
        // from the records it happens to render.
        .chain(dcg_next_action.map(|next| NextSafeAction {
            label: next.clone(),
            reason: "The next step in this project's Change Graph".into(),
            action_id: None,
        }))
        .chain(
            actions
                .iter()
                .find(|action| action.enabled)
                .map(|action| NextSafeAction {
                    label: action.label.clone(),
                    reason: "Available from the current authoritative lifecycle state".into(),
                    action_id: Some(action.action_id.clone()),
                }),
        )
        .collect();
    Ok(ConsoleReadModel {
        subject: requested.subject.clone(),
        revisions,
        freshness,
        health,
        read_only,
        permission_reason: read_only.then(|| "Current state is inspection-only".into()),
        navigation: draft_ipc::console_application::navigation_for(requested.subject.scope()),
        content,
        actions,
        next_safe_actions,
        evidence_links: Vec::new(),
        operation_links: Vec::new(),
    })
}

fn required_subject_value<'a>(value: Option<&'a str>, name: &str) -> DraftResult<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| DraftError::invalid_config(format!("{name} is required for this scope")))
}

/// One action `draftd` is prepared to offer, before a capability is issued.
///
/// Everything a frontend needs to render an action, and everything the server
/// needs to validate its invocation, is decided here — in one place, by the
/// authority. A frontend adds nothing to this and infers nothing from it.
struct OfferedAction {
    action_id: String,
    label: String,
    target: Option<ActionTarget>,
    inputs: Vec<ActionInputField>,
    /// Whether the action can run right now, decided by `draftd`.
    enabled: bool,
    disabled_reason: Option<String>,
    requires_confirmation: bool,
}

impl OfferedAction {
    fn new(action_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            action_id: action_id.into(),
            label: label.into(),
            target: None,
            inputs: Vec::new(),
            enabled: true,
            disabled_reason: None,
            requires_confirmation: false,
        }
    }

    fn on(mut self, kind: ActionTargetKind, id: impl Into<String>) -> Self {
        self.target = Some(ActionTarget {
            kind,
            id: id.into(),
        });
        self
    }

    fn taking(mut self, inputs: Vec<ActionInputField>) -> Self {
        self.inputs = inputs;
        self
    }

    fn confirmed(mut self) -> Self {
        self.requires_confirmation = true;
        self
    }

    fn gated(mut self, enabled: bool, reason: &str) -> Self {
        if !enabled {
            self.enabled = false;
            self.disabled_reason = Some(reason.to_string());
        }
        self
    }
}

fn text_input(id: &str, label: &str, required: bool) -> ActionInputField {
    ActionInputField {
        id: id.into(),
        label: label.into(),
        kind: ActionInputKind::Text { max_length: None },
        required,
        help: None,
    }
}

fn select_input(
    id: &str,
    label: &str,
    options: Vec<SelectOption>,
    required: bool,
) -> ActionInputField {
    ActionInputField {
        id: id.into(),
        label: label.into(),
        kind: ActionInputKind::Select { options },
        required,
        help: None,
    }
}

fn confirmation_input(id: &str, label: &str) -> ActionInputField {
    ActionInputField {
        id: id.into(),
        label: label.into(),
        kind: ActionInputKind::Confirmation,
        required: true,
        help: None,
    }
}

/// The authorization view for the newest revision of each ChangePack.
///
/// Computed by the server so a frontend never derives legality from raw
/// records. A ChangePack with no sealed revision has nothing to authorize yet and
/// simply contributes nothing.
fn dcg_authorizations(
    app: &App,
    root: &Path,
    graph: &draft_core::app::workflow::ProjectWorkflowView,
) -> Vec<draft_core::app::workflow::AuthorizationView> {
    graph
        .change_packs
        .iter()
        .filter_map(|change| {
            let revision = change.revisions.first()?;
            app.dcg_authorization(root, change.change_pack.as_str(), revision.id.as_str())
                .ok()
        })
        .collect()
}

/// Every Change Graph action's shape and declared inputs, ungated.
///
/// Separate from [`graph_actions`] because two questions live here and only
/// one of them depends on state: *what does this action take?* never changes,
/// while *may it run now?* is recomputed each time. A caller re-deriving an
/// issued capability's input contract needs the first and must not be given
/// the second.
fn graph_action_definitions() -> Vec<OfferedAction> {
    graph_actions(
        &[draft_core::app::workflow::ActionAvailability {
            action: "publish".into(),
            available: true,
            reason: None,
        }],
        false,
    )
}

/// The Change Graph actions, gated by the server's own availability answer.
///
/// Each stage is a separate action because each is a separate act. In
/// particular approving and promoting are never one control: a Decision
/// authorizes, and a Promotion is what changes the Baseline.
/// The provider lifecycle acts the Console offers.
///
/// Only `unbind` and `rebind`, and deliberately so. Both take one binding id
/// and nothing else, so there is a narrowest deterministic input for each;
/// both are reversible in the sense that matters — neither destroys history;
/// and both run through the same audited application operation the CLI uses,
/// so a Console unbind is journalled, audited and appended exactly like a
/// terminal one.
///
/// Binding, redefining and reprofiling are absent because each needs a
/// canonical semantics contract, definition or profile document, and §8.3
/// specifies no input syntax for one. Offering a form that half-built such a
/// document would be inventing a second authoring path for an immutable fact.
fn provider_actions(read_only: bool) -> Vec<OfferedAction> {
    let gate =
        |action: OfferedAction| action.gated(!read_only, "Current project state is read-only");
    vec![
        gate(
            OfferedAction::new(
                "project.provider.unbind",
                "Unbind — stops routing new work, keeps every past fact",
            )
            .taking(vec![text_input("binding", "Provider binding", true)])
            .confirmed(),
        ),
        gate(
            OfferedAction::new(
                "project.provider.rebind",
                "Rebind — resume routing through it",
            )
            .taking(vec![text_input("binding", "Provider binding", true)])
            .confirmed(),
        ),
    ]
}

fn graph_actions(
    availability: &[draft_core::app::workflow::ActionAvailability],
    read_only: bool,
) -> Vec<OfferedAction> {
    // An empty availability set means nothing was computed for this subject,
    // so nothing is offered. It does not mean the actions do not exist — see
    // [`graph_action_definitions`], which is what a caller re-deriving an
    // action's declared inputs needs.
    if availability.is_empty() {
        return Vec::new();
    }
    let allows = |name: &str| {
        availability
            .iter()
            .find(|action| action.action == name)
            .map(|action| action.available)
            .unwrap_or(true)
    };
    let reason = |name: &str| {
        availability
            .iter()
            .find(|action| action.action == name)
            .and_then(|action| action.reason.clone())
            .unwrap_or_else(|| "Unavailable in the project's current state".to_string())
    };
    let gate =
        |action: OfferedAction| action.gated(!read_only, "Current project state is read-only");

    vec![
        gate(
            OfferedAction::new("dcg.change_pack.open", "Open a ChangePack").taking(vec![
                text_input("intent", "What the change is for", true),
                text_input("scope", "Resources it may touch (space-separated)", true),
            ]),
        ),
        gate(
            OfferedAction::new("dcg.revision_pack.seal", "Seal a revision")
                .taking(vec![text_input("change_pack_id", "ChangePack", true)]),
        ),
        gate(
            OfferedAction::new("dcg.evidence.record", "Record evidence").taking(vec![text_input(
                "revision_pack_id",
                "RevisionPack",
                true,
            )]),
        ),
        gate(
            OfferedAction::new("dcg.assessment.record", "Assess risk").taking(vec![
                text_input("revision_pack_id", "RevisionPack", true),
                select_input(
                    "risk",
                    "Assessed risk",
                    ["low", "medium", "high", "critical"]
                        .into_iter()
                        .map(|value| SelectOption {
                            value: value.into(),
                            label: value.into(),
                        })
                        .collect(),
                    true,
                ),
            ]),
        ),
        gate(
            OfferedAction::new("dcg.gate.evaluate", "Evaluate the gate").taking(vec![text_input(
                "revision_pack_id",
                "RevisionPack",
                true,
            )]),
        ),
        gate(
            OfferedAction::new("dcg.decision.approve", "Approve — authorizes a promotion")
                .taking(vec![
                    text_input("revision_pack_id", "RevisionPack", true),
                    text_input("gate", "Gate", false),
                ])
                .confirmed(),
        ),
        gate(
            OfferedAction::new("dcg.decision.reject", "Reject")
                .taking(vec![
                    text_input("revision_pack_id", "RevisionPack", true),
                    text_input("reason", "Reason", true),
                ])
                .confirmed(),
        ),
        // `expected_baseline` is required. It is what makes a stale view fail
        // deterministically instead of promoting onto a parent nobody judged
        // the work against.
        gate(
            OfferedAction::new("dcg.promote", "Promote — changes the accepted Baseline")
                .taking(vec![
                    text_input("change_pack_id", "ChangePack", true),
                    text_input("revision_pack_id", "RevisionPack", true),
                    text_input("decision", "Decision", true),
                    text_input("gate", "Gate", true),
                    text_input(
                        "expected_baseline",
                        "Baseline you believe is accepted",
                        true,
                    ),
                    confirmation_input("confirm", "This changes what the project accepts"),
                ])
                .confirmed(),
        ),
        gate(
            OfferedAction::new("dcg.publish", "Publish — delivers it outside Draft")
                .taking(vec![text_input("purpose", "Purpose", false)])
                .confirmed(),
        )
        .gated(allows("publish"), &reason("publish")),
        // Offered separately from publishing, and confirmed, because it is a
        // separate decision: granting the authority to announce work outside
        // Draft is not the same act as announcing it.
        gate(
            OfferedAction::new(
                "dcg.publication.grant",
                "Grant publish authority — permits delivery outside Draft",
            )
            .taking(vec![confirmation_input(
                "confirm",
                "This permits this project to send work outside Draft",
            )])
            .confirmed(),
        ),
    ]
}

/// Extension-management actions, one set per installed extension and source.
///
/// Eligibility is computed here from authoritative state — a grant that binds,
/// a pending authorization, a source's enabled flag — and never by a frontend
/// reading raw records and deciding for itself.
fn extension_actions() -> Vec<OfferedAction> {
    let mut actions = Vec::new();

    let sources = draft_extension_service::catalog::source_list().unwrap_or_default();
    let source_options: Vec<SelectOption> = sources
        .iter()
        .map(|source| SelectOption {
            value: source.source.id.clone(),
            label: source.source.id.clone(),
        })
        .collect();

    actions.push(
        OfferedAction::new("extension.source.add", "Add extension source").taking(vec![
            text_input("source_id", "Source id", true),
            text_input("location", "Catalog location", true),
        ]),
    );

    if !source_options.is_empty() {
        actions.push(
            OfferedAction::new("extension.install", "Install extension").taking(vec![
                text_input("extension_id", "Extension id", true),
                select_input("source_id", "Source", source_options.clone(), true),
                text_input("version", "Version", false),
            ]),
        );
    }

    for status in &sources {
        let id = status.source.id.clone();
        actions.push(
            OfferedAction::new("extension.source.refresh", "Refresh source")
                .on(ActionTargetKind::ExtensionSource, &id)
                .gated(status.source.enabled, "A disabled source is not refreshed")
                .gated(
                    status.trusted,
                    "Accept this source's signed root before refreshing",
                ),
        );
        actions.push(
            OfferedAction::new("extension.source.enable", "Enable source")
                .on(ActionTargetKind::ExtensionSource, &id)
                .gated(!status.source.enabled, "The source is already enabled"),
        );
        actions.push(
            OfferedAction::new("extension.source.disable", "Disable source")
                .on(ActionTargetKind::ExtensionSource, &id)
                .gated(status.source.enabled, "The source is already disabled"),
        );
        actions.push(
            OfferedAction::new("extension.source.trust", "Trust source root")
                .on(ActionTargetKind::ExtensionSource, &id)
                .taking(vec![
                    text_input("root_json", "Root metadata", true),
                    text_input("fingerprint", "Root fingerprint", true),
                ]),
        );
        actions.push(
            OfferedAction::new("extension.source.remove", "Remove source")
                .on(ActionTargetKind::ExtensionSource, &id)
                .confirmed()
                // A built-in source's trust anchor comes from the build, so it
                // is disabled rather than removed.
                .gated(
                    !status.source.builtin,
                    "Built-in sources are disabled, not removed",
                ),
        );
    }

    // Update All is offered from the authoritative plan, not from "some row
    // looks older". Planning is local and side-effect-free, so deriving this
    // read model never refreshes a source or touches the network.
    // A store that cannot be read offers nothing rather than guessing.
    if let Ok(plan) = draft_extension_service::catalog::plan_updates() {
        let applicable = plan.applicable().count();
        let action =
            OfferedAction::new("extension.update_all", "Update all extensions").confirmed();
        actions.push(match (applicable, plan.blocking_reason()) {
            (0, Some(reason)) => action.gated(false, reason),
            (0, None) => action.gated(false, "Every installed extension is up to date"),
            _ => action,
        });
    }

    for installed in draft_extension_service::extension::list().unwrap_or_default() {
        let view = match draft_extension_service::authorization::view(installed.clone()) {
            Ok(view) => view,
            Err(_) => continue,
        };
        let id = view.installed.id().to_string();
        let id = id.as_str();
        let pending = view
            .pending_authorization
            .as_ref()
            .map(|pending| pending.missing_permissions.clone())
            .unwrap_or_default();

        actions.push(
            OfferedAction::new("extension.enable", "Enable extension")
                .on(ActionTargetKind::Extension, id)
                .gated(!view.installed.enabled, "The extension is already enabled"),
        );
        actions.push(
            OfferedAction::new("extension.disable", "Disable extension")
                .on(ActionTargetKind::Extension, id)
                .gated(view.installed.enabled, "The extension is already disabled"),
        );
        actions.push(
            OfferedAction::new("extension.update", "Update extension")
                .on(ActionTargetKind::Extension, id)
                .taking(vec![text_input("version", "Version", false)])
                .gated(
                    view.installed.update_source_id().is_some(),
                    "No source is recorded for this installation",
                ),
        );
        actions.push(
            OfferedAction::new("extension.authorize", "Authorize capability")
                .on(ActionTargetKind::PendingAuthorization, id)
                .taking(vec![confirmation_input(
                    "acknowledged",
                    &format!(
                        "Grant {} to this exact build",
                        if pending.is_empty() {
                            "the declared permissions".to_string()
                        } else {
                            pending
                                .iter()
                                .map(|permission| permission.as_str().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                )])
                .gated(
                    !pending.is_empty(),
                    "Nothing is awaiting authorization for this build",
                ),
        );
        actions.push(
            OfferedAction::new("extension.revoke", "Revoke authorization")
                .on(ActionTargetKind::Grant, id)
                .confirmed()
                .gated(
                    !view.authorized_permissions.is_empty(),
                    "Nothing is currently authorized for this build",
                ),
        );
        actions.push(
            OfferedAction::new("extension.uninstall", "Uninstall extension")
                .on(ActionTargetKind::Extension, id)
                .confirmed(),
        );
    }

    actions
}

/// A capability shortfall together with what would resolve it.
///
/// The remedy is chosen here, by the authority that knows the state. A
/// frontend renders `reason` and, when there is one, invokes the action named
/// by `remediation_action_id` — it never works out from a capability kind, an
/// extension id or a rendered label which action is the right one.
#[derive(Debug, Clone, Serialize)]
struct CapabilityGapPresentation {
    gap_id: String,
    capability: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    extension_id: Option<String>,
    /// What kind of remedy this is: `authorize`, `install`, `enable`, and so
    /// on. Presentation-neutral and stable.
    remedy: String,
    /// An action in the *currently issued* set that performs the remedy, if
    /// one exists in this scope. Absent when the remedy is real but no action
    /// here can perform it — Draft cannot know which package fills a gap, so
    /// there is legitimately nothing to invoke.
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation_action_id: Option<String>,
    /// Where the remedy lives, when it is not here.
    #[serde(skip_serializing_if = "Option::is_none")]
    remedy_scope: Option<String>,
}

/// Capability shortfalls in the current installation, with their remedies.
///
/// A withheld capability is an installed extension that declares something it
/// is not authorized to do. The remedy is authorizing it, and that action is
/// issued in this same Global model — so the link is to a real, eligible,
/// currently-issued action rather than a guess.
fn withheld_capability_gaps(actions: &[ActionPresentation]) -> Vec<CapabilityGapPresentation> {
    let contributions = match draft_extension_service::contributions::resolve() {
        Ok(contributions) => contributions,
        Err(_) => return Vec::new(),
    };
    contributions
        .withheld
        .iter()
        .map(|withheld| {
            let authorize = actions.iter().find(|action| {
                action.action_id == "extension.authorize"
                    && action.enabled
                    && action
                        .target
                        .as_ref()
                        .is_some_and(|target| target.id == withheld.extension_id)
            });
            CapabilityGapPresentation {
                gap_id: withheld.gap_id(),
                capability: withheld.capability.as_str().to_string(),
                reason: format!(
                    "{} declares {} but is not authorized to use it",
                    withheld.extension_id,
                    withheld.capability.as_str()
                ),
                extension_id: Some(withheld.extension_id.clone()),
                remedy: "authorize".into(),
                remediation_action_id: authorize.map(|action| action.action_id.clone()),
                remedy_scope: None,
            }
        })
        .collect()
}

/// The authoritative watermark for whatever project a subject names.
///
/// `None` for a global subject: there is no project to read, and the
/// installation's own revision is validated where an extension action runs.
fn subject_watermark(app: &App, subject: &ConsoleSubject) -> Option<ReadModelWatermark> {
    let workspace_id = subject.workspace_id()?;
    let entry = draft_core::project::registry::ProjectRegistry::global()
        .ok()?
        .resolve(workspace_id)
        .ok()?;
    app.read_model_watermark(Path::new(&entry.project_path), subject.change_pack_id())
        .ok()
}

/// The projection an action's offer rests on.
///
/// Every offered action names one, so what would invalidate the offer follows
/// from the action rather than from a rule somebody has to remember to apply
/// at the call site. Getting this wrong in the permissive direction is the
/// failure mode: an action that declares fewer dependencies than it has can
/// be invoked against state it never saw.
fn projection_for(action_id: &str) -> Projection {
    match action_id {
        // Delivery rests on the accepted Baseline, the route the binding
        // currently selects, and whether an attempt is already in flight.
        id if id.starts_with("dcg.publish") || id.starts_with("dcg.publication.") => {
            Projection::PublicationEligibility
        }
        // Extension management is about the installation, whose registry
        // revision is validated separately against authoritative state.
        id if id.starts_with("extension.") => Projection::InstallationState,
        // A provider act depends on the binding store and on nothing else.
        // Naming `ChangePackEligibility` here would make every unbind stale the
        // moment an unrelated ChangePack sealed a revision — correct-by-accident
        // and wrong in the direction that annoys rather than the direction
        // that is unsafe, but wrong either way: the offer never read those
        // stores.
        id if id.starts_with("project.provider.") => Projection::ProviderCatalog,
        // Everything else in the graph is a step in the one chain, and every
        // step's availability rests on the ChangePack, the project, the evidence
        // and the judgements made about it.
        _ => Projection::ChangePackEligibility,
    }
}

#[allow(clippy::too_many_arguments)]
fn issue_action_presentations(
    sessions: &SessionManager,
    session: &ApplicationSession,
    subject: &ConsoleSubject,
    revisions: &CanonicalRevisions,
    watermark: &ReadModelWatermark,
    read_only: bool,
    dcg_availability: &[draft_core::app::workflow::ActionAvailability],
) -> Vec<ActionPresentation> {
    let offered = match subject.scope() {
        ConsoleScope::ChangePack => graph_actions(dcg_availability, read_only),
        // Extension management belongs to the whole installation, not to one
        // project. Offering it here is what lets the TUI reach the same
        // workflows the CLI and the browser already have.
        ConsoleScope::Global => extension_actions(),
        ConsoleScope::Project => {
            let mut offered = graph_actions(dcg_availability, read_only);
            offered.extend(provider_actions(read_only));
            offered
        }
        // A Baseline is an accepted historical node. Nothing about it is
        // mutable, and the acts that produced it belong to the ChangePack that was
        // promoted — offering them here would imply a Baseline can be edited.
        ConsoleScope::Baseline => Vec::new(),
    };
    if offered.is_empty() {
        return Vec::new();
    }

    // Read the deadline from the session manager's own clock: issuing and
    // consuming must agree about what time it is, or a capability could be
    // stamped against one clock and judged against another.
    let expires_at_unix_ms = sessions.now_unix_ms() + CONSOLE_ACTION_TTL_MS;
    let capabilities_supported = session
        .capabilities
        .iter()
        .any(|capability| capability == "action_capabilities");

    offered
        .into_iter()
        .map(|action| {
            let enabled = capabilities_supported && action.enabled;
            let input_contract_digest = input_contract_digest(&action.inputs);
            let invocation_capability = enabled.then(|| {
                sessions.issue_action(ActionBinding {
                    application_session_id: session.id.clone(),
                    principal: session.principal.clone(),
                    workspace_id: subject.workspace_id().map(ToOwned::to_owned),
                    change_pack_id: subject.change_pack_id().map(ToOwned::to_owned),
                    action_id: action.action_id.clone(),
                    target: action.target.clone(),
                    workspace_revision: revisions.workspace.clone(),
                    change_revision: revisions.change_pack.clone(),
                    registry_revision: Some(revisions.registry),
                    input_contract_digest: input_contract_digest.clone(),
                    precondition: RequestPrecondition {
                        projection: projection_for(&action.action_id),
                        watermark: watermark.clone(),
                    },
                    expires_at_unix_ms,
                })
            });
            ActionPresentation {
                action_id: action.action_id,
                label: action.label,
                enabled,
                disabled_reason: (!enabled).then(|| {
                    if !capabilities_supported {
                        "The client did not negotiate action capabilities".into()
                    } else {
                        action
                            .disabled_reason
                            .unwrap_or_else(|| "Unavailable in the current state".into())
                    }
                }),
                invocation_capability,
                requires_confirmation: action.requires_confirmation,
                expires_at_unix_ms: enabled.then_some(expires_at_unix_ms),
                inputs: action.inputs,
                input_contract_digest,
                target: action.target,
            }
        })
        .collect()
}

/// Check submitted arguments against the contract the action declared.
///
/// The frontend's own checks are a convenience; this is the decision. Anything
/// the action did not declare, anything required and missing, anything of the
/// wrong shape, and any select value outside the server's own option set is
/// refused here.
fn validate_arguments(
    inputs: &[ActionInputField],
    arguments: &BTreeMap<String, Value>,
) -> DraftResult<()> {
    for name in arguments.keys() {
        if !inputs.iter().any(|input| &input.id == name) {
            return Err(DraftError::invalid_config(format!(
                "action does not declare an input named '{name}'"
            )));
        }
    }
    for input in inputs {
        let Some(value) = arguments.get(&input.id) else {
            if input.required {
                return Err(DraftError::invalid_config(format!(
                    "action input '{}' is required",
                    input.id
                )));
            }
            continue;
        };
        match &input.kind {
            ActionInputKind::Text { max_length } => {
                let text = value.as_str().ok_or_else(|| {
                    DraftError::invalid_config(format!("action input '{}' expects text", input.id))
                })?;
                if let Some(limit) = max_length {
                    if text.chars().count() > *limit as usize {
                        return Err(DraftError::invalid_config(format!(
                            "action input '{}' exceeds its {limit}-character limit",
                            input.id
                        )));
                    }
                }
            }
            ActionInputKind::Select { options } => {
                let chosen = value.as_str().ok_or_else(|| {
                    DraftError::invalid_config(format!(
                        "action input '{}' expects one of its declared values",
                        input.id
                    ))
                })?;
                if !options.iter().any(|option| option.value == chosen) {
                    return Err(DraftError::invalid_config(format!(
                        "action input '{}' does not offer the value '{chosen}'",
                        input.id
                    )));
                }
            }
            ActionInputKind::Boolean => {
                if !value.is_boolean() {
                    return Err(DraftError::invalid_config(format!(
                        "action input '{}' expects true or false",
                        input.id
                    )));
                }
            }
            ActionInputKind::Confirmation => {
                if value.as_bool() != Some(true) {
                    return Err(DraftError::invalid_config(format!(
                        "action input '{}' must be acknowledged",
                        input.id
                    )));
                }
            }
        }
    }
    Ok(())
}

fn console_action_response(
    store: &ServiceStore,
    sessions: &SessionManager,
    app: &App,
    req: &Request,
    id: String,
) -> Response {
    match console_action_invoke(store, sessions, app, req) {
        Ok(result) => Response::ok(id, serde_json::to_value(result).unwrap_or(Value::Null)),
        Err(ConsoleError::Draft(error)) if error.kind == DraftErrorKind::ConflictDetected => {
            let expected_revisions = req
                .params
                .get("expected_revisions")
                .cloned()
                .unwrap_or(Value::Null);
            let registry_revision = draft_core::project::registry::ProjectRegistry::global()
                .and_then(|registry| registry.envelope())
                .map(|envelope| envelope.revision)
                .unwrap_or_default();
            Response::err(
                id,
                ErrorObject {
                    code: "STALE_CONSOLE_ACTION".into(),
                    message: error.message,
                    details: json!({
                        "current_revisions": { "registry": registry_revision },
                        "expected_revisions": expected_revisions,
                        "invalidation_targets": ["active_scope", "actions"],
                        "refresh_guidance": "Refresh the affected scope and act again using a newly issued descriptor",
                    }),
                },
            )
        }
        Err(error) => Response::err(id, error.into_error_object()),
    }
}

/// The inputs the named action currently declares.
///
/// Re-derived from live state rather than trusted from the client, so a
/// capability issued against an older option set no longer matches its digest.
/// Only the inputs are needed here: whether the action was *allowed* is what
/// issuing the capability decided, and the revision checks cover staleness.
fn declared_inputs(binding: &ActionBinding) -> Vec<ActionInputField> {
    let offered = if binding.action_id.starts_with("extension.") {
        extension_actions()
    } else if binding.action_id.starts_with("project.provider.") {
        // Ungated, for the same reason the graph definitions are: this
        // re-derives the declared *inputs*, and whether the act was allowed is
        // what issuing the capability already decided.
        provider_actions(false)
    } else {
        // The definitions, not the gated offer: this re-derives the declared
        // *inputs*, and whether the action was allowed is what issuing the
        // capability already decided. Gating here would return nothing and
        // make every argument fail its contract digest.
        graph_action_definitions()
    };
    offered
        .into_iter()
        .find(|action| action.action_id == binding.action_id && action.target == binding.target)
        .map(|action| action.inputs)
        .unwrap_or_default()
}

/// Run an extension-management action through its authoritative operation.
///
/// The action is turned back into the exact `extension.*` request the CLI and
/// the Console gateway already send, and dispatched through the same handler.
/// There is deliberately no second implementation of any of these mutations.
fn invoke_extension_action(
    store: &ServiceStore,
    sessions: &SessionManager,
    binding: &ActionBinding,
    invocation: &ConsoleActionInvocation,
) -> DraftResult<ConsoleActionResult> {
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    let current_registry = registry.envelope()?.revision;
    if binding.registry_revision != Some(current_registry)
        || invocation.expected_revisions.registry != current_registry
    {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "installation state changed; refresh and act again",
        ));
    }

    let mut params = serde_json::Map::new();
    if let Some(target) = &binding.target {
        let key = match target.kind {
            ActionTargetKind::ExtensionSource => "source_id",
            // An extension, a pending authorization and a grant are all
            // addressed by the extension they belong to.
            ActionTargetKind::Extension
            | ActionTargetKind::PendingAuthorization
            | ActionTargetKind::Grant => "extension_id",
        };
        params.insert(key.to_string(), Value::String(target.id.clone()));
    }
    for (name, value) in &invocation.arguments {
        // A confirmation is the user acknowledging the action, not an argument
        // the daemon method takes.
        if name == "acknowledged" {
            continue;
        }
        params.insert(name.clone(), value.clone());
    }
    params.insert(
        "operation_id".to_string(),
        Value::String(invocation.operation_id.clone()),
    );

    // Authorizing grants exactly what the artifact is currently waiting on,
    // read here from authoritative state. The client acknowledged a list it
    // was shown; it does not get to send one back, because a client-supplied
    // permission set would be a client deciding its own authorization.
    if binding.action_id == "extension.authorize" {
        let extension_id = params
            .get("extension_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DraftError::invalid_config("authorizing is not bound to an extension")
            })?;
        let view = draft_extension_service::authorization::view(
            draft_extension_service::extension::show(extension_id)?,
        )?;
        let pending = view
            .pending_authorization
            .as_ref()
            .map(|pending| pending.missing_permissions.clone())
            .unwrap_or_default();
        if pending.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "nothing is awaiting authorization for this build; refresh and act again",
            ));
        }
        params.insert(
            "permissions".to_string(),
            Value::Array(
                pending
                    .iter()
                    .map(|permission| Value::String(permission.as_str().to_string()))
                    .collect(),
            ),
        );
    }

    let request = Request::new(
        &invocation.operation_id,
        &binding.action_id,
        Value::Object(params),
    );
    // `dispatch_inner`, not `dispatch`: `console.action.invoke` is itself a
    // mutation and was already recorded in the operation log under the
    // client's operation id, so this must not open a second operation under
    // the same id. ChangePack actions reach their App methods the same way.
    let response = dispatch_inner(store, sessions, request);
    let result = match (response.result, response.error) {
        (Some(result), _) => result,
        (None, Some(error)) => {
            return Err(DraftError::new(DraftErrorKind::IpcError, error.message))
        }
        (None, None) => Value::Null,
    };

    Ok(ConsoleActionResult {
        operation_id: invocation.operation_id.clone(),
        revisions: CanonicalRevisions {
            registry: registry.envelope()?.revision,
            workspace: None,
            change_pack: None,
            policy: None,
        },
        result,
        invalidation_targets: vec!["extensions".into(), "actions".into()],
    })
}

/// Run one Change Graph action through the application boundary.
///
/// Every arm is one `App` call. Nothing here decides whether the action is
/// legal — the domain does, under its own guards, and a check repeated here
/// would be a second rule to keep in step with the first.
fn invoke_graph_action(
    app: &App,
    binding: &ActionBinding,
    invocation: &ConsoleActionInvocation,
    workspace_id: &str,
) -> DraftResult<ConsoleActionResult> {
    let entry = draft_core::project::registry::ProjectRegistry::global()?.resolve(workspace_id)?;
    let root = Path::new(&entry.project_path);
    let argument = |name: &str| {
        invocation
            .arguments
            .get(name)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    let required = |name: &str| {
        argument(name).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::Validation,
                format!("'{name}' is required for {}", binding.action_id),
            )
        })
    };

    let result = match binding.action_id.as_str() {
        "dcg.change_pack.open" => {
            let scope: Vec<String> = required("scope")?
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect();
            to_value(app.dcg_open_change_pack(root, &required("intent")?, &scope)?)?
        }
        "dcg.revision_pack.seal" => to_value(app.dcg_seal(root, &required("change_pack_id")?)?)?,
        // Straight through the same application operation the CLI calls, which
        // commits via `commit_audited_mutation` — MutationJournal, AuditFact
        // and Activity, with no Console-specific path around any of them.
        "project.provider.unbind" => to_value(app.provider_unbind(root, &required("binding")?)?)?,
        "project.provider.rebind" => to_value(app.provider_rebind(root, &required("binding")?)?)?,
        "dcg.evidence.record" => to_value(app.dcg_verify(root, &required("revision_pack_id")?)?)?,
        "dcg.assessment.record" => to_value(
            app.dcg_assess(
                root,
                &required("revision_pack_id")?,
                &required("risk")?,
                argument("rationale")
                    .as_deref()
                    .unwrap_or("assessed through the console"),
            )?,
        )?,
        "dcg.gate.evaluate" => {
            to_value(app.dcg_evaluate_gate(root, &required("revision_pack_id")?, &[])?)?
        }
        "dcg.decision.approve" => to_value(app.dcg_decide(
            root,
            &required("revision_pack_id")?,
            argument("gate").as_deref(),
            true,
            None,
        )?)?,
        "dcg.decision.reject" => to_value(app.dcg_decide(
            root,
            &required("revision_pack_id")?,
            None,
            false,
            Some(&required("reason")?),
        )?)?,
        "dcg.promote" => to_value(app.dcg_promote(
            root,
            &required("change_pack_id")?,
            &required("revision_pack_id")?,
            &required("decision")?,
            &required("gate")?,
            Some(required("expected_baseline")?.as_str()),
        )?)?,
        // The operation id is the attempt identity, so a client that retries
        // this invocation converges on what its attempt concluded rather than
        // delivering a second time.
        "dcg.publication.grant" => to_value(app.dcg_grant_publish(root)?)?,
        "dcg.publish" => to_value(
            app.dcg_publish(
                root,
                None,
                argument("purpose")
                    .as_deref()
                    .unwrap_or("draft.publish/export"),
                &invocation.operation_id,
                argument("retry_authorization").as_deref(),
            )?,
        )?,
        other => {
            return Err(DraftError::new(
                DraftErrorKind::IpcError,
                format!("unknown Change Graph action: {other}"),
            ))
        }
    };

    Ok(ConsoleActionResult {
        operation_id: invocation.operation_id.clone(),
        revisions: CanonicalRevisions {
            registry: draft_core::project::registry::ProjectRegistry::global()?
                .envelope()?
                .revision,
            workspace: draft_core::dcg::source_view::WorkspaceRevision::derive(root)
                .ok()
                .map(|revision| revision.content_digest),
            change_pack: None,
            policy: None,
        },
        result,
        invalidation_targets: vec!["project".into(), "graph".into(), "events".into()],
    })
}

/// Refuse an invocation whose descriptor no longer matches authoritative state.
///
/// Runs before any argument reaches a mutation. What it catches is precisely
/// the race a self-consistent request hides: the offer was computed, somebody
/// else committed, and the caller is now acting on a screen that describes a
/// project that no longer exists.
///
/// A descriptor bound to no project is left to its own authority check — an
/// extension action's registry revision is compared against current
/// installation state where it runs, and duplicating that here would give one
/// fact two owners.
fn revalidate_binding(app: &App, binding: &ActionBinding) -> DraftResult<()> {
    let Some(workspace_id) = binding.workspace_id.as_deref() else {
        return Ok(());
    };
    let entry = draft_core::project::registry::ProjectRegistry::global()?.resolve(workspace_id)?;
    let outcome = app.revalidate_precondition(
        Path::new(&entry.project_path),
        binding.change_pack_id.as_deref(),
        &binding.precondition,
    )?;
    match outcome {
        draft_ipc::console_application::ActionOutcome::Proceed => Ok(()),
        draft_ipc::console_application::ActionOutcome::Stale { moved } => Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!(
                "this action was offered against state that has since moved ({}); refresh and \
                 act again",
                moved
                    .iter()
                    .map(describe_movement)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        draft_ipc::console_application::ActionOutcome::Conflict { subject, detail } => {
            Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("{subject}: {detail}"),
            ))
        }
    }
}

/// Name what moved, so a refusal says which fact changed rather than "stale".
fn describe_movement(reason: &draft_ipc::console_application::StaleReason) -> String {
    use draft_ipc::console_application::StaleReason as Reason;
    match reason {
        Reason::ProjectControlAdvanced { seen, current } => {
            format!("the project control record advanced from generation {seen} to {current}")
        }
        Reason::ActivityAdvanced => "the Activity Ledger advanced".to_string(),
        Reason::StoreAdvanced {
            store,
            seen,
            current,
        } => format!("{store:?} advanced from {seen} to {current}"),
        Reason::StoreUnknown { store } => format!("{store:?} no longer reports a generation"),
        Reason::StoreUndeclared { store } => {
            format!("the descriptor recorded no {store:?} generation to compare")
        }
    }
}

fn console_action_invoke(
    store: &ServiceStore,
    sessions: &SessionManager,
    app: &App,
    req: &Request,
) -> ConsoleResult<ConsoleActionResult> {
    let invocation: ConsoleActionInvocation = serde_json::from_value(req.params.clone())
        .map_err(|error| DraftError::invalid_config(error.to_string()))?;
    let session = sessions
        .application(&invocation.application_session_id)
        .ok_or(ConsoleError::SessionUnknown(
            "Console application session is missing or was replaced; reconnect and act again",
        ))?;
    let binding = sessions
        .consume_action(
            &invocation.invocation_capability,
            &invocation.application_session_id,
            &session.principal,
        )
        .map_err(|message| DraftError::new(DraftErrorKind::ConflictDetected, message))?;
    if binding.workspace_revision != invocation.expected_revisions.workspace
        || binding.change_revision != invocation.expected_revisions.change_pack
    {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "action descriptor and expected revisions do not match; refresh and act again",
        )
        .into());
    }

    // The check above only proves the client sent back what it was given. This
    // one re-reads authoritative state and judges the descriptor against it,
    // which is the only way to catch a project that moved between the offer
    // and the invocation (§2.56).
    revalidate_binding(app, &binding)?;

    // The arguments are checked against the contract this capability was
    // issued under, and that contract is re-derived from current state. A
    // client holding a stale select-option set fails here rather than acting
    // on a value the server no longer offers.
    let inputs = declared_inputs(&binding);
    if binding.input_contract_digest != input_contract_digest(&inputs) {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "the action's inputs changed since this descriptor was issued; refresh and act again",
        )
        .into());
    }
    validate_arguments(&inputs, &invocation.arguments)?;

    // Extension management is bound to an entity rather than a ChangePack, and runs
    // through the same authoritative `extension.*` operations the CLI and the
    // browser already use.
    if binding.action_id.starts_with("extension.") {
        return invoke_extension_action(store, sessions, &binding, &invocation)
            .map_err(ConsoleError::from);
    }

    let workspace_id = binding
        .workspace_id
        .as_deref()
        .ok_or_else(|| DraftError::invalid_config("domain action is not bound to a workspace"))?;

    // Change Graph and provider actions are bound to a project, so they
    // resolve before the `change_pack_id` requirement below. Both run through the
    // same invoker, which calls the authoritative application operation — the
    // one that journals, audits and appends.
    if binding.action_id.starts_with("dcg.") || binding.action_id.starts_with("project.provider.") {
        return invoke_graph_action(app, &binding, &invocation, workspace_id)
            .map_err(ConsoleError::from);
    }

    // Nothing else is a Console action. The Change Graph path above is the
    // only domain one, and an id that reaches here names an action this build
    // does not issue.
    Err(DraftError::new(
        DraftErrorKind::IpcError,
        format!(
            "unknown backend-issued Console action: {}",
            binding.action_id
        ),
    )
    .into())
}

fn console_watch(sessions: &SessionManager, req: &Request) -> ConsoleResult<ConsoleWatchEvent> {
    let watch: ConsoleWatchRequest = serde_json::from_value(req.params.clone())
        .map_err(|error| DraftError::invalid_config(error.to_string()))?;
    let session = sessions
        .application(&watch.application_session_id)
        .ok_or(ConsoleError::SessionUnknown(
        "Console application session is missing or was replaced; reconnect before resuming watch",
    ))?;
    if !session
        .capabilities
        .iter()
        .any(|capability| capability == "watch_v1")
    {
        return Err(DraftError::new(
            DraftErrorKind::UnsupportedSchema,
            "console.watch was not negotiated for this application session",
        )
        .into());
    }
    let subject = watch
        .subjects
        .into_iter()
        .next()
        .unwrap_or_else(ConsoleSubject::global);
    Ok(ConsoleWatchEvent {
        application_session_id: watch.application_session_id,
        cursor: watch.after_cursor.unwrap_or_default().saturating_add(1),
        kind: "invalidation".into(),
        subject,
        revisions: CanonicalRevisions {
            registry: draft_core::project::registry::ProjectRegistry::global()?
                .envelope()?
                .revision,
            ..CanonicalRevisions::default()
        },
        invalidation_targets: vec!["active_scope".into()],
        payload: None,
    })
}

fn operation_status(store: &ServiceStore, req: &Request) -> DraftResult<ConsoleOperationStatus> {
    let operation_id = string_param(req, "operation_id")?;
    OperationStore::at(store.operations_root())
        .load(&operation_id)?
        .map(console_operation_status)
        .ok_or_else(|| DraftError::not_found(format!("operation '{operation_id}' was not found")))
}

fn operation_cancel(store: &ServiceStore, req: &Request) -> DraftResult<ConsoleOperationStatus> {
    let operation_id = string_param(req, "target_operation_id")?;
    let record = OperationStore::at(store.operations_root()).cancel(&operation_id)?;
    Ok(console_operation_status(record))
}

fn console_operation_status(
    record: draft_core::execution::operation::OperationRecord,
) -> ConsoleOperationStatus {
    let phase = match record.phase {
        draft_core::execution::operation::OperationPhase::Prepared => {
            ConsoleOperationPhase::Prepared
        }
        draft_core::execution::operation::OperationPhase::Running => ConsoleOperationPhase::Running,
        draft_core::execution::operation::OperationPhase::Finalizing => {
            ConsoleOperationPhase::Finalizing
        }
        draft_core::execution::operation::OperationPhase::Completed => {
            ConsoleOperationPhase::Completed
        }
        draft_core::execution::operation::OperationPhase::FailedBeforeFinalization => {
            ConsoleOperationPhase::FailedBeforeFinalization
        }
        draft_core::execution::operation::OperationPhase::CancelledBeforeFinalization => {
            ConsoleOperationPhase::CancelledBeforeFinalization
        }
        draft_core::execution::operation::OperationPhase::SafelyRetryable => {
            ConsoleOperationPhase::SafelyRetryable
        }
        draft_core::execution::operation::OperationPhase::ReconciliationRequired => {
            ConsoleOperationPhase::ReconciliationRequired
        }
    };
    ConsoleOperationStatus {
        operation_id: record.operation_id.to_string(),
        phase,
        status: format!("{:?}", record.status).to_lowercase(),
        completed: None,
        total: None,
        cancellation_allowed: record.finalization_started_at.is_none()
            && !matches!(
                record.status,
                OperationStatus::Completed | OperationStatus::Cancelled
            ),
        retry_allowed: record.phase
            == draft_core::execution::operation::OperationPhase::SafelyRetryable,
        finalization_started_at: record
            .finalization_started_at
            .map(|value| value.to_rfc3339()),
        target_identity: record.target_identity,
        result: record.result,
        error: record.error,
        recovery_guidance: record.recovery_guidance,
    }
}

/// How long an issued Console action capability stays valid.
///
/// Short on purpose: a descriptor names the exact authoritative state it was
/// issued against, so a stale one should be re-fetched rather than replayed.
pub const CONSOLE_ACTION_TTL_MS: i64 = 30_000;

fn console_overview(store: &ServiceStore, app: &App) -> DraftResult<Value> {
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    let projects = registry.list()?;
    let issues = registry.inspect()?;
    let mut summaries = Vec::new();
    for project in projects.iter().take(100) {
        let path = Path::new(&project.project_path);
        let status = if !path.exists() {
            json!({ "freshness": "unavailable", "error": { "code": "PATH_MISSING", "message": "project path is unavailable" } })
        } else {
            match app.status(path) {
                Ok(status) => {
                    json!({ "freshness": "fresh", "scanned_at": chrono::Utc::now(), "status": status })
                }
                Err(error) => {
                    json!({ "freshness": "corrupt", "scanned_at": chrono::Utc::now(), "error": { "code": error.code(), "message": error.message } })
                }
            }
        };
        summaries.push(json!({ "project": project, "summary": status }));
    }
    Ok(json!({
        "schema_version": draft_core::contracts::current_version(draft_core::contracts::ContractId::ConsoleOverview),
        "daemon": { "connected": true, "version": draft_core::DRAFT_VERSION },
        "projects": summaries,
        "project_count": projects.len(),
        "registry": { "issues": issues, "freshness": "fresh" },
        "jobs": store.list_jobs()?,
        "generated_at": chrono::Utc::now(),
    }))
}

fn console_project(app: &App, req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "workspace_id")?;
    let entry = draft_core::project::registry::ProjectRegistry::global()?.resolve(&workspace_id)?;
    let root = Path::new(&entry.project_path);
    let revision = draft_core::dcg::source_view::WorkspaceRevision::derive(root)?;
    let changes = console_change_pack_summaries(app, root)?;
    Ok(json!({
        "project": entry,
        "revision": revision,
        "status": app.status(root)?,
        "tasks": app.task_list(root)?,
        "change_packs": changes,
        "inbox": app.inbox(root)?,
    }))
}

fn console_change_pack_summaries(app: &App, root: &Path) -> DraftResult<Vec<Value>> {
    app.dcg_change_packs(root)?
        .into_iter()
        .map(|change| {
            Ok(json!({
                "change_pack_id": change.change_pack,
                "lifecycle": change.lifecycle,
                "revisions": change.revisions.len(),
                "latest_revision": change.revisions.first().map(|r| r.id.to_string()),
            }))
        })
        .collect()
}

fn console_inbox(app: &App) -> DraftResult<Value> {
    let projects = draft_core::project::registry::ProjectRegistry::global()?.list()?;
    let mut results = Vec::new();
    for project in projects.into_iter().take(100) {
        let scanned_at = chrono::Utc::now();
        match app.inbox(Path::new(&project.project_path)) {
            Ok(items) => results.push(json!({ "workspace_id": project.workspace_id, "freshness": "fresh", "scanned_at": scanned_at, "items": items })),
            Err(error) => results.push(json!({ "workspace_id": project.workspace_id, "freshness": "unavailable", "scanned_at": scanned_at, "items": [], "error": { "code": error.code(), "message": error.message } })),
        }
    }
    let notifications =
        draft_core::execution::notification::NotificationStore::global()?.list(false)?;
    Ok(
        json!({ "projects": results, "notifications": notifications, "generated_at": chrono::Utc::now() }),
    )
}

fn console_doctor(app: &App) -> DraftResult<Value> {
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    Ok(json!({
        "global": app.doctor_global()?,
        "registry_issues": registry.inspect()?,
        "projects": registry.list()?,
        "generated_at": chrono::Utc::now(),
    }))
}

fn console_settings(app: &App) -> DraftResult<Value> {
    let home = draft_core::project::home::DraftGlobalStore::locate()?;
    draft_core::trust::identity::reject_retired_profile_state(None)?;
    draft_core::trust::identity::global::reject_retired_actor_profile(&home)?;
    draft_core::project::config::reject_retired_profile_config(&home.config_toml())?;
    let resolver = draft_core::project::config::ConfigResolver::load(
        None,
        Some(home.config_toml().as_path()),
    )?;
    Ok(json!({
        "user": {
            "name": resolver.get("user.name").unwrap_or_else(|| "unknown".into()),
            "email": resolver.get("user.email"),
            "name_source": resolver.source_of("user.name").unwrap_or("fallback"),
            "email_source": resolver.source_of("user.email"),
        },
        "security": app.identity_status()?,
        "global_store": home.root(),
        "product_version": draft_core::DRAFT_VERSION,
    }))
}

fn console_search(app: &App, req: &Request) -> DraftResult<Value> {
    let query = string_param(req, "query")?.to_lowercase();
    let limit = req
        .params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50) as usize;
    let mut results = Vec::new();
    for project in draft_core::project::registry::ProjectRegistry::global()?
        .list()?
        .into_iter()
        .take(100)
    {
        let workspace_id = project.workspace_id.clone();
        let root = Path::new(&project.project_path);
        if project.name.to_lowercase().contains(&query)
            || project.workspace_id.to_lowercase().contains(&query)
        {
            results.push(json!({ "kind": "project", "workspace_id": workspace_id, "title": project.name, "subtitle": project.project_path }));
        }
        if results.len() >= limit || !root.exists() {
            continue;
        }
        for task in app.task_list(root)? {
            if task.name.to_lowercase().contains(&query)
                || task.goal.to_lowercase().contains(&query)
            {
                results.push(json!({ "kind": "task", "workspace_id": workspace_id, "id": task.id, "title": task.name, "subtitle": task.goal }));
                if results.len() >= limit {
                    break;
                }
            }
        }
        if results.len() >= limit {
            continue;
        }
        for change in app.dcg_change_packs(root)?.into_iter().take(100) {
            // A ChangePack has no name of its own; what it is for lives in its
            // definition, so its id and its revisions are what can be matched.
            let latest = change
                .revisions
                .first()
                .map(|revision| revision.id.to_string())
                .unwrap_or_default();
            let searchable = format!("{} {latest}", change.change_pack).to_lowercase();
            if searchable.contains(&query) {
                results.push(json!({ "kind": "change_pack", "workspace_id": workspace_id, "id": change.change_pack, "title": change.change_pack, "subtitle": format!("{:?}", change.lifecycle) }));
            }
            // Actions are not searched per ChangePack any more. What may legally
            // be done to a revision is the authorization view's answer, and it
            // depends on evidence, gates and decisions — computing it for every
            // ChangePack to match a substring would be doing the expensive thing
            // for a search box.
            if results.len() >= limit {
                break;
            }
        }
        if results.len() < limit {
            for hit in app.resource_search(root, &query, (limit - results.len()).min(20))? {
                results.push(json!({
                    "kind": "resource",
                    "workspace_id": workspace_id,
                    "id": hit.locator.body,
                    "title": hit.locator.body,
                    "subtitle": format!("line {} · {}", hit.line, hit.preview),
                }));
            }
        }
        if results.len() < limit {
            for event in app.canonical_events(root)?.into_iter().rev().take(50) {
                let searchable = format!(
                    "{} {}",
                    event.kind,
                    event.subject.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                if searchable.contains(&query) {
                    results.push(json!({ "kind": "event", "workspace_id": workspace_id, "id": event.event_id, "title": event.kind, "subtitle": event.subject }));
                }
                if results.len() >= limit {
                    break;
                }
            }
        }
        if results.len() < limit {
            for receipt in app.receipts(root)?.into_iter().rev().take(50) {
                if receipt.to_string().to_lowercase().contains(&query) {
                    let id = receipt
                        .get("id")
                        .or_else(|| receipt.get("receipt_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("receipt");
                    results.push(json!({ "kind": "receipt", "workspace_id": workspace_id, "id": id, "title": id, "subtitle": receipt.get("event_type").or_else(|| receipt.get("kind")) }));
                }
                if results.len() >= limit {
                    break;
                }
            }
        }
    }
    if results.len() < limit
        && ["settings", "user", "profile", "theme"]
            .iter()
            .any(|value| value.contains(&query))
    {
        results.push(json!({ "kind": "setting", "id": "settings", "title": "Settings", "subtitle": "User profile and presentation preferences" }));
    }
    results.truncate(limit);
    Ok(json!({ "query": query, "results": results, "partial": false }))
}

/// The permissions a request asks to grant, by their stable wire names.
/// An app that reads contributed domain knowledge from the extensions the user
/// has installed, enabled and authorized.
fn app() -> App {
    App::with_extension_contributions(std::sync::Arc::new(
        draft_extension_service::contributions::InstalledExtensions,
    ))
}

/// A positive integer parameter, falling back to `default` when absent.
fn usize_param(req: &Request, key: &str, default: usize) -> usize {
    req.params
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
}

fn permission_params(
    req: &Request,
) -> DraftResult<Vec<draft_core::extension::ExtensionPermission>> {
    let declared = req
        .params
        .get("permissions")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::IpcError,
                "missing 'permissions' parameter".to_string(),
            )
        })?;
    declared
        .iter()
        .map(|value| {
            let name = value.as_str().ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::IpcError,
                    "'permissions' entries must be strings".to_string(),
                )
            })?;
            draft_core::extension::ExtensionPermission::parse(name)
                .map_err(draft_core::extension::from_format_error)
        })
        .collect()
}

/// The authorization decision is audited under the caller's operation id, so a
/// grant is traceable to the request that asked for it.
fn operation_id_for(req: &Request) -> draft_core::support::common::OperationId {
    match &req.operation_id {
        Some(operation_id) => draft_core::support::common::OperationId::new(operation_id.clone()),
        None => draft_core::support::common::OperationId::generate(),
    }
}

/// A resource locator parameter.
///
/// Accepts the structured `{scheme, body}` form, and a bare string as the
/// `file` scheme for the common case. Never parses the body: what it means is
/// the owning adapter's business.
fn locator_param(req: &Request, key: &str) -> DraftResult<ResourceLocator> {
    let value = req.params.get(key).ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::IpcError,
            format!("missing '{key}' parameter"),
        )
    })?;
    if let Some(body) = value.as_str() {
        return Ok(ResourceLocator::file(body));
    }
    serde_json::from_value(value.clone()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::IpcError,
            format!("invalid '{key}' resource locator: {error}"),
        )
    })
}

fn string_param(req: &Request, key: &str) -> DraftResult<String> {
    req.params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::IpcError,
                format!("missing '{key}' parameter"),
            )
        })
}

fn update_task(app: &App, path: &Path, req: &Request) -> DraftResult<Value> {
    let status = optional_string_param(req, "status")
        .map(|status| status.replace('-', "_"))
        .map(|status| {
            serde_json::from_value::<draft_core::task::TaskLifecycleStatus>(json!(status))
        })
        .transpose()
        .map_err(|_| DraftError::invalid_config("invalid task lifecycle status"))?;
    let priority = optional_string_param(req, "priority")
        .map(|priority| serde_json::from_value::<draft_core::task::TaskPriority>(json!(priority)))
        .transpose()
        .map_err(|_| DraftError::invalid_config("invalid task priority"))?;
    let due_at = if req
        .params
        .get("clear_due")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some(None)
    } else if let Some(due) = optional_string_param(req, "due") {
        Some(Some(
            chrono::DateTime::parse_from_rfc3339(&due)
                .map_err(|_| DraftError::invalid_config("due must be RFC 3339"))?
                .with_timezone(&chrono::Utc),
        ))
    } else {
        None
    };
    let assignee_ref = if req
        .params
        .get("clear_assignee")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some(None)
    } else {
        optional_string_param(req, "assignee").map(|id| {
            Some(draft_core::task::AssigneeRef {
                kind: optional_string_param(req, "assignee_kind").unwrap_or_else(|| "actor".into()),
                id,
            })
        })
    };
    to_value(app.task_update(
        path,
        &string_param(req, "task")?,
        status,
        priority,
        due_at,
        assignee_ref,
    )?)
}

fn optional_string_param(req: &Request, key: &str) -> Option<String> {
    req.params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Missing means unchanged, JSON null means unset, and a string means set.
fn nullable_string_param(req: &Request, key: &str) -> DraftResult<Option<Option<String>>> {
    match req.params.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) => Ok(Some(Some(value.clone()))),
        Some(_) => Err(DraftError::invalid_config(format!(
            "'{key}' must be a string or null"
        ))),
    }
}

fn string_vec_param(req: &Request, key: &str) -> DraftResult<Vec<String>> {
    req.params
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::IpcError,
                format!("missing '{key}' parameter"),
            )
        })
}

fn string_vec_param_default(req: &Request, key: &str) -> Vec<String> {
    req.params
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn to_response<T: Serialize>(id: String, r: DraftResult<T>) -> Response {
    match r {
        Ok(v) => Response::ok(id, serde_json::to_value(v).unwrap_or(Value::Null)),
        Err(e) => Response::err(id, draft_err(&e)),
    }
}

/// A Console application-protocol failure.
///
/// Only one variant is Console-specific, and it exists so the gateway can tell
/// "your application session is gone" apart from every other failure **without
/// reading an error message**. Draft Core has no business knowing what a
/// Console session is, so the distinction lives here, at the service that owns
/// the protocol, and is mapped to a public code at the boundary.
#[derive(Debug)]
enum ConsoleError {
    /// The named application session is unknown or was replaced.
    ///
    /// Every site that raises this does so *before* consuming an invocation
    /// capability, opening an operation, or dispatching anything — so a caller
    /// receiving it knows, without guessing, that nothing executed. That proof
    /// is what makes automatic recovery safe.
    SessionUnknown(&'static str),
    Draft(DraftError),
}

impl From<DraftError> for ConsoleError {
    fn from(error: DraftError) -> Self {
        ConsoleError::Draft(error)
    }
}

impl ConsoleError {
    fn into_error_object(self) -> ErrorObject {
        match self {
            ConsoleError::SessionUnknown(message) => {
                ErrorObject::new(UNKNOWN_CONSOLE_SESSION, message)
            }
            ConsoleError::Draft(error) => draft_err(&error),
        }
    }
}

type ConsoleResult<T> = Result<T, ConsoleError>;

/// The public code a rejected Console application session reports.
///
/// Stable and machine-checkable: the gateway keys its recovery on this and
/// never on prose.
pub const UNKNOWN_CONSOLE_SESSION: &str = "UNKNOWN_CONSOLE_SESSION";

impl<T: Serialize> IntoResponse<T> for ConsoleResult<T> {
    fn into_response(self, id: String) -> Response {
        match self {
            Ok(value) => match serde_json::to_value(value) {
                Ok(value) => Response::ok(id, value),
                Err(error) => Response::err(id, draft_err(&DraftError::storage(error.to_string()))),
            },
            Err(error) => Response::err(id, error.into_error_object()),
        }
    }
}

fn draft_err(e: &DraftError) -> ErrorObject {
    let mut obj = ErrorObject::new(e.code(), e.message.clone());
    obj.details = json!({ "context": e.context, "suggestion": e.suggestion });
    obj
}

trait IntoResponse<T> {
    fn into_response(self, id: String) -> Response;
}

impl<T: Serialize> IntoResponse<T> for DraftResult<T> {
    fn into_response(self, id: String) -> Response {
        to_response(id, self)
    }
}
