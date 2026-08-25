use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use draft_core::app::App;
use draft_core::operation::{BeginOperation, OperationStatus, OperationStore};
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_ipc::{
    ErrorObject, HandshakeRequest, HandshakeResponse, Request, Response, IPC_CAPABILITIES,
    IPC_PROTOCOL,
};
use draft_sessions::SessionManager;
use draft_store::{ServiceJobRecord, ServiceJobStatus, ServiceStore};

pub fn dispatch(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let id = req.id.clone();
    if let Err(error) = req.validate() {
        return Response::err(id, error);
    }
    if req.method == "service.handshake" {
        return handshake(req);
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
            | "pack.list"
            | "pack.show"
            | "risk.assess"
            | "receipt.list"
            | "receipt.show"
            | "console.overview"
            | "console.project"
            | "console.project.settings"
            | "console.inbox"
            | "console.doctor"
            | "console.search"
            | "console.settings"
            | "extension.list"
            | "extension.show"
            | "extension.source.list"
            | "extension.search"
            | "notification.list"
            | "config.list"
            | "hook.list"
            | "ignore.list"
            | "candidate.list"
            | "editor.tree"
            | "editor.workspace"
            | "editor.file"
            | "editor.search"
            | "editor.diff"
            | "editor.session.show"
    )
}

fn dispatch_inner(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let app = App::new();
    let id = req.id.clone();
    match req.method.as_str() {
        "service.ping" => Response::ok(id, json!({ "pong": true })),
        "service.shutdown" => Response::ok(id, json!({ "stopping": true })),
        "service.status" => draft_core::workspace::registry::ProjectRegistry::global()
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
        "workspace.list" => match draft_core::workspace::registry::ProjectRegistry::global()
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
        "extension.list" => draft_adapters::extension::list().into_response(id),
        "extension.show" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_adapters::extension::show(&extension_id))
            .into_response(id),
        "extension.source.list" => draft_adapters::catalog::source_list().into_response(id),
        "extension.source.add" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "location")
                    .and_then(|location| draft_adapters::catalog::source_add(&source_id, &location))
            })
            .into_response(id),
        "extension.source.remove" => string_param(&req, "source_id")
            .and_then(|source_id| draft_adapters::catalog::source_remove(&source_id))
            .into_response(id),
        "extension.source.trust" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "root_json").and_then(|root_json| {
                    string_param(&req, "fingerprint").and_then(|fingerprint| {
                        draft_adapters::catalog::trust_source_bytes(
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
        "extension.source.refresh" => string_param(&req, "source_id")
            .and_then(|source_id| draft_adapters::catalog::source_refresh(&source_id))
            .into_response(id),
        "extension.search" => {
            draft_adapters::catalog::discover(req.params.get("query").and_then(Value::as_str))
                .into_response(id)
        }
        "extension.install" => string_param(&req, "source_id")
            .and_then(|source_id| {
                string_param(&req, "extension_id").and_then(|extension_id| {
                    draft_adapters::catalog::install_from_source(
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
                    draft_adapters::catalog::update_from_source(
                        &source_id,
                        &extension_id,
                        req.params.get("version").and_then(Value::as_str),
                    )
                })
            })
            .into_response(id),
        "extension.update_all" => draft_adapters::catalog::update_all().into_response(id),
        "extension.uninstall" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_adapters::extension::uninstall(&extension_id))
            .into_response(id),
        "extension.enable" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_adapters::extension::set_enabled(&extension_id, true))
            .into_response(id),
        "extension.disable" => string_param(&req, "extension_id")
            .and_then(|extension_id| draft_adapters::extension::set_enabled(&extension_id, false))
            .into_response(id),
        "notification.list" => draft_core::operation::notification::NotificationStore::global()
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
                draft_core::operation::notification::NotificationStore::global()?.mark_read(
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
                draft_core::operation::notification::NotificationStore::global()?
                    .dismiss(&notification_id)
            })
            .into_response(id),
        "notification.resolve" => string_param(&req, "notification_id")
            .and_then(|notification_id| {
                draft_core::operation::notification::NotificationStore::global()?
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
        "pack.create" => with_path(&req, |p| {
            app.pack_create(
                p,
                optional_string_param(&req, "name"),
                optional_string_param(&req, "task"),
                req.params
                    .get("from_working_tree")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            )
        })
        .into_response(id),
        "pack.create_from_base" => with_path(&req, |p| {
            app.pack_create_from_base(
                p,
                string_param(&req, "name")?,
                optional_string_param(&req, "base_pack"),
            )
        })
        .into_response(id),
        "pack.select" => with_path(&req, |p| {
            app.pack_select_ref(p, &string_param(&req, "pack")?)
        })
        .into_response(id),
        "pack.delete" => with_path(&req, |p| {
            app.pack_delete_ref(p, &string_param(&req, "pack")?)
        })
        .into_response(id),
        "pack.reopen" => with_path(&req, |p| {
            app.pack_reopen(
                p,
                &string_param(&req, "pack")?,
                req.operation_id.as_deref().unwrap_or(req.id.as_str()),
            )
        })
        .into_response(id),
        "pack.list" => with_path(&req, |p| app.pack_list(p)).into_response(id),
        "pack.show" => {
            with_path(&req, |p| app.pack_show(p, &string_param(&req, "pack")?)).into_response(id)
        }
        "pack.canonical.list" => {
            with_path(&req, |p| console_pack_summaries(&app, p)).into_response(id)
        }
        "pack.inspect" => {
            with_path(&req, |p| app.pack_inspect(p, &string_param(&req, "pack")?)).into_response(id)
        }
        "pack.diff" => with_path(&req, |p| {
            app.pack_diff_text(p, &string_param(&req, "pack")?)
        })
        .into_response(id),
        "pack.readiness" => with_path(&req, |p| {
            app.submit_readiness_selected(p, Some(&string_param(&req, "pack")?))
        })
        .into_response(id),
        "pack.receipts" => with_path(&req, |p| app.pack_receipts(p, &string_param(&req, "pack")?))
            .into_response(id),
        "verify.run" => with_path(&req, |p| {
            let pack = string_param(&req, "pack")?;
            app.verify_pack(p, &pack, false, false)
        })
        .into_response(id),
        "risk.assess" => {
            with_path(&req, |p| app.risk(p, &string_param(&req, "pack")?)).into_response(id)
        }
        "review.start" | "review.comment" => with_path(&req, |p| {
            app.review(
                p,
                &string_param(&req, "pack")?,
                optional_string_param(&req, "comment"),
            )
        })
        .into_response(id),
        "decision.approve" => with_path(&req, |p| {
            app.decide_pack(
                p,
                &string_param(&req, "pack")?,
                true,
                optional_string_param(&req, "reason"),
            )
        })
        .into_response(id),
        "decision.reject" => with_path(&req, |p| {
            app.decide_pack(
                p,
                &string_param(&req, "pack")?,
                false,
                optional_string_param(&req, "reason"),
            )
        })
        .into_response(id),
        "compare.run" => with_path(&req, |p| {
            app.compare(
                p,
                &string_param(&req, "left")?,
                &string_param(&req, "right")?,
            )
        })
        .into_response(id),
        "compose.run" => with_path(&req, |p| {
            app.compose(
                p,
                &string_param(&req, "left")?,
                &string_param(&req, "right")?,
                &string_param(&req, "output")?,
            )
        })
        .into_response(id),
        "submit.run" => with_path(&req, |p| {
            app.submit(p, &string_param(&req, "pack")?, BTreeMap::new())
        })
        .into_response(id),
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
        "waiver.create" => with_path(&req, |p| {
            app.waive(
                p,
                &string_param(&req, "pack")?,
                &string_param(&req, "finding_id")?,
                &string_param(&req, "reason")?,
                &string_param(&req, "expires")?,
            )
        })
        .into_response(id),
        "doctor.project" => with_path(&req, |p| app.doctor(p)).into_response(id),
        "events.canonical" => with_path(&req, |p| app.canonical_events(p)).into_response(id),
        "editor.tree" => with_path(&req, |p| app.editor_tree(p)).into_response(id),
        "editor.workspace" => with_path(&req, |p| app.editor_workspace(p)).into_response(id),
        "editor.file" => with_path(&req, |p| {
            app.editor_read(p, &string_param(&req, "file_path")?)
        })
        .into_response(id),
        "editor.search" => with_path(&req, |p| {
            app.editor_search(
                p,
                &string_param(&req, "query")?,
                req.params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100) as usize,
            )
        })
        .into_response(id),
        "editor.diff" => with_path(&req, |p| {
            app.editor_diff(
                p,
                &string_param(&req, "file_path")?,
                optional_string_param(&req, "pack").as_deref(),
            )
        })
        .into_response(id),
        "editor.session.show" => with_path(&req, |p| {
            draft_core::operation::editor::EditSessionStore::for_workspace(p)
                .load(&string_param(&req, "session_id")?)
        })
        .into_response(id),
        "editor.session.save" => {
            with_path(&req, |p| save_editor_session(&app, p, &req)).into_response(id)
        }
        "editor.session.stage" => {
            with_path(&req, |p| save_editor_session(&app, p, &req)).into_response(id)
        }
        "editor.session.commit" => {
            with_path(&req, |p| commit_editor_session(&app, p, &req)).into_response(id)
        }
        "editor.create" => with_path(&req, |p| {
            app.editor_create_file(
                p,
                &string_param(&req, "file_path")?,
                optional_string_param(&req, "content")
                    .as_deref()
                    .unwrap_or_default(),
            )
        })
        .into_response(id),
        "editor.rename" => with_path(&req, |p| {
            app.editor_rename_file(p, &string_param(&req, "from")?, &string_param(&req, "to")?)
        })
        .into_response(id),
        "editor.delete" => with_path(&req, |p| {
            app.editor_delete_file(p, &string_param(&req, "file_path")?)
        })
        .into_response(id),
        "editor.restore" => with_path(&req, |p| {
            app.editor_restore_from_pack_base(
                p,
                &string_param(&req, "file_path")?,
                &string_param(&req, "pack")?,
            )
        })
        .into_response(id),
        "editor.save" => with_path(&req, |p| {
            app.editor_save_to_pack(
                p,
                &string_param(&req, "file_path")?,
                &string_param(&req, "content")?,
                optional_string_param(&req, "pack_name"),
            )
        })
        .into_response(id),
        other => Response::err(
            id,
            ErrorObject::new("UNKNOWN_METHOD", format!("unknown method: {other}")),
        ),
    }
}

fn save_editor_session(app: &App, root: &Path, req: &Request) -> DraftResult<Value> {
    let attribution: draft_core::operation::editor::EditAttribution = serde_json::from_value(
        req.params
            .get("attribution")
            .cloned()
            .ok_or_else(|| DraftError::invalid_config("editor attribution is required"))?,
    )
    .map_err(|error| DraftError::invalid_config(format!("invalid editor attribution: {error}")))?;
    validate_editor_attribution(app, root, &attribution)?;
    let operation_id = draft_core::support::common::OperationId::new(
        req.operation_id
            .clone()
            .unwrap_or_else(|| format!("op_{}", req.id)),
    );
    let store = draft_core::operation::editor::EditSessionStore::for_workspace(root);
    let session = if let Some(session_id) = optional_string_param(req, "session_id") {
        let existing = store.load(&session_id)?;
        if existing.attribution != attribution {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "editor attribution cannot change within an open session",
            ));
        }
        existing
    } else {
        store.open(attribution, operation_id.clone())?
    };
    let edit_kind = optional_string_param(req, "edit_kind").unwrap_or_else(|| "write".into());
    match edit_kind.as_str() {
        "write" | "create_file" => to_value(store.stage_write(
            &session.session_id,
            &string_param(req, "file_path")?,
            optional_string_param(req, "content").unwrap_or_default(),
            operation_id,
        )?),
        "create_directory" => to_value(store.stage_create_directory(
            &session.session_id,
            &string_param(req, "file_path")?,
            operation_id,
        )?),
        "rename" | "move" => to_value(store.stage_rename(
            &session.session_id,
            &string_param(req, "from")?,
            &string_param(req, "to")?,
            operation_id,
        )?),
        "delete" => to_value(
            store.stage_delete(
                &session.session_id,
                &string_param(req, "file_path")?,
                req.params
                    .get("recursive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                operation_id,
            )?,
        ),
        _ => Err(DraftError::invalid_config(
            "unknown staged editor operation",
        )),
    }
}

fn commit_editor_session(app: &App, root: &Path, req: &Request) -> DraftResult<Value> {
    let session_id = string_param(req, "session_id")?;
    let session =
        draft_core::operation::editor::EditSessionStore::for_workspace(root).load(&session_id)?;
    validate_editor_attribution(app, root, &session.attribution)?;
    let operation_id = draft_core::support::common::OperationId::new(
        req.operation_id
            .clone()
            .unwrap_or_else(|| format!("op_{}", req.id)),
    );
    to_value(app.editor_session_commit(root, &session_id, operation_id)?)
}

fn validate_editor_attribution(
    app: &App,
    root: &Path,
    attribution: &draft_core::operation::editor::EditAttribution,
) -> DraftResult<()> {
    use draft_core::operation::editor::EditAttribution;
    match attribution {
        EditAttribution::Task { id } => {
            app.task_show(root, id)?;
        }
        EditAttribution::Pack { id } | EditAttribution::Review { id } => {
            let report = app.pack_inspect(root, id)?;
            if matches!(
                report.lifecycle.as_str(),
                "submitted" | "import_submitted" | "rolled_back"
            ) {
                return Err(DraftError::invalid_config(
                    "immutable pack lifecycle cannot receive editor mutations",
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
        draft_core::workspace::registry::ProjectRegistry::global()?
            .resolve(workspace_id)?
            .repository_path
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
    let result = run_job(
        &App::new(),
        Path::new(&job.workspace_path),
        &request,
        &job.kind,
    );

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
        "verify" => {
            let pack = string_param(req, "pack")?;
            to_value(app.verify_pack(path, &pack, false, false)?)
        }
        "risk" => to_value(app.risk(path, &string_param(req, "pack")?)?),
        "compose" => to_value(app.compose(
            path,
            &string_param(req, "left")?,
            &string_param(req, "right")?,
            &string_param(req, "output")?,
        )?),
        "submit" => to_value(app.submit(path, &string_param(req, "pack")?, BTreeMap::new())?),
        "rollback" => to_value(app.rollback(path, &string_param(req, "target")?, true)?),
        "index-rebuild" => to_value(app.index_rebuild(path)?),
        "extension-install" => to_value(draft_adapters::catalog::install_from_source(
            &string_param(req, "source_id")?,
            &string_param(req, "extension_id")?,
            req.params.get("version").and_then(Value::as_str),
        )?),
        "extension-update" => to_value(draft_adapters::catalog::update_from_source(
            &string_param(req, "source_id")?,
            &string_param(req, "extension_id")?,
            req.params.get("version").and_then(Value::as_str),
        )?),
        "extension-update-all" => to_value(draft_adapters::catalog::update_all()?),
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
    draft_core::workspace::registry::ProjectRegistry::global()?.upsert(
        ws.workspace_id.as_str(),
        &ws.root,
        None,
    )?;
    Ok(json!({ "registered": true, "workspace_id": ws.workspace_id.to_string() }))
}

fn unregister_workspace(req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "workspace_id")?;
    let removed =
        draft_core::workspace::registry::ProjectRegistry::global()?.remove(&workspace_id)?;
    Ok(json!({ "workspace_id": workspace_id, "unregistered": removed }))
}

fn relocate_workspace(req: &Request) -> DraftResult<Value> {
    let workspace_id = string_param(req, "workspace_id")?;
    let destination = string_param(req, "destination")?;
    to_value(
        draft_core::workspace::registry::ProjectRegistry::global()?
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
    } else if let Some(workspace_id) = optional_string_param(req, "workspace_id") {
        let registry = draft_core::workspace::registry::ProjectRegistry::global()?;
        if is_mutation(&req.method) {
            let blocking = registry.inspect()?.into_iter().any(|issue| {
                issue.workspace_id == workspace_id
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
        registry.resolve(&workspace_id)?.repository_path
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

fn console_overview(store: &ServiceStore, app: &App) -> DraftResult<Value> {
    let registry = draft_core::workspace::registry::ProjectRegistry::global()?;
    let projects = registry.list()?;
    let issues = registry.inspect()?;
    let mut summaries = Vec::new();
    for project in projects.iter().take(100) {
        let path = Path::new(&project.repository_path);
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
    let entry =
        draft_core::workspace::registry::ProjectRegistry::global()?.resolve(&workspace_id)?;
    let root = Path::new(&entry.repository_path);
    let revision = draft_core::workspace::source_view::WorkspaceRevision::derive(root)?;
    let packs = console_pack_summaries(app, root)?;
    Ok(json!({
        "project": entry,
        "revision": revision,
        "status": app.status(root)?,
        "tasks": app.task_list(root)?,
        "packs": packs,
        "inbox": app.inbox(root)?,
    }))
}

fn console_pack_summaries(app: &App, root: &Path) -> DraftResult<Vec<Value>> {
    app.list_canonical_packs(root)?
        .into_iter()
        .map(|manifest| {
            let inspect = app.pack_inspect(root, &manifest.pack_id)?;
            Ok(json!({
                "pack_id": manifest.pack_id,
                "name": manifest.name,
                "intent": manifest.intent.as_str(),
                "submit_state": inspect.lifecycle.as_str(),
                "import_state": inspect.quarantine.map(|record| record.trust_evaluation),
                "revision": inspect.revision_id,
                "valid_actions": inspect.valid_actions,
            }))
        })
        .collect()
}

fn console_inbox(app: &App) -> DraftResult<Value> {
    let projects = draft_core::workspace::registry::ProjectRegistry::global()?.list()?;
    let mut results = Vec::new();
    for project in projects.into_iter().take(100) {
        let scanned_at = chrono::Utc::now();
        match app.inbox(Path::new(&project.repository_path)) {
            Ok(items) => results.push(json!({ "workspace_id": project.workspace_id, "freshness": "fresh", "scanned_at": scanned_at, "items": items })),
            Err(error) => results.push(json!({ "workspace_id": project.workspace_id, "freshness": "unavailable", "scanned_at": scanned_at, "items": [], "error": { "code": error.code(), "message": error.message } })),
        }
    }
    let notifications =
        draft_core::operation::notification::NotificationStore::global()?.list(false)?;
    Ok(
        json!({ "projects": results, "notifications": notifications, "generated_at": chrono::Utc::now() }),
    )
}

fn console_doctor(app: &App) -> DraftResult<Value> {
    let registry = draft_core::workspace::registry::ProjectRegistry::global()?;
    Ok(json!({
        "global": app.doctor_global()?,
        "registry_issues": registry.inspect()?,
        "projects": registry.list()?,
        "generated_at": chrono::Utc::now(),
    }))
}

fn console_settings(app: &App) -> DraftResult<Value> {
    let home = draft_core::workspace::home::DraftGlobalStore::locate()?;
    draft_core::trust::identity::reject_retired_profile_state(None)?;
    draft_core::trust::identity::global::reject_retired_actor_profile(&home)?;
    draft_core::workspace::config::reject_retired_profile_config(&home.config_toml())?;
    let resolver = draft_core::workspace::config::ConfigResolver::load(
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
    for project in draft_core::workspace::registry::ProjectRegistry::global()?
        .list()?
        .into_iter()
        .take(100)
    {
        let workspace_id = project.workspace_id.clone();
        let root = Path::new(&project.repository_path);
        if project.name.to_lowercase().contains(&query)
            || project.workspace_id.to_lowercase().contains(&query)
        {
            results.push(json!({ "kind": "project", "workspace_id": workspace_id, "title": project.name, "subtitle": project.repository_path }));
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
        for pack in app.list_canonical_packs(root)?.into_iter().take(100) {
            let inspect = app.pack_inspect(root, &pack.pack_id)?;
            let searchable =
                format!("{} {} {:?}", pack.pack_id, pack.name, pack.intent).to_lowercase();
            if searchable.contains(&query) {
                results.push(json!({ "kind": "pack", "workspace_id": workspace_id, "id": pack.pack_id, "title": pack.name, "subtitle": inspect.lifecycle }));
            }
            if results.len() < limit {
                for action in inspect.valid_actions {
                    if action.contains(&query) {
                        results.push(json!({ "kind": "action", "workspace_id": workspace_id, "id": action, "pack_id": pack.pack_id, "title": format!("{} {}", action, pack.name), "subtitle": "Currently valid pack action" }));
                    }
                }
            }
            if results.len() >= limit {
                break;
            }
        }
        if results.len() < limit {
            for hit in app.editor_search(root, &query, (limit - results.len()).min(20))? {
                results.push(json!({ "kind": "file", "workspace_id": workspace_id, "id": hit.path, "title": hit.path, "subtitle": format!("line {} · {}", hit.line, hit.preview) }));
            }
        }
        if results.len() < limit {
            for event in app.canonical_events(root)?.into_iter().rev().take(50) {
                let searchable = format!(
                    "{} {}",
                    event.event_type,
                    event.subject_id.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                if searchable.contains(&query) {
                    results.push(json!({ "kind": "event", "workspace_id": workspace_id, "id": event.event_id, "title": event.event_type, "subtitle": event.subject_id }));
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
