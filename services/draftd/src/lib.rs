use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use draft_core::error::{DraftError, DraftErrorKind, DraftResult};
use draft_core::{App, DecisionKind};
use draft_ipc::{ErrorObject, Request, Response};
use draft_sessions::SessionManager;
use draft_store::{ServiceJobRecord, ServiceJobStatus, ServiceStore, WorkspaceRecord};

pub fn dispatch(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let app = App::new();
    let id = req.id.clone();
    match req.method.as_str() {
        "service.ping" => Response::ok(id, json!({ "pong": true })),
        "service.shutdown" => Response::ok(id, json!({ "stopping": true })),
        "service.status" => Response::ok(
            id,
            json!({
                "running": true,
                "version": draft_core::DRAFT_VERSION,
                "sessions": sessions.count(),
                "workspaces": store.list().len(),
            }),
        ),
        "workspace.init" => with_path(&req, |p| app.init(p)).into_response(id),
        "workspace.status" => with_path(&req, |p| app.status(p)).into_response(id),
        "workspace.register" => register_workspace(store, &app, &req).into_response(id),
        "events.list" => with_path(&req, |p| app.events(p)).into_response(id),
        "events.verify" => with_path(&req, |p| app.verify_events(p)).into_response(id),
        "events.replay" => with_path(&req, |p| app.replay_events(p)).into_response(id),
        "index.rebuild" => with_path(&req, |p| app.index_rebuild(p)).into_response(id),
        "job.submit" => submit_job(store, &app, &req).into_response(id),
        "job.list" => Ok(store.list_jobs()).into_response(id),
        "job.status" => job_status(store, &req).into_response(id),
        "job.cancel" => job_cancel(store, &req).into_response(id),
        "task.create" => with_path(&req, |p| {
            app.task_create(
                p,
                &string_param(&req, "title")?,
                optional_string_param(&req, "description"),
            )
        })
        .into_response(id),
        "task.list" => with_path(&req, |p| app.task_list(p)).into_response(id),
        "task.show" => {
            with_path(&req, |p| app.task_show(p, &string_param(&req, "task_id")?)).into_response(id)
        }
        "checkpoint.create" => {
            with_path(&req, |p| app.checkpoint(p, &string_param(&req, "message")?))
                .into_response(id)
        }
        "run.list" => with_path(&req, |p| app.runs(p)).into_response(id),
        "run.show" => {
            with_path(&req, |p| app.run_show(p, &string_param(&req, "run_id")?)).into_response(id)
        }
        "run.spawn" => with_path(&req, |p| {
            app.spawn_run(
                p,
                &string_param(&req, "task")?,
                &string_param(&req, "name")?,
                string_vec_param(&req, "command")?,
            )
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
        "pack.list" => with_path(&req, |p| app.pack_list(p)).into_response(id),
        "pack.show" => {
            with_path(&req, |p| app.pack_show(p, &string_param(&req, "pack")?)).into_response(id)
        }
        "verify.run" => {
            with_path(&req, |p| app.verify(p, &string_param(&req, "pack")?)).into_response(id)
        }
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
            app.decide(
                p,
                &string_param(&req, "pack")?,
                DecisionKind::Approve,
                optional_string_param(&req, "reason"),
            )
        })
        .into_response(id),
        "decision.reject" => with_path(&req, |p| {
            app.decide(
                p,
                &string_param(&req, "pack")?,
                DecisionKind::Reject,
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
        "save.run" => with_path(&req, |p| {
            app.save(p, &string_param(&req, "pack")?, BTreeMap::new())
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
        other => Response::err(
            id,
            ErrorObject::new("UNKNOWN_METHOD", format!("unknown method: {other}")),
        ),
    }
}

fn submit_job(store: &ServiceStore, app: &App, req: &Request) -> DraftResult<ServiceJobRecord> {
    let kind = string_param(req, "kind")?;
    let path = string_param(req, "path")?;
    let mut job = ServiceJobRecord {
        id: format!("job_{}", &uuid_like()[..12]),
        kind: kind.clone(),
        workspace_path: path.clone(),
        status: ServiceJobStatus::Queued,
        submitted_at: chrono::Utc::now(),
        started_at: None,
        ended_at: None,
        result: None,
        error: None,
    };
    store
        .save_job(&job)
        .map_err(|e| DraftError::storage(format!("failed to save queued job: {e}")))?;
    job.status = ServiceJobStatus::Running;
    job.started_at = Some(chrono::Utc::now());
    store
        .save_job(&job)
        .map_err(|e| DraftError::storage(format!("failed to save running job: {e}")))?;
    let result = run_job(app, Path::new(&path), req, &kind);
    job.ended_at = Some(chrono::Utc::now());
    match result {
        Ok(value) => {
            job.status = ServiceJobStatus::Completed;
            job.result = Some(value);
        }
        Err(e) => {
            job.status = ServiceJobStatus::Failed;
            job.error = Some(e.to_string());
        }
    }
    store
        .save_job(&job)
        .map_err(|e| DraftError::storage(format!("failed to save completed job: {e}")))?;
    Ok(job)
}

fn run_job(app: &App, path: &Path, req: &Request, kind: &str) -> DraftResult<Value> {
    match kind {
        "scan" => to_value(app.status(path)?),
        "verify" => to_value(app.verify(path, &string_param(req, "pack")?)?),
        "risk" => to_value(app.risk(path, &string_param(req, "pack")?)?),
        "compose" => to_value(app.compose(
            path,
            &string_param(req, "left")?,
            &string_param(req, "right")?,
            &string_param(req, "output")?,
        )?),
        "save" => to_value(app.save(path, &string_param(req, "pack")?, BTreeMap::new())?),
        "rollback" => to_value(app.rollback(path, &string_param(req, "target")?, true)?),
        "index-rebuild" => to_value(app.index_rebuild(path)?),
        other => Err(DraftError::new(
            DraftErrorKind::IpcError,
            format!("unknown job kind: {other}"),
        )),
    }
}

fn job_status(store: &ServiceStore, req: &Request) -> DraftResult<ServiceJobRecord> {
    let id = string_param(req, "job_id")?;
    store
        .load_job(&id)
        .ok_or_else(|| DraftError::not_found(format!("unknown job: {id}")))
}

fn job_cancel(store: &ServiceStore, req: &Request) -> DraftResult<ServiceJobRecord> {
    let mut job = job_status(store, req)?;
    if matches!(
        job.status,
        ServiceJobStatus::Queued | ServiceJobStatus::Running
    ) {
        job.status = ServiceJobStatus::Cancelled;
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

fn register_workspace(store: &ServiceStore, app: &App, req: &Request) -> DraftResult<Value> {
    let path = string_param(req, "path")?;
    let ws = app.open(Path::new(&path))?;
    store
        .register(WorkspaceRecord {
            id: ws.id.to_string(),
            path: ws.root.display().to_string(),
            workspace_kind: "draft-native".to_string(),
            registered_at: chrono::Utc::now(),
        })
        .map_err(|e| DraftError::storage(format!("failed to register workspace: {e}")))?;
    Ok(json!({ "registered": true, "workspace_id": ws.id.to_string() }))
}

fn with_path<T, F>(req: &Request, f: F) -> DraftResult<T>
where
    F: FnOnce(&Path) -> DraftResult<T>,
{
    let path = string_param(req, "path")?;
    if path.contains("..") {
        return Err(DraftError::new(
            DraftErrorKind::IpcError,
            "path traversal is not allowed",
        ));
    }
    f(Path::new(&path))
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

fn optional_string_param(req: &Request, key: &str) -> Option<String> {
    req.params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
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
