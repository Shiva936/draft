//! Draft Console AG-UI backend (PRD §9.19, TDD §39, §44).
//!
//! A local-only HTTP server (axum) that serves a dependency-free single-page app
//! and a small JSON API backed by real `draft-core` operations. Security posture
//! (NFRD §16.2): binds to loopback by default, enforces a request-size limit,
//! requires a per-session CSRF token on every mutating request, never exposes
//! private keys, and performs all mutations through the same core policy paths
//! as the CLI (which emit signed receipts).

use axum::{
    body::Bytes,
    extract::{Path as AxPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use draft_core::{home::GlobalHome, App};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dist");

struct AppState {
    root: PathBuf,
    csrf: String,
    bearer: String,
    allowed_origin: String,
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

/// Start the Console, blocking until Ctrl-C. `bind` is typically `127.0.0.1`.
pub fn serve(root: PathBuf, bind: &str, port: u16) -> Result<(), String> {
    let csrf = random_token();
    let bearer = load_or_create_service_token().map_err(|e| e.to_string())?;
    let allowed_origin = format!("http://{bind}:{port}");
    let state = Arc::new(AppState {
        root,
        csrf,
        bearer,
        allowed_origin,
    });
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let addr = format!("{bind}:{port}");
    rt.block_on(async move {
        let app = router(state);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("bind {addr}: {e}"))?;
        println!("Draft Console → http://{addr}  (Ctrl-C to stop)");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| format!("serve: {e}"))
    })
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/*path", get(asset))
        .route("/packs", get(list_packs))
        .route("/packs/:id", get(get_pack))
        .route("/packs/:id/diff", get(get_diff))
        .route("/packs/:id/risk", get(get_risk))
        .route("/packs/:id/readiness", get(get_readiness))
        .route("/packs/:id/receipts", get(get_receipts))
        .route("/tasks", get(get_tasks))
        .route("/inbox", get(get_inbox))
        .route("/doctor", get(get_doctor))
        .route("/editor/tree", get(editor_tree))
        .route("/editor/workspace", get(editor_workspace))
        .route("/editor/file", get(editor_file))
        .route("/editor/search", get(editor_search))
        .route("/editor/diff", get(editor_diff))
        .route("/editor/create", post(editor_create))
        .route("/editor/rename", post(editor_rename))
        .route("/editor/delete", post(editor_delete))
        .route("/editor/restore", post(editor_restore))
        .route("/editor/save", post(editor_save))
        .route(
            "/editor/task-from-selection",
            post(editor_task_from_selection),
        )
        .route("/events", get(get_events))
        .route("/packs/:id/approve", post(approve))
        .route("/packs/:id/reject", post(reject))
        .route("/packs/export", post(export))
        .route("/packs/import", post(import))
        // Cap request bodies (imports are bounded further by the import parser).
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            128 * 1024 * 1024,
        ))
        .with_state(state)
}

// ---- Handlers ------------------------------------------------------------

async fn index(State(st): State<Arc<AppState>>) -> Html<String> {
    let index = DIST
        .get_file("index.html")
        .and_then(|file| file.contents_utf8())
        .unwrap_or("<!doctype html><title>Draft Console</title><div id=\"app\"></div>");
    Html(render_index_html(index, &st))
}

fn render_index_html(index: &str, st: &AppState) -> String {
    let with_csrf = index.replace("__CSRF__", &st.csrf);
    if with_csrf.contains("__BEARER__") {
        with_csrf.replace("__BEARER__", &st.bearer)
    } else {
        with_csrf.replace(
            "<meta name=\"draft-csrf\"",
            &format!(
                "<meta name=\"draft-bearer\" content=\"{}\" />\n    <meta name=\"draft-csrf\"",
                st.bearer
            ),
        )
    }
}

async fn asset(AxPath(path): AxPath<String>) -> Response {
    let Some(file) = DIST.get_file(format!("assets/{path}")) else {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    };
    let content_type = if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    };
    (
        [(header::CONTENT_TYPE, content_type)],
        file.contents().to_vec(),
    )
        .into_response()
}

async fn list_packs(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().list_canonical_packs(&st.root))
}

async fn get_pack(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().pack_inspect(&st.root, &id))
}

async fn get_diff(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    App::new()
        .pack_diff_text(&st.root, &id)
        .map(|s| Json(Value::String(s)))
        .map_err(to_http)
}

async fn get_risk(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    App::new()
        .pack_risk_json(&st.root, &id)
        .map(Json)
        .map_err(to_http)
}

async fn get_readiness(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().submit_readiness_selected(&st.root, Some(&id)))
}

async fn get_receipts(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().pack_receipts_v2(&st.root, &id))
}

async fn get_tasks(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    let app = App::new();
    json(app.task_definitions(&st.root).and_then(|tasks| {
        tasks
            .into_iter()
            .map(|task| app.task_view(&st.root, task.id.as_str()))
            .collect::<Result<Vec<_>, _>>()
    }))
}

async fn get_inbox(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().inbox(&st.root))
}

async fn get_doctor(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().doctor(&st.root))
}

async fn editor_tree(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().editor_tree(&st.root))
}

async fn editor_workspace(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().editor_workspace(&st.root))
}

#[derive(Deserialize)]
struct EditorFileQuery {
    path: String,
}

async fn editor_file(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<EditorFileQuery>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().editor_read(&st.root, &q.path))
}

#[derive(Deserialize)]
struct EditorSearchQuery {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

async fn editor_search(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<EditorSearchQuery>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().editor_search(&st.root, &q.q, q.limit.unwrap_or(100)))
}

#[derive(Deserialize)]
struct EditorDiffQuery {
    path: String,
    #[serde(default)]
    pack: Option<String>,
}

async fn editor_diff(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<EditorDiffQuery>,
) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().editor_diff(&st.root, &q.path, q.pack.as_deref()))
}

#[derive(Deserialize)]
struct EditorCreateBody {
    path: String,
    #[serde(default)]
    content: String,
}

async fn editor_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorCreateBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().editor_create_file(&st.root, &body.path, &body.content))
}

#[derive(Deserialize)]
struct EditorRenameBody {
    from: String,
    to: String,
}

async fn editor_rename(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorRenameBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().editor_rename_file(&st.root, &body.from, &body.to))
}

#[derive(Deserialize)]
struct EditorDeleteBody {
    path: String,
}

async fn editor_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorDeleteBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().editor_delete_file(&st.root, &body.path))
}

#[derive(Deserialize)]
struct EditorRestoreBody {
    path: String,
    pack: String,
}

async fn editor_restore(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorRestoreBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().editor_restore_from_pack_base(&st.root, &body.path, &body.pack))
}

#[derive(Deserialize)]
struct EditorSaveBody {
    path: String,
    content: String,
    #[serde(default)]
    pack_name: Option<String>,
}

async fn editor_save(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorSaveBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().editor_save_to_pack(&st.root, &body.path, &body.content, body.pack_name))
}

#[derive(Deserialize)]
struct EditorTaskBody {
    path: String,
    start_line: u32,
    end_line: u32,
    selected_text: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    workspace_hash: Option<String>,
}

async fn editor_task_from_selection(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EditorTaskBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().task_create_from_selection(
        &st.root,
        &body.path,
        body.start_line,
        body.end_line,
        &body.selected_text,
        body.reason,
        body.workspace_hash,
    ))
}

async fn get_events(State(st): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    check_bearer(&headers, &st)?;
    json(App::new().canonical_events(&st.root))
}

#[derive(Deserialize, Default)]
struct DecideBody {
    #[serde(default)]
    reason: Option<String>,
}

async fn approve(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
    body: Option<Json<DecideBody>>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    let reason = body.and_then(|b| b.0.reason);
    App::new()
        .decide_pack(&st.root, &id, true, reason)
        .map(|pid| Json(serde_json::json!({ "approved": pid })))
        .map_err(to_http)
}

async fn reject(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
    body: Option<Json<DecideBody>>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    let reason = body.and_then(|b| b.0.reason);
    App::new()
        .decide_pack(&st.root, &id, false, reason)
        .map(|pid| Json(serde_json::json!({ "rejected": pid })))
        .map_err(to_http)
}

#[derive(Deserialize)]
struct ExportBody {
    pack_id: String,
}

async fn export(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ExportBody>,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().pack_export(&st.root, &body.pack_id, None))
}

#[derive(Deserialize)]
struct ImportQuery {
    name: Option<String>,
}

async fn import(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> ApiResult {
    check_write_auth(&headers, &st)?;
    json(App::new().pack_import_bytes(&st.root, &body, q.name.as_deref()))
}

// ---- Helpers -------------------------------------------------------------

fn json<T: serde::Serialize>(r: draft_core::error::DraftResult<T>) -> ApiResult {
    r.and_then(|v| {
        serde_json::to_value(&v)
            .map_err(|e| draft_core::error::DraftError::storage(format!("serialize: {e}")))
    })
    .map(Json)
    .map_err(to_http)
}

fn to_http(e: draft_core::error::DraftError) -> (StatusCode, String) {
    use draft_core::error::DraftErrorKind::*;
    let code = match e.kind {
        WorkspaceNotFound | NotFound => StatusCode::NOT_FOUND,
        InvalidConfig | ConflictDetected => StatusCode::BAD_REQUEST,
        RiskPolicyBlocked | ReviewRequired => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.message)
}

fn load_or_create_service_token() -> draft_core::error::DraftResult<String> {
    let home = GlobalHome::locate()?;
    home.create_all()?;
    let path = home.services_dir().join("tokens").join("agui.token");
    if path.exists() {
        return fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .map_err(draft_core::error::DraftError::from);
    }
    let token = random_token();
    fs::write(&path, format!("{token}\n")).map_err(draft_core::error::DraftError::from)?;
    Ok(token)
}

fn check_write_auth(headers: &HeaderMap, st: &AppState) -> Result<(), (StatusCode, String)> {
    check_bearer(headers, st)?;
    check_origin(headers, st)?;
    check_csrf(headers, st)
}

fn check_bearer(headers: &HeaderMap, st: &AppState) -> Result<(), (StatusCode, String)> {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Err((StatusCode::UNAUTHORIZED, "missing bearer token".into()));
    };
    let token = value.strip_prefix("Bearer ").unwrap_or_default();
    if token == st.bearer {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "invalid bearer token".into()))
    }
}

fn check_origin(headers: &HeaderMap, st: &AppState) -> Result<(), (StatusCode, String)> {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    if origin == st.allowed_origin {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "invalid request origin".into()))
    }
}

fn check_csrf(headers: &HeaderMap, st: &AppState) -> Result<(), (StatusCode, String)> {
    let token = headers.get("x-draft-csrf").and_then(|v| v.to_str().ok());
    if token == Some(st.csrf.as_str()) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "invalid or missing CSRF token".into(),
        ))
    }
}

fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        AppState {
            root: PathBuf::from("."),
            csrf: "secret".into(),
            bearer: "bearer-secret".into(),
            allowed_origin: "http://127.0.0.1:4317".into(),
        }
    }

    #[test]
    fn csrf_enforced() {
        let st = test_state();
        let mut ok = HeaderMap::new();
        ok.insert("x-draft-csrf", "secret".parse().unwrap());
        assert!(check_csrf(&ok, &st).is_ok());
        let bad = HeaderMap::new();
        assert!(check_csrf(&bad, &st).is_err());
    }

    #[test]
    fn index_injects_csrf_and_hides_no_keys() {
        let st = AppState {
            csrf: "tok123".into(),
            bearer: "bearer456".into(),
            ..test_state()
        };
        let html = DIST
            .get_file("index.html")
            .and_then(|file| file.contents_utf8())
            .unwrap_or("")
            .to_string();
        let html = render_index_html(&html, &st);
        assert!(html.contains("tok123"));
        assert!(html.contains("bearer456"));
        assert!(html.contains("<title>Draft Console</title>"));
        assert!(!html.contains("signing.key"));
    }

    #[test]
    fn bearer_and_origin_are_enforced() {
        let st = test_state();
        let mut ok = HeaderMap::new();
        ok.insert(
            header::AUTHORIZATION,
            "Bearer bearer-secret".parse().unwrap(),
        );
        ok.insert(header::ORIGIN, "http://127.0.0.1:4317".parse().unwrap());
        ok.insert("x-draft-csrf", "secret".parse().unwrap());
        assert!(check_write_auth(&ok, &st).is_ok());

        let missing = HeaderMap::new();
        assert_eq!(
            check_bearer(&missing, &st).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );

        let mut wrong_origin = ok.clone();
        wrong_origin.insert(header::ORIGIN, "http://evil.invalid".parse().unwrap());
        assert_eq!(
            check_write_auth(&wrong_origin, &st).unwrap_err().1,
            "invalid request origin"
        );
    }

    #[test]
    fn random_tokens_differ() {
        assert_ne!(random_token(), random_token());
    }

    #[tokio::test]
    async fn tasks_route_uses_shared_task_view_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::env::set_var("DRAFT_GLOBAL_HOME", root.join(".draft").join("_global"));
        let app = App::new();
        app.init(root).unwrap();
        app.task_define(
            root,
            "agui-task",
            "Review the local AG-UI task list",
            Some("docs_update".to_string()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap();

        let st = Arc::new(AppState {
            root: root.to_path_buf(),
            ..test_state()
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer bearer-secret".parse().unwrap(),
        );
        let Json(value) = get_tasks(State(st), headers).await.unwrap();
        let tasks = value.as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task"]["name"], "agui-task");
        assert!(tasks[0]["health_status"]["text"].is_string());
        assert!(tasks[0]["recommended_action"].is_string());
    }
}
