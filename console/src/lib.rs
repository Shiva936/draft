//! Authenticated loopback gateway for Draft Console.
//!
//! The gateway owns browser transport security only. Every Draft read or
//! mutation is translated to `draft-ipc` and handled by `draftd`; no domain
//! transition, persistence rule, or valid-action decision is implemented here.

use axum::{
    body::Bytes,
    extract::{Extension, Path as AxPath, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    middleware::{self, Next},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use draft_ipc::{call, socket_path, Request as IpcRequest, IPC_PROTOCOL};
use futures_util::stream;
use include_dir::{include_dir, Dir};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dist");
static LOGO: &[u8] = include_bytes!("../../assets/logo/draft-console.png");
use draft_ipc::contract_versions::{
    CONSOLE_API_ENVELOPE as API_ENVELOPE_VERSION, CONSOLE_API_FAILURE as API_FAILURE_VERSION,
    CONSOLE_DAEMON_EVENT as DAEMON_EVENT_VERSION, CONSOLE_JOBS_EVENT as JOBS_EVENT_VERSION,
    CONSOLE_MUTATION_REQUEST as MUTATION_VERSION, CONSOLE_SESSION as SESSION_VERSION,
};
const SESSION_COOKIE: &str = "draft_console_session";
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const BOOTSTRAP_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ConsoleLaunchOptions {
    pub bind: String,
    pub port: u16,
    pub preselected_workspace_id: Option<String>,
    pub open_browser: bool,
}

impl Default for ConsoleLaunchOptions {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".into(),
            port: 4317,
            preselected_workspace_id: None,
            open_browser: true,
        }
    }
}

#[derive(Debug)]
struct BootstrapSecret {
    value: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct BrowserSession {
    csrf: String,
    expires_at: Instant,
}

struct AppState {
    authority: String,
    origin: String,
    bootstrap: Mutex<Option<BootstrapSecret>>,
    sessions: Mutex<HashMap<String, BrowserSession>>,
    ipc_path: PathBuf,
    preselected_workspace_id: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    details: Value,
}

impl ApiError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "schema_version": API_FAILURE_VERSION,
                "error": { "code": self.code, "message": self.message, "details": self.details }
            })),
        )
            .into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

/// Serve Console with a workspace preselected from canonical metadata.
pub fn serve(root: PathBuf, bind: &str, port: u16) -> Result<(), String> {
    let preselected_workspace_id = std::fs::read(root.join(".draft/workspace.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("id")
                .or_else(|| value.get("workspace_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
    serve_console(ConsoleLaunchOptions {
        bind: bind.into(),
        port,
        preselected_workspace_id,
        open_browser: false,
    })
}

pub fn serve_console(options: ConsoleLaunchOptions) -> Result<(), String> {
    if options.bind != "127.0.0.1" && options.bind != "::1" {
        return Err("Console gateway may bind only to 127.0.0.1 or ::1".into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime: {error}"))?;
    runtime.block_on(async move {
        let requested = format!("{}:{}", options.bind, options.port);
        let listener = tokio::net::TcpListener::bind(&requested)
            .await
            .map_err(|error| format!("bind {requested}: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("local address: {error}"))?;
        let (authority, origin) = origin_for(address);
        let secret = random_token(32);
        let state = Arc::new(AppState {
            authority,
            origin: origin.clone(),
            bootstrap: Mutex::new(Some(BootstrapSecret {
                value: secret.clone(),
                expires_at: Instant::now() + BOOTSTRAP_TTL,
            })),
            sessions: Mutex::new(HashMap::new()),
            ipc_path: socket_path(),
            preselected_workspace_id: options.preselected_workspace_id,
        });
        let url = format!("{origin}/#bootstrap={secret}");
        println!("Draft Console → {origin}  (Ctrl-C to stop)");
        if !options.open_browser || !open_browser(&url) {
            println!("Open this URL in your browser: {url}");
        }
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|error| format!("serve: {error}"))
    })
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/draft-console.png", get(logo))
        .route("/assets/*path", get(asset))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/session", get(session))
        .route("/api/v1/events", get(sse))
        .route("/api/v1/jobs/:job_id", get(job_status))
        .route("/api/v1/jobs/:job_id/cancel", post(job_cancel))
        .route("/api/v1/system/overview", get(system_overview))
        .route("/api/v1/projects", get(projects))
        .route(
            "/api/v1/project-actions/:action",
            post(system_project_action),
        )
        .route("/api/v1/inbox", get(inbox))
        .route(
            "/api/v1/inbox/:notification_id/:action",
            post(notification_action),
        )
        .route("/api/v1/doctor", get(doctor))
        .route("/api/v1/settings", get(settings))
        .route("/api/v1/settings/user", post(user_update))
        .route("/api/v1/extensions", get(extensions))
        .route("/api/v1/extensions/sources", get(extension_sources))
        .route("/api/v1/extensions/discover", get(extension_discover))
        .route(
            "/api/v1/extensions/sources/:source_id/:action",
            post(extension_source_action),
        )
        .route(
            "/api/v1/extensions/actions/:action",
            post(extension_bulk_action),
        )
        .route(
            "/api/v1/extensions/:extension_id/:action",
            post(extension_action),
        )
        .route("/api/v1/search", get(search))
        .route("/api/v1/projects/:workspace_id", get(project))
        .route(
            "/api/v1/projects/:workspace_id/settings",
            get(project_settings),
        )
        .route("/api/v1/projects/:workspace_id/tasks", get(tasks))
        .route("/api/v1/projects/:workspace_id/events", get(events))
        .route("/api/v1/projects/:workspace_id/files", get(files))
        .route("/api/v1/projects/:workspace_id/file", get(file))
        .route("/api/v1/projects/:workspace_id/packs", get(packs))
        .route("/api/v1/projects/:workspace_id/packs/:pack_id", get(pack))
        .route(
            "/api/v1/projects/:workspace_id/packs/:pack_id/:view",
            get(pack_view),
        )
        .route(
            "/api/v1/projects/:workspace_id/actions/:action",
            post(project_action),
        )
        .route(
            "/api/v1/projects/:workspace_id/packs/:pack_id/actions/:action",
            post(pack_action),
        )
        .fallback(fallback)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            2 * 1024 * 1024,
        ))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_security,
        ))
        .with_state(state)
}

/// A per-response CSP nonce. The editor registers its stylesheet at runtime, so
/// the served document carries a nonce instead of relaxing `style-src`.
#[derive(Clone)]
struct CspNonce(String);

async fn request_security(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Err(error) = validate_security_headers(request.headers(), &state) {
        return error.into_response();
    }
    let nonce = random_token(16);
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let response_headers = response.headers_mut();
    let policy = format!(
        "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'nonce-{nonce}'; img-src 'self' data:; font-src 'self'; connect-src 'self'"
    );
    if let Ok(value) = HeaderValue::from_str(&policy) {
        response_headers.insert(header::CONTENT_SECURITY_POLICY, value);
    }
    response_headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn validate_security_headers(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    let host_valid = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == state.authority);
    if !host_valid {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "INVALID_HOST",
            "invalid Console host",
        ));
    }
    for forwarded in [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
        "via",
    ] {
        if headers.contains_key(forwarded) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "FORWARDED_REQUEST_REJECTED",
                "forwarded requests are not trusted by Console",
            ));
        }
    }
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site != "same-origin" && site != "none")
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "CROSS_SITE_REQUEST_REJECTED",
            "cross-site browser requests are not allowed",
        ));
    }
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        if origin != state.origin {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "INVALID_ORIGIN",
                "invalid Console request origin",
            ));
        }
    }
    Ok(())
}

const NONCE_PLACEHOLDER: &str = "__CSP_NONCE__";

async fn index(Extension(nonce): Extension<CspNonce>) -> Html<String> {
    let document = DIST
        .get_file("index.html")
        .and_then(|file| file.contents_utf8())
        .unwrap_or("<!doctype html><title>Draft Console</title><div id=\"root\"></div>");
    Html(document.replace(NONCE_PLACEHOLDER, &nonce.0))
}

async fn fallback(Extension(nonce): Extension<CspNonce>, uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    index(Extension(nonce)).await.into_response()
}

async fn logo() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], LOGO).into_response()
}

async fn asset(AxPath(path): AxPath<String>) -> Response {
    let Some(file) = DIST.get_file(format!("assets/{path}")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    };
    (
        [(header::CONTENT_TYPE, content_type)],
        file.contents().to_vec(),
    )
        .into_response()
}

async fn bootstrap(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_exact_origin(&headers, &state)?;
    let body = parse_body(&body)?;
    let secret = body.get("secret").and_then(Value::as_str).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "bootstrap request requires string field 'secret'",
        )
    })?;
    let mut bootstrap = state.bootstrap.lock().unwrap();
    let valid = bootstrap.as_ref().is_some_and(|candidate| {
        candidate.expires_at > Instant::now() && constant_time_eq(&candidate.value, secret)
    });
    if !valid {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "INVALID_BOOTSTRAP",
            "bootstrap secret is invalid, expired, or already consumed",
        ));
    }
    *bootstrap = None;
    drop(bootstrap);

    let session_id = random_token(32);
    let csrf = random_token(24);
    state.sessions.lock().unwrap().insert(
        session_id.clone(),
        BrowserSession {
            csrf: csrf.clone(),
            expires_at: Instant::now() + SESSION_TTL,
        },
    );
    let cookie = format!(
        "{SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    let mut response = Json(json!({
        "schema_version": API_ENVELOPE_VERSION,
        "data": {
            "schema_version": SESSION_VERSION,
            "authenticated": true,
            "csrf_token": csrf,
            "preselected_workspace_id": state.preselected_workspace_id,
        }
    }))
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "COOKIE_ERROR",
                "failed to issue session",
            )
        })?,
    );
    Ok(response)
}

async fn session(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let session = authenticate(&headers, &state)?;
    Ok(Json(json!({
        "schema_version": API_ENVELOPE_VERSION,
        "data": {
            "schema_version": SESSION_VERSION,
            "authenticated": true,
            "csrf_token": session.csrf,
            "preselected_workspace_id": state.preselected_workspace_id,
        }
    })))
}

async fn sse(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    authenticate(&headers, &state)?;
    let ipc_path = state.ipc_path.clone();
    let stream = stream::unfold(
        (ipc_path, true, false),
        |(ipc_path, first, emit_jobs)| async move {
            if emit_jobs {
                let jobs = match call(
                    &ipc_path,
                    &IpcRequest::new(request_id(), "job.list", json!({})),
                ) {
                    Ok(response) if response.ok => {
                        response.result.expect("validated IPC success response")
                    }
                    Ok(response) => json!({
                        "unavailable": true,
                        "error": response.error,
                    }),
                    Err(error) => json!({
                        "unavailable": true,
                        "error": { "code": "SERVICE_UNAVAILABLE", "message": error.to_string() },
                    }),
                };
                let event = SseEvent::default()
                    .event("jobs")
                    .json_data(json!({ "schema_version": JOBS_EVENT_VERSION, "jobs": jobs }))
                    .expect("Draft SSE payloads are representable as JSON");
                return Some((Ok::<_, Infallible>(event), (ipc_path, false, false)));
            }
            if !first {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            let connected = draft_ipc::is_running(&ipc_path);
            let event = SseEvent::default()
                .event("daemon")
                .retry(Duration::from_millis(1500))
                .json_data(json!({
                    "schema_version": DAEMON_EVENT_VERSION,
                    "connected": connected,
                    "protocol": IPC_PROTOCOL,
                }))
                .expect("Draft SSE payloads are representable as JSON");
            Some((Ok::<_, Infallible>(event), (ipc_path, false, true)))
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("keepalive"),
        )
        .into_response())
}

async fn job_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(job_id): AxPath<String>,
) -> ApiResult {
    authenticated_call(&state, &headers, "job.status", json!({ "job_id": job_id }))
}

async fn job_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(job_id): AxPath<String>,
    body: Bytes,
) -> ApiResult {
    parse_body(&body)?;
    authenticated_mutation_call(&state, &headers, "job.cancel", json!({ "job_id": job_id }))
}

async fn system_overview(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "console.overview", json!({}))
}

async fn projects(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "workspace.list", json!({}))
}

async fn inbox(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "console.inbox", json!({}))
}

async fn doctor(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "console.doctor", json!({}))
}

async fn settings(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "console.settings", json!({}))
}

async fn user_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    authenticated_mutation_call(&state, &headers, "config.global.update", parse_body(&body)?)
}

async fn extensions(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "extension.list", json!({}))
}

async fn extension_sources(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    authenticated_call(&state, &headers, "extension.source.list", json!({}))
}

#[derive(Deserialize)]
struct ExtensionSearchQuery {
    #[serde(default)]
    q: String,
}

async fn extension_discover(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ExtensionSearchQuery>,
) -> ApiResult {
    authenticated_call(
        &state,
        &headers,
        "extension.search",
        json!({ "query": query.q }),
    )
}

async fn extension_source_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((source_id, action)): AxPath<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "add" => "extension.source.add",
        "remove" => "extension.source.remove",
        "trust" => "extension.source.trust",
        "refresh" => "extension.source.refresh",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_EXTENSION_SOURCE_ACTION",
                "unknown extension source action",
            ))
        }
    };
    let mut params = parse_body(&body)?;
    if action == "add"
        && !params
            .get("location")
            .and_then(Value::as_str)
            .is_some_and(|location| location.starts_with("https://"))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PATH_NOT_ACCEPTED",
            "Console catalog sources must use HTTPS; local paths remain an explicit CLI workflow",
        ));
    }
    params["source_id"] = Value::String(source_id);
    authenticated_mutation_call(&state, &headers, method, params)
}

async fn extension_bulk_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(action): AxPath<String>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "update-all" => "job.submit",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_EXTENSION_ACTION",
                "unknown extension bulk action",
            ))
        }
    };
    let mut params = parse_body(&body)?;
    params["kind"] = Value::String("extension-update-all".into());
    authenticated_mutation_call(&state, &headers, method, params)
}

async fn extension_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((extension_id, action)): AxPath<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "install" | "update" => "job.submit",
        "uninstall" => "extension.uninstall",
        "enable" => "extension.enable",
        "disable" => "extension.disable",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_EXTENSION_ACTION",
                "unknown extension action",
            ))
        }
    };
    let mut params = parse_body(&body)?;
    params["extension_id"] = Value::String(extension_id);
    if matches!(action.as_str(), "install" | "update") {
        params["kind"] = Value::String(format!("extension-{action}"));
    }
    authenticated_mutation_call(&state, &headers, method, params)
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> ApiResult {
    authenticated_call(
        &state,
        &headers,
        "console.search",
        json!({ "query": query.q, "limit": query.limit.unwrap_or(50) }),
    )
}

async fn project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "console.project",
        json!({}),
    )
}

async fn project_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "console.project.settings",
        json!({}),
    )
}

async fn tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "task.views", json!({}))
}

async fn events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "events.canonical",
        json!({}),
    )
}

async fn files(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "editor.tree", json!({}))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
    Query(query): Query<FileQuery>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "editor.file",
        json!({ "file_path": query.path }),
    )
}

async fn packs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "pack.canonical.list",
        json!({}),
    )
}

async fn pack(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, pack_id)): AxPath<(String, String)>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "pack.inspect",
        json!({ "pack": pack_id }),
    )
}

async fn pack_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, pack_id, view)): AxPath<(String, String, String)>,
) -> ApiResult {
    let method = match view.as_str() {
        "diff" => "pack.diff",
        "risk" => "risk.assess",
        "verify" | "readiness" | "review" | "approvals" | "submit" => "pack.readiness",
        "receipts" | "rollback" => "pack.receipts",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_PACK_VIEW",
                "unknown pack view",
            ))
        }
    };
    project_call(
        &state,
        &headers,
        &workspace_id,
        method,
        json!({ "pack": pack_id }),
    )
}

async fn project_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, action)): AxPath<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "editor-create" | "editor-rename" | "editor-delete" => "editor.session.stage",
        "editor-restore" => "editor.restore",
        "editor-save" => "editor.session.save",
        "editor-commit" => "editor.session.commit",
        "task-create" => "task.create",
        "task-update" => "task.update",
        "task-next-action-add" => "task.next_action.add",
        "task-next-action-set" => "task.next_action.set",
        "task-drop" => "task.drop",
        "pack-create" => "pack.create_from_base",
        "pack-select" => "pack.select",
        "pack-delete" => "pack.delete",
        "config-set" => "config.set",
        "config-unset" => "config.unset",
        "hook-set" => "hook.set",
        "hook-unset" => "hook.unset",
        "hook-run" => "hook.run",
        "ignore-add" => "ignore.add",
        "ignore-remove" => "ignore.remove",
        "candidate-add" => "candidate.add",
        "candidate-update" => "candidate.update",
        "candidate-remove" => "candidate.remove",
        "waive" => "waiver.create",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_ACTION",
                "unknown project action",
            ))
        }
    };
    mutation_call(&state, &headers, &workspace_id, method, parse_body(&body)?)
}

async fn system_project_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(action): AxPath<String>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "init" => "workspace.init",
        "register" => "workspace.register",
        "relocate" => "workspace.relocate",
        "unregister" => "workspace.unregister",
        "adopt-copy" => "workspace.adopt_copy",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_PROJECT_ACTION",
                "unknown project action",
            ))
        }
    };
    authenticated_mutation_call(&state, &headers, method, parse_body(&body)?)
}

async fn notification_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((notification_id, action)): AxPath<(String, String)>,
    body: Bytes,
) -> ApiResult {
    parse_body(&body)?;
    let method = match action.as_str() {
        "read" => "notification.read",
        "dismiss" => "notification.dismiss",
        "resolve" => "notification.resolve",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_NOTIFICATION_ACTION",
                "unknown notification action",
            ))
        }
    };
    authenticated_mutation_call(
        &state,
        &headers,
        method,
        json!({ "notification_id": notification_id }),
    )
}

async fn pack_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, pack_id, action)): AxPath<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "verify" => "job.submit",
        "review" => "review.start",
        "approve" => "decision.approve",
        "reject" => "decision.reject",
        "submit" => "job.submit",
        "rollback" => "job.submit",
        "reopen" => "pack.reopen",
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "UNKNOWN_ACTION",
                "unknown pack action",
            ))
        }
    };
    let mut params = parse_body(&body)?;
    params["pack"] = Value::String(pack_id);
    if method == "job.submit" {
        params["kind"] = Value::String(action);
    }
    mutation_call(&state, &headers, &workspace_id, method, params)
}

fn authenticated_call(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    params: Value,
) -> ApiResult {
    authenticate(headers, state)?;
    ipc_call(state, IpcRequest::new(request_id(), method, params))
}

fn project_call(
    state: &AppState,
    headers: &HeaderMap,
    workspace_id: &str,
    method: &str,
    mut params: Value,
) -> ApiResult {
    authenticate(headers, state)?;
    params["workspace_id"] = Value::String(workspace_id.into());
    ipc_call(state, IpcRequest::new(request_id(), method, params))
}

fn mutation_call(
    state: &AppState,
    headers: &HeaderMap,
    workspace_id: &str,
    method: &str,
    mut params: Value,
) -> ApiResult {
    params["workspace_id"] = Value::String(workspace_id.into());
    authenticated_mutation_call(state, headers, method, params)
}

fn authenticated_mutation_call(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    params: Value,
) -> ApiResult {
    let session = authenticate(headers, state)?;
    require_exact_origin(headers, state)?;
    let csrf = headers
        .get("x-draft-csrf")
        .and_then(|value| value.to_str().ok());
    if csrf != Some(session.csrf.as_str()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "INVALID_CSRF",
            "invalid or missing CSRF token",
        ));
    }
    let operation_id = headers
        .get("x-draft-operation-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.starts_with("op_") && value.len() <= 128)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_OPERATION_ID",
                "mutations require x-draft-operation-id",
            )
        })?;
    ipc_call(
        state,
        IpcRequest::new(request_id(), method, params).with_operation_id(operation_id),
    )
}

fn ipc_call(state: &AppState, request: IpcRequest) -> ApiResult {
    let response = call(&state.ipc_path, &request).map_err(|error| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "DAEMON_UNAVAILABLE",
            format!("draftd is unavailable: {error}"),
        )
    })?;
    if response.ok {
        Ok(Json(json!({
            "schema_version": API_ENVELOPE_VERSION,
            "data": response.result.unwrap_or(Value::Null),
        })))
    } else {
        let error = response.error.unwrap_or_else(|| {
            draft_ipc::ErrorObject::new("IPC_ERROR", "daemon returned an empty error")
        });
        Err(ApiError {
            status: status_for_error(&error.code),
            code: error.code,
            message: error.message,
            details: error.details,
        })
    }
}

fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<BrowserSession, ApiError> {
    let session_id = cookie(headers, SESSION_COOKIE).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED_SESSION",
            "Console session is missing",
        )
    })?;
    let mut sessions = state.sessions.lock().unwrap();
    sessions.retain(|_, session| session.expires_at > Instant::now());
    sessions.get(&session_id).cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED_SESSION",
            "Console session expired",
        )
    })
}

fn require_exact_origin(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if origin == Some(state.origin.as_str()) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "INVALID_ORIGIN",
            "an exact Console Origin header is required",
        ))
    }
}

fn parse_body(body: &[u8]) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            format!("request body must carry numeric schema_version {MUTATION_VERSION}"),
        ));
    }
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            error.to_string(),
        )
    })?;
    let Some(object) = value.as_object() else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "request body must be a JSON object",
        ));
    };
    match object.get("schema_version") {
        Some(Value::Number(version)) if version.as_u64() == Some(u64::from(MUTATION_VERSION)) => {
            Ok(value)
        }
        Some(Value::Number(version)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "UNSUPPORTED_SCHEMA",
            format!("schema version {version} is unsupported"),
        )),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            format!("request body must carry numeric schema_version {MUTATION_VERSION}"),
        )),
    }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

fn status_for_error(code: &str) -> StatusCode {
    match code {
        "NOT_FOUND" | "WORKSPACE_NOT_FOUND" => StatusCode::NOT_FOUND,
        "CONFLICT_DETECTED" | "OPERATION_IN_PROGRESS" => StatusCode::CONFLICT,
        "INVALID_CONFIG" | "IPC_ERROR" | "VALIDATION_ERROR" => StatusCode::BAD_REQUEST,
        "UNSUPPORTED_SCHEMA" => StatusCode::UNPROCESSABLE_ENTITY,
        "RISK_POLICY_BLOCKED" | "REVIEW_REQUIRED" | "PROTECTED_FILE_ACCESS" => {
            StatusCode::FORBIDDEN
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn origin_for(address: SocketAddr) -> (String, String) {
    match address {
        SocketAddr::V4(address) => {
            let authority = format!("{}:{}", address.ip(), address.port());
            (authority.clone(), format!("http://{authority}"))
        }
        SocketAddr::V6(address) => {
            let authority = format!("[{}]:{}", address.ip(), address.port());
            (authority.clone(), format!("http://{authority}"))
        }
    }
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn request_id() -> String {
    random_token(12)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState {
            authority: "127.0.0.1:4317".into(),
            origin: "http://127.0.0.1:4317".into(),
            bootstrap: Mutex::new(Some(BootstrapSecret {
                value: "secret".into(),
                expires_at: Instant::now() + Duration::from_secs(10),
            })),
            sessions: Mutex::new(HashMap::new()),
            ipc_path: PathBuf::from("missing.sock"),
            preselected_workspace_id: None,
        }
    }

    #[test]
    fn bootstrap_comparison_and_cookie_parsing_are_strict() {
        assert!(constant_time_eq("same", "same"));
        assert!(!constant_time_eq("same", "different"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=x; draft_console_session=abc".parse().unwrap(),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE).as_deref(), Some("abc"));
    }

    #[test]
    fn exact_origin_is_required() {
        let state = state();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, state.origin.parse().unwrap());
        assert!(require_exact_origin(&headers, &state).is_ok());
        headers.insert(header::ORIGIN, "http://localhost:4317".parse().unwrap());
        assert!(require_exact_origin(&headers, &state).is_err());
    }

    #[test]
    fn only_loopback_bindings_are_accepted_by_contract() {
        assert_eq!(
            origin_for("127.0.0.1:4317".parse().unwrap()).1,
            "http://127.0.0.1:4317"
        );
        assert_eq!(
            origin_for("[::1]:4317".parse().unwrap()).1,
            "http://[::1]:4317"
        );
    }

    #[tokio::test]
    async fn bootstrap_is_single_use_and_issues_a_hardened_host_cookie() {
        let state = Arc::new(state());
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, state.origin.parse().unwrap());
        let response = bootstrap(
            State(state.clone()),
            headers.clone(),
            Bytes::from_static(br#"{"schema_version":1,"secret":"secret"}"#),
        )
        .await
        .unwrap();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/"));
        assert!(!cookie.to_ascii_lowercase().contains("domain="));

        let replay = bootstrap(
            State(state),
            headers,
            Bytes::from_static(br#"{"schema_version":1,"secret":"secret"}"#),
        )
        .await
        .unwrap_err();
        assert_eq!(replay.code, "INVALID_BOOTSTRAP");
    }

    #[test]
    fn request_boundary_rejects_dns_rebinding_forwarders_and_cross_site_fetches() {
        let state = state();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, state.authority.parse().unwrap());
        assert!(validate_security_headers(&headers, &state).is_ok());

        headers.insert(header::HOST, "localhost:4317".parse().unwrap());
        assert_eq!(
            validate_security_headers(&headers, &state)
                .unwrap_err()
                .code,
            "INVALID_HOST"
        );
        headers.insert(header::HOST, state.authority.parse().unwrap());
        headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        assert_eq!(
            validate_security_headers(&headers, &state)
                .unwrap_err()
                .code,
            "FORWARDED_REQUEST_REJECTED"
        );
        headers.remove("x-forwarded-for");
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert_eq!(
            validate_security_headers(&headers, &state)
                .unwrap_err()
                .code,
            "CROSS_SITE_REQUEST_REJECTED"
        );
    }

    #[test]
    fn mutation_transport_requires_session_origin_csrf_and_operation_id() {
        let state = state();
        state.sessions.lock().unwrap().insert(
            "session".into(),
            BrowserSession {
                csrf: "csrf".into(),
                expires_at: Instant::now() + Duration::from_secs(10),
            },
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}=session").parse().unwrap(),
        );
        headers.insert(header::ORIGIN, state.origin.parse().unwrap());
        let error = mutation_call(&state, &headers, "ws_a", "task.create", json!({})).unwrap_err();
        assert_eq!(error.code, "INVALID_CSRF");
        headers.insert("x-draft-csrf", "csrf".parse().unwrap());
        let error = mutation_call(&state, &headers, "ws_a", "task.create", json!({})).unwrap_err();
        assert_eq!(error.code, "MISSING_OPERATION_ID");
    }

    #[test]
    fn request_schema_failures_are_distinct() {
        assert_eq!(
            parse_body(br#"{"schema_version":2}"#).unwrap_err().code,
            "UNSUPPORTED_SCHEMA"
        );
        for body in [
            br#"{}"#.as_slice(),
            br#"{"schema_version":"1"}"#.as_slice(),
            b"not-json".as_slice(),
        ] {
            assert_eq!(parse_body(body).unwrap_err().code, "VALIDATION_ERROR");
        }
    }
}
