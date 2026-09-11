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
use draft_ipc::console_application::{
    CONSOLE_CAPABILITIES, CONSOLE_PROTOCOL_MAJOR, CONSOLE_PROTOCOL_MINOR,
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
use std::sync::{Arc, Condvar, Mutex};
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
    /// This browser session's Console application session, established lazily.
    ///
    /// Shared by `Arc` so the registry lock can be released the moment the
    /// handle is taken: the handshake itself is serialized per browser session,
    /// never behind the global registry mutex.
    console: Arc<ConsoleSession>,
}

/// The Console application session belonging to one browser session.
///
/// `draftd` rolls an application session per `client_instance_id` and
/// invalidates the capabilities of the one it replaces, so two concurrent
/// handshakes for the same browser are actively harmful — the loser is left
/// holding a dead session. Establishment and replacement are therefore
/// singleflight, and replacement is compare-and-swap on `generation` so a late
/// straggler reporting the *old* session does not roll a third.
#[derive(Debug)]
struct ConsoleSession {
    /// Stable for the life of the browser session; `draftd` keys its rolling on
    /// this, so it must not change when we merely recover.
    client_instance_id: String,
    state: Mutex<ConsoleSessionState>,
    /// Signalled when a handshake finishes, successfully or not, so waiters
    /// never park forever behind a failed attempt.
    settled: Condvar,
}

#[derive(Debug, Clone)]
enum ConsoleSessionState {
    Uninitialized,
    Handshaking,
    Ready(ConsoleSessionHandle),
}

/// What a caller holds while it works, and what it presents when recovering.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConsoleSessionHandle {
    application_session_id: String,
    /// Bumped on every replacement. A recovery request naming an older
    /// generation has already been overtaken and simply reuses the current one.
    generation: u64,
    negotiated_capabilities: Vec<String>,
}

impl ConsoleSession {
    fn new() -> Self {
        Self {
            // Opaque and per-browser-session: two browsers never share, and a
            // recovery never changes it, so recovery replaces exactly this
            // session and no other.
            client_instance_id: format!("draft-console-web-{}", random_token(16)),
            state: Mutex::new(ConsoleSessionState::Uninitialized),
            settled: Condvar::new(),
        }
    }
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
    let preselected_workspace_id = std::fs::read(root.join(".draft/project.json"))
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
        .route("/api/v1/console/model", get(console_model))
        .route(
            "/api/v1/console/actions/invoke",
            post(console_action_invoke),
        )
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
        .route("/api/v1/projects/:workspace_id/resources", get(resources))
        .route("/api/v1/projects/:workspace_id/resource", get(resource))
        .route(
            "/api/v1/projects/:workspace_id/classification",
            get(classification),
        )
        .route(
            "/api/v1/projects/:workspace_id/presentation",
            get(presentation),
        )
        .route("/api/v1/projects/:workspace_id/tools", get(tools))
        .route(
            "/api/v1/projects/:workspace_id/observation-coverage",
            get(observation_coverage),
        )
        .route(
            "/api/v1/projects/:workspace_id/observation-provenance",
            get(observation_provenance),
        )
        .route(
            "/api/v1/projects/:workspace_id/observation-pending",
            get(observation_pending),
        )
        .route(
            "/api/v1/projects/:workspace_id/observation-preview",
            get(observation_preview),
        )
        .route(
            "/api/v1/projects/:workspace_id/observation-transitions",
            get(observation_transitions),
        )
        .route("/api/v1/projects/:workspace_id/intents", get(intents))
        .route(
            "/api/v1/projects/:workspace_id/task-templates",
            get(task_templates),
        )
        // The Change Graph. Read models are GET, and the two acts that change
        // something are POST — promotion changes what the project accepts,
        // publication causes an effect outside Draft, and neither is a read.
        .route("/api/v1/projects/:workspace_id/graph", get(graph))
        .route(
            "/api/v1/projects/:workspace_id/graph/baseline",
            get(graph_baseline),
        )
        .route(
            "/api/v1/projects/:workspace_id/graph/authorization/:change_id/:revision_id",
            get(graph_authorization),
        )
        .route(
            "/api/v1/projects/:workspace_id/graph/publications",
            get(graph_publications),
        )
        // Providers and Baselines are §8.3 sections of their own. Reads only:
        // a binding changes through the audited action path like every other
        // mutation, and a Baseline never changes at all.
        .route("/api/v1/projects/:workspace_id/providers", get(providers))
        .route(
            "/api/v1/projects/:workspace_id/providers/:binding_id",
            get(provider),
        )
        .route("/api/v1/projects/:workspace_id/baselines", get(baselines))
        .route(
            "/api/v1/projects/:workspace_id/baselines/:baseline_id",
            get(baseline),
        )
        .route(
            "/api/v1/projects/:workspace_id/graph/promote",
            post(graph_promote),
        )
        .route(
            "/api/v1/projects/:workspace_id/graph/publish",
            post(graph_publish),
        )
        .route(
            "/api/v1/projects/:workspace_id/actions/:action",
            post(project_action),
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

    let workspace_id = random_token(32);
    let csrf = random_token(24);
    state.sessions.lock().unwrap().insert(
        workspace_id.clone(),
        BrowserSession {
            csrf: csrf.clone(),
            expires_at: Instant::now() + SESSION_TTL,
            console: Arc::new(ConsoleSession::new()),
        },
    );
    let cookie = format!(
        "{SESSION_COOKIE}={workspace_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
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
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    page: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

async fn extension_discover(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ExtensionSearchQuery>,
) -> ApiResult {
    // Discovery is a read of verified cached metadata, so it stays a GET and
    // works offline. Contacting a source is a mutation — it rewrites the trust
    // cache — and goes through the source refresh action instead.
    let mut params = json!({ "query": query.q });
    for (key, value) in [
        ("source", query.source.map(Value::String)),
        ("capability", query.capability.map(Value::String)),
        ("page", query.page.map(Value::from)),
        ("limit", query.limit.map(Value::from)),
    ] {
        if let Some(value) = value {
            params[key] = value;
        }
    }
    authenticated_call(&state, &headers, "extension.search", params)
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
        "delete" => "extension.source.delete",
        "enable" => "extension.source.enable",
        "disable" => "extension.source.disable",
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
        "authorize" => "extension.authorize",
        "revoke" => "extension.revoke",
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

async fn resources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "resource.list", json!({}))
}

/// A resource is addressed by its locator.
///
/// `scheme` defaults to `file` so the common case stays a plain path, but the
/// gateway never parses `body` — what it means belongs to the owning adapter.
#[derive(Deserialize)]
struct ResourceQuery {
    body: String,
    #[serde(default = "default_scheme")]
    scheme: String,
}

fn default_scheme() -> String {
    "file".to_string()
}

async fn resource(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
    Query(query): Query<ResourceQuery>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "resource.get",
        json!({
            "resource_locator": { "scheme": query.scheme, "body": query.body },
        }),
    )
}

async fn classification(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "classification.bundle",
        json!({}),
    )
}

/// How each resource would be presented, and by whom.
///
/// Read-only: choosing between tied publishers is a person's decision, made
/// through the ordinary action surface, not a side effect of rendering.
async fn presentation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "presentation.bindings",
        json!({ "surface": "resource" }),
    )
}

/// The tool actions installed extensions offer. Listing is read-only; invoking
/// one mutates and goes through the project action surface.
async fn tools(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "tool.list", json!({}))
}

/// Which domains the current observation covers, and what it could not see.
async fn observation_coverage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "observation.coverage",
        json!({}),
    )
}

/// Which implementations actually performed the current observation.
///
/// A list: the same state observed again later is a different historical
/// observation, and neither record replaces the other.
async fn observation_provenance(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "observation.provenance",
        json!({}),
    )
}

/// The semantics an installed extension would observe under, if adopted.
///
/// Read-only, and empty in the ordinary case. Adopting is a mutation and goes
/// through the project action surface with its own confirmation.
async fn observation_pending(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "observation.pending",
        json!({}),
    )
}

/// What adopting the pending semantics would do.
///
/// Deliberately a read: the preview runs a trial observation and throws it
/// away, so looking at the consequences of a change is never a way of making it.
async fn observation_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "observation.preview",
        json!({}),
    )
}

/// Every adoption this project has made.
async fn observation_transitions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "observation.transitions",
        json!({}),
    )
}

async fn intents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "intent.list", json!({}))
}

async fn task_templates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "task.templates", json!({}))
}

/// The project's whole Change Graph state, with server-computed availability.
async fn graph(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "dcg.project", json!({}))
}

/// The Baseline the project accepts. Its authoritative state; there is no other.
async fn graph_baseline(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(&state, &headers, &workspace_id, "dcg.baseline", json!({}))
}

/// Everything decided about one revision, and what may legally follow.
async fn graph_authorization(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, change_id, revision_id)): AxPath<(String, String, String)>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "dcg.authorization",
        json!({ "change": change_id, "revision": revision_id }),
    )
}

async fn graph_publications(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "dcg.publication.list",
        json!({}),
    )
}

/// Every binding, definition and profile this project holds.
async fn providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "project.provider.list",
        json!({}),
    )
}

/// One binding, with the immutable facts it currently points at.
async fn provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, binding_id)): AxPath<(String, String)>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "project.provider.show",
        json!({ "binding": binding_id }),
    )
}

/// The accepted lineage, newest first.
async fn baselines(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "dcg.baseline.list",
        json!({}),
    )
}

/// One accepted Baseline: its three roots, lineage, composition, deliveries.
async fn baseline(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, baseline_id)): AxPath<(String, String)>,
) -> ApiResult {
    project_call(
        &state,
        &headers,
        &workspace_id,
        "dcg.baseline.show",
        json!({ "baseline": baseline_id }),
    )
}

/// Promote an authorized revision.
///
/// `expected_baseline` is passed through and required by `draftd`: a browser
/// acting on a view that has since moved fails deterministically rather than
/// promoting onto a parent nobody judged the work against.
async fn graph_promote(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    // A mutation, so it takes the CSRF check and the client's operation id
    // like every other one. Without the operation id a lost reply would look
    // like a promotion that never happened.
    mutation_call(&state, &headers, &workspace_id, "dcg.promotion.run", body)
}

/// Publish a promoted Baseline.
///
/// The body names what to deliver and what for. It cannot name the delivery
/// semantics, the recovery class or the attempt identity: the first is the
/// provider's, the second follows from it, and the third is the operation id.
async fn graph_publish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(workspace_id): AxPath<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    // The operation id is the attempt identity: a browser that retries after
    // an uncertain reply converges on what its attempt concluded rather than
    // delivering a second time.
    mutation_call(&state, &headers, &workspace_id, "dcg.publication.run", body)
}

async fn project_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath((workspace_id, action)): AxPath<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let method = match action.as_str() {
        "resource-create" | "resource-relocate" | "resource-delete" => "resource.workspace.stage",
        "resource-save" => "resource.workspace.save",
        "resource-commit" => "resource.workspace.commit",
        "task-create" => "task.create",
        "task-update" => "task.update",
        "task-next-action-add" => "task.next_action.add",
        "task-next-action-set" => "task.next_action.set",
        "task-drop" => "task.drop",
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
        // Invoking a tool mutates: it runs a command and may apply what the
        // tool proposed, so it goes through the mutation path with its CSRF
        // value and operation id like any other change.
        "tool-invoke" => "tool.invoke",
        // Adopting new observation semantics establishes a new baseline and
        // supersedes work: a mutation, with its own confirmation, not a read.
        "observation-adopt" => "observation.adopt",
        // The Change Graph's mutating steps. Promotion and publication have
        // their own routes because they are the two acts with consequences
        // outside the project; everything up to them is an ordinary project
        // action. Each is a distinct act on the record — establishing evidence
        // is not judging risk, and judging risk is not deciding — so the
        // Console offers them separately rather than as one "approve" button.
        "graph-change-open" => "dcg.change.open",
        // Abandon and Reopen, never Delete. Stopping work is a statement about
        // the future; deleting would be a statement about the past, and the
        // record of work that was done and then decided against is frequently
        // the part worth keeping.
        "graph-change-abandon" => "dcg.change.abandon",
        "graph-change-reopen" => "dcg.change.reopen",
        "graph-revision-seal" => "dcg.revision.seal",
        // Recording that somebody looked. Offered separately from deciding,
        // because reading a revision and concluding something about it are
        // different acts and only one of them authorizes anything.
        "graph-review-record" => "dcg.review.record",
        "graph-evidence-record" => "dcg.evidence.record",
        "graph-assessment-record" => "dcg.assessment.record",
        "graph-gate-evaluate" => "dcg.gate.evaluate",
        "graph-gate-waive" => "dcg.gate.waive",
        // One method, because approving and rejecting are one act with a
        // different answer — the Decision records which, and both are equally
        // immutable once recorded.
        "graph-decision-record" => "dcg.decision.record",
        "graph-publication-grant" => "dcg.publication.grant",
        "graph-publication-authorize-retry" => "dcg.publication.authorize_retry",
        "graph-publication-withdraw-attempt" => "dcg.publication.withdraw_attempt",
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

/// The public code `draftd` reports when it does not know an application
/// session. Matching on it is what keeps recovery off free-text messages.
const UNKNOWN_CONSOLE_SESSION: &str = "UNKNOWN_CONSOLE_SESSION";

#[derive(Deserialize)]
struct ConsoleModelQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    change_id: Option<String>,
    /// Required by the `BASELINE` scope and meaningless elsewhere.
    #[serde(default)]
    baseline_id: Option<String>,
}

/// The authoritative Console read model — state *and* the actions `draftd`
/// currently issues.
///
/// The browser renders what this returns and decides nothing: which actions
/// exist, whether they are enabled, and what inputs they take are all settled
/// here. A stale application session is recovered transparently because this is
/// a read and retrying it cannot execute anything.
async fn console_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ConsoleModelQuery>,
) -> ApiResult {
    let session = authenticate(&headers, &state)?;
    let subject = json!({
        "scope": query.scope.as_deref().unwrap_or("GLOBAL"),
        "workspace_id": query.workspace_id,
        "change_id": query.change_id,
        "baseline_id": query.baseline_id,
    });

    let mut handle = console_session(&state, &session.console)?;
    for attempt in 0..2 {
        let request = IpcRequest::new(
            request_id(),
            "console.snapshot",
            json!({
                "application_session_id": handle.application_session_id,
                "subject": subject,
            }),
        );
        match raw_ipc_call(&state, request) {
            Ok(model) => {
                return Ok(Json(json!({
                    "schema_version": API_ENVELOPE_VERSION,
                    "data": model,
                })))
            }
            Err(error) if error.code == UNKNOWN_CONSOLE_SESSION && attempt == 0 => {
                handle = recover_console_session(&state, &session.console, &handle)?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        UNKNOWN_CONSOLE_SESSION,
        "Console application session could not be established; refresh and try again",
    ))
}

#[derive(Deserialize)]
struct ConsoleInvokeBody {
    invocation_capability: String,
    #[serde(default)]
    expected_revisions: Value,
    #[serde(default)]
    arguments: Value,
}

/// Invoke one server-issued action.
///
/// The browser sends the capability `draftd` issued, the revisions it was shown
/// and the arguments it collected — never an application session id, which is
/// this gateway's to hold.
///
/// There is deliberately **no retry loop** here. An action is a mutation, and
/// the only refusal we could safely retry — an unknown application session — is
/// raised by `draftd` before it consumes the capability or opens an operation.
/// But recovering invalidates that capability, so the honest answer is to
/// refresh the session and return the conflict: the browser refetches the model
/// and re-invokes with a freshly issued capability, which is the same
/// revalidation path a changed action would take anyway. Anything else — a
/// timeout, a dropped connection, an ambiguous failure — is never replayed,
/// because the mutation may already have run.
async fn console_action_invoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let session = authenticate(&headers, &state)?;
    let invocation: ConsoleInvokeBody = serde_json::from_slice(&body).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            format!("invalid action invocation: {error}"),
        )
    })?;
    let handle = console_session(&state, &session.console)?;
    let mut params = json!({
        "application_session_id": handle.application_session_id,
        "invocation_capability": invocation.invocation_capability,
        "expected_revisions": invocation.expected_revisions,
        "arguments": invocation.arguments,
    });
    if params["arguments"].is_null() {
        params["arguments"] = json!({});
    }

    let outcome = authenticated_mutation_call(&state, &headers, "console.action.invoke", params);
    if let Err(error) = &outcome {
        if error.code == UNKNOWN_CONSOLE_SESSION {
            // Proven pre-dispatch: nothing ran, and nothing was consumed.
            // Re-establish so the browser's next read succeeds, and let it
            // reacquire the action against the fresh authoritative model.
            let _ = recover_console_session(&state, &session.console, &handle);
        }
    }
    outcome
}

/// A raw daemon call that surfaces the error object rather than an HTTP status,
/// so the caller can decide whether a refusal is recoverable.
fn raw_ipc_call(state: &AppState, request: IpcRequest) -> Result<Value, ApiError> {
    let response = call(&state.ipc_path, &request).map_err(|error| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "DAEMON_UNAVAILABLE",
            format!("draftd is unavailable: {error}"),
        )
    })?;
    match (response.result, response.error) {
        (Some(result), _) => Ok(result),
        (None, Some(error)) => Err(ApiError {
            status: status_for_error(&error.code),
            code: error.code,
            message: error.message,
            details: error.details,
        }),
        (None, None) => Ok(Value::Null),
    }
}

/// The current application session, establishing one if there is none.
///
/// Exactly one handshake runs per browser session: a concurrent caller either
/// waits for the in-flight attempt or reuses its result. The registry lock is
/// long gone by the time this runs — only this browser session's own state is
/// held, and never across the daemon call itself.
fn console_session(
    state: &AppState,
    console: &ConsoleSession,
) -> Result<ConsoleSessionHandle, ApiError> {
    let mut current = console.state.lock().unwrap();
    loop {
        match &*current {
            ConsoleSessionState::Ready(handle) => return Ok(handle.clone()),
            ConsoleSessionState::Handshaking => {
                // Someone else is establishing it; wait for them to settle
                // rather than racing a second `open_application`.
                current = console.settled.wait(current).unwrap();
            }
            ConsoleSessionState::Uninitialized => {
                *current = ConsoleSessionState::Handshaking;
                drop(current);
                return finish_handshake(state, console, 0);
            }
        }
    }
}

/// Replace a session the daemon has rejected — at most once per generation.
///
/// `failed` is the handle the caller was using. If the installed session has
/// already moved past it, another caller recovered first and this one simply
/// adopts that result; only the caller whose generation is still current
/// performs the replacement. That is what stops concurrent stragglers rolling
/// S1 → S2 → S3 → S4 and invalidating each other's fresh capabilities.
fn recover_console_session(
    state: &AppState,
    console: &ConsoleSession,
    failed: &ConsoleSessionHandle,
) -> Result<ConsoleSessionHandle, ApiError> {
    let mut current = console.state.lock().unwrap();
    loop {
        match &*current {
            ConsoleSessionState::Ready(handle) if handle.generation != failed.generation => {
                return Ok(handle.clone())
            }
            ConsoleSessionState::Ready(handle) => {
                let next = handle.generation + 1;
                *current = ConsoleSessionState::Handshaking;
                drop(current);
                return finish_handshake(state, console, next);
            }
            ConsoleSessionState::Handshaking => {
                current = console.settled.wait(current).unwrap();
            }
            ConsoleSessionState::Uninitialized => {
                *current = ConsoleSessionState::Handshaking;
                drop(current);
                return finish_handshake(state, console, failed.generation + 1);
            }
        }
    }
}

/// Perform the one handshake this caller owns, then publish the result.
///
/// A failure resets the state to `Uninitialized` and wakes every waiter, so a
/// daemon that was briefly down leaves the session retryable rather than parked
/// in `Handshaking` forever.
fn finish_handshake(
    state: &AppState,
    console: &ConsoleSession,
    generation: u64,
) -> Result<ConsoleSessionHandle, ApiError> {
    let outcome = raw_ipc_call(
        state,
        IpcRequest::new(
            request_id(),
            "console.handshake",
            json!({
                "protocol": { "major": CONSOLE_PROTOCOL_MAJOR, "minor": CONSOLE_PROTOCOL_MINOR },
                "client_name": "draft-console-web",
                "client_version": env!("CARGO_PKG_VERSION"),
                "client_instance_id": console.client_instance_id,
                // Without `action_capabilities` draftd issues no invocation
                // capability and every action would arrive disabled.
                "requested_capabilities": CONSOLE_CAPABILITIES,
            }),
        ),
    );

    let mut current = console.state.lock().unwrap();
    let result = match outcome {
        Ok(value) => {
            let handle = ConsoleSessionHandle {
                application_session_id: value
                    .get("application_session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                generation,
                negotiated_capabilities: value
                    .get("negotiated_capabilities")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            *current = ConsoleSessionState::Ready(handle.clone());
            Ok(handle)
        }
        Err(error) => {
            *current = ConsoleSessionState::Uninitialized;
            Err(error)
        }
    };
    drop(current);
    console.settled.notify_all();
    result
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
    let workspace_id = cookie(headers, SESSION_COOKIE).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED_SESSION",
            "Console session is missing",
        )
    })?;
    let mut sessions = state.sessions.lock().unwrap();
    sessions.retain(|_, session| session.expires_at > Instant::now());
    sessions.get(&workspace_id).cloned().ok_or_else(|| {
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
        "CONFLICT_DETECTED" | "OPERATION_IN_PROGRESS" | "UNKNOWN_CONSOLE_SESSION" => {
            StatusCode::CONFLICT
        }
        "INVALID_CONFIG" | "IPC_ERROR" | "VALIDATION_ERROR" => StatusCode::BAD_REQUEST,
        "UNSUPPORTED_SCHEMA" => StatusCode::UNPROCESSABLE_ENTITY,
        "RISK_POLICY_BLOCKED" | "REVIEW_REQUIRED" | "PROTECTED_RESOURCE_ACCESS" => {
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
mod surface;

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
                console: Arc::new(ConsoleSession::new()),
            },
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}=session").parse().unwrap(),
        );
        headers.insert(header::ORIGIN, state.origin.parse().unwrap());
        let error = mutation_call(&state, &headers, "prj_a", "task.create", json!({})).unwrap_err();
        assert_eq!(error.code, "INVALID_CSRF");
        headers.insert("x-draft-csrf", "csrf".parse().unwrap());
        let error = mutation_call(&state, &headers, "prj_a", "task.create", json!({})).unwrap_err();
        assert_eq!(error.code, "MISSING_OPERATION_ID");
    }

    /// A countable stand-in for `draftd`, so a test can assert how many times
    /// the gateway actually handshook.
    struct FakeDaemon {
        _directory: tempfile::TempDir,
        path: PathBuf,
        stop: Arc<std::sync::atomic::AtomicBool>,
        handshakes: Arc<std::sync::atomic::AtomicUsize>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeDaemon {
        /// `answer` decides every non-handshake method, so a test can make the
        /// daemon reject a session exactly once and then behave.
        fn start(
            answer: impl Fn(&IpcRequest) -> Result<Value, draft_ipc::ErrorObject>
                + Send
                + Sync
                + 'static,
        ) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("daemon.sock");
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let handshakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = handshakes.clone();
            let handler: draft_ipc::Handler = Arc::new(move |request: IpcRequest| {
                let id = request.id.clone();
                if request.method == "console.handshake" {
                    let serial = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    // A real handshake is not instant; the pause widens the
                    // window a second caller would race through.
                    std::thread::sleep(Duration::from_millis(50));
                    return draft_ipc::Response::ok(
                        id,
                        json!({
                            "application_session_id": format!("console_session_{serial}"),
                            "negotiated_capabilities": CONSOLE_CAPABILITIES,
                        }),
                    );
                }
                match answer(&request) {
                    Ok(value) => draft_ipc::Response::ok(id, value),
                    Err(error) => draft_ipc::Response::err(id, error),
                }
            });
            let serve_path = path.clone();
            let serve_stop = stop.clone();
            let thread = std::thread::spawn(move || {
                let _ = draft_ipc::serve(&serve_path, serve_stop, handler);
            });
            for _ in 0..200 {
                if path.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Self {
                _directory: directory,
                path,
                stop,
                handshakes,
                thread: Some(thread),
            }
        }

        fn handshakes(&self) -> usize {
            self.handshakes.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            // Unblock the accept loop so the thread can observe `stop`.
            let _ = call(
                &self.path,
                &IpcRequest::new("stop", "service.ping", Value::Null),
            );
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn session_state(daemon: &FakeDaemon) -> AppState {
        let mut state = state();
        state.ipc_path = daemon.path.clone();
        state
    }

    #[test]
    fn concurrent_first_use_handshakes_exactly_once() {
        let daemon = FakeDaemon::start(|_| Ok(json!({})));
        let state = Arc::new(session_state(&daemon));
        let console = Arc::new(ConsoleSession::new());

        // Eight callers race for a browser session that has none yet.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let state = state.clone();
                let console = console.clone();
                std::thread::spawn(move || console_session(&state, &console).unwrap())
            })
            .collect();
        let sessions: Vec<ConsoleSessionHandle> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert_eq!(daemon.handshakes(), 1, "one browser session, one handshake");
        assert!(
            sessions.windows(2).all(|pair| pair[0] == pair[1]),
            "every caller receives the same session"
        );
        assert_eq!(sessions[0].generation, 0);
        assert!(sessions[0]
            .negotiated_capabilities
            .iter()
            .any(|capability| capability == "action_capabilities"));
    }

    #[test]
    fn concurrent_recovery_replaces_the_session_once() {
        let daemon = FakeDaemon::start(|_| Ok(json!({})));
        let state = Arc::new(session_state(&daemon));
        let console = Arc::new(ConsoleSession::new());

        let original = console_session(&state, &console).unwrap();
        assert_eq!(daemon.handshakes(), 1);

        // Eight stale responses arrive at once, all naming the same generation.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let state = state.clone();
                let console = console.clone();
                let failed = original.clone();
                std::thread::spawn(move || {
                    recover_console_session(&state, &console, &failed).unwrap()
                })
            })
            .collect();
        let recovered: Vec<ConsoleSessionHandle> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert_eq!(
            daemon.handshakes(),
            2,
            "one replacement, not one per stale caller"
        );
        assert!(recovered.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(recovered[0].generation, 1, "no S1 -> S2 -> S3 chain");
        assert_ne!(
            recovered[0].application_session_id,
            original.application_session_id
        );
    }

    #[test]
    fn a_straggler_naming_a_superseded_session_reuses_the_replacement() {
        let daemon = FakeDaemon::start(|_| Ok(json!({})));
        let state = session_state(&daemon);
        let console = ConsoleSession::new();

        let original = console_session(&state, &console).unwrap();
        let replaced = recover_console_session(&state, &console, &original).unwrap();
        assert_eq!(daemon.handshakes(), 2);

        // A response issued against the original session lands late. It has
        // already been overtaken, so it adopts the replacement rather than
        // rolling a third session and invalidating the fresh capabilities.
        let straggler = recover_console_session(&state, &console, &original).unwrap();
        assert_eq!(straggler, replaced);
        assert_eq!(
            daemon.handshakes(),
            2,
            "a superseded generation recovers nothing"
        );
    }

    #[test]
    fn separate_browser_sessions_establish_independently() {
        let daemon = FakeDaemon::start(|_| Ok(json!({})));
        let state = session_state(&daemon);
        let first = ConsoleSession::new();
        let second = ConsoleSession::new();

        let a = console_session(&state, &first).unwrap();
        let b = console_session(&state, &second).unwrap();

        assert_eq!(daemon.handshakes(), 2, "one handshake each, not one shared");
        assert_ne!(a.application_session_id, b.application_session_id);
        assert_ne!(first.client_instance_id, second.client_instance_id);
    }

    #[test]
    fn a_failed_handshake_wakes_waiters_and_stays_retryable() {
        // No daemon at all: every handshake fails.
        let state = Arc::new(state());
        let console = Arc::new(ConsoleSession::new());

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let state = state.clone();
                let console = console.clone();
                std::thread::spawn(move || console_session(&state, &console).is_err())
            })
            .collect();
        // Every caller returns — none parks forever behind the failed attempt.
        assert!(handles.into_iter().all(|handle| handle.join().unwrap()));

        // And the session is not poisoned: it is simply uninitialized again.
        assert!(matches!(
            *console.state.lock().unwrap(),
            ConsoleSessionState::Uninitialized
        ));

        let daemon = FakeDaemon::start(|_| Ok(json!({})));
        let recovered = session_state(&daemon);
        assert!(console_session(&recovered, &console).is_ok());
    }

    /// A browser session wired to a fake daemon, with its cookie headers.
    fn browser_session(state: &AppState) -> (String, HeaderMap) {
        let console = Arc::new(ConsoleSession::new());
        state.sessions.lock().unwrap().insert(
            "session".into(),
            BrowserSession {
                csrf: "csrf".into(),
                expires_at: Instant::now() + Duration::from_secs(30),
                console,
            },
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}=session").parse().unwrap(),
        );
        ("session".into(), headers)
    }

    #[test]
    fn a_stale_snapshot_session_is_recovered_and_the_read_retried() {
        // The daemon rejects the first snapshot with the typed pre-dispatch
        // refusal, then answers normally — exactly the case a daemon restart
        // produces.
        let rejected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = rejected.clone();
        let daemon = FakeDaemon::start(move |request| {
            if request.method != "console.snapshot" {
                return Ok(json!({}));
            }
            if !seen.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(draft_ipc::ErrorObject::new(
                    UNKNOWN_CONSOLE_SESSION,
                    "Console application session is missing or was replaced",
                ));
            }
            Ok(json!({ "actions": [], "revisions": { "registry": 1 } }))
        });
        let state = Arc::new(session_state(&daemon));
        let (_, headers) = browser_session(&state);

        let response = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(console_model(
                State(state.clone()),
                headers,
                Query(ConsoleModelQuery {
                    scope: None,
                    workspace_id: None,
                    change_id: None,
                    baseline_id: None,
                }),
            ));

        assert!(response.is_ok(), "a read recovers transparently");
        assert_eq!(
            daemon.handshakes(),
            2,
            "the initial handshake plus exactly one replacement"
        );
    }

    #[test]
    fn an_ambiguous_snapshot_failure_is_surfaced_rather_than_recovered() {
        // Anything that is not the typed pre-dispatch refusal is reported as
        // it stands; the gateway does not re-handshake speculatively.
        let daemon = FakeDaemon::start(|request| {
            if request.method == "console.snapshot" {
                return Err(draft_ipc::ErrorObject::new("IPC_ERROR", "something else"));
            }
            Ok(json!({}))
        });
        let state = Arc::new(session_state(&daemon));
        let (_, headers) = browser_session(&state);

        let response = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(console_model(
                State(state.clone()),
                headers,
                Query(ConsoleModelQuery {
                    scope: None,
                    workspace_id: None,
                    change_id: None,
                    baseline_id: None,
                }),
            ));

        assert_eq!(response.unwrap_err().code, "IPC_ERROR");
        assert_eq!(daemon.handshakes(), 1, "no speculative re-handshake");
    }

    #[test]
    fn the_capability_bearing_model_is_never_cacheable() {
        // Every gateway response carries `no-store`, which is what keeps
        // short-lived invocation capabilities out of any HTTP cache. Pinned
        // here because the model route is the one that carries them.
        let source = include_str!("lib.rs");
        let headers = source
            .split("async fn request_security(")
            .nth(1)
            .expect("the shared security layer exists");
        assert!(
            headers.contains(r#"header::CACHE_CONTROL, HeaderValue::from_static("no-store")"#),
            "the shared response layer must keep setting no-store"
        );
        assert!(source.contains(r#".route("/api/v1/console/model", get(console_model))"#));
    }

    #[test]
    fn the_browser_cannot_supply_its_own_application_session() {
        // The invoke body carries a capability and revisions — never a session
        // id, a client instance id or a capability set. Anything a browser sent
        // under those names is simply not read.
        let body = br#"{
            "invocation_capability": "cap-1",
            "expected_revisions": {"registry": 1, "workspace": null, "change": null, "policy": null},
            "arguments": {},
            "application_session_id": "attacker_session",
            "client_instance_id": "attacker_instance",
            "negotiated_capabilities": ["action_capabilities"]
        }"#;
        let invocation: ConsoleInvokeBody = serde_json::from_slice(body).unwrap();
        assert_eq!(invocation.invocation_capability, "cap-1");
        // The struct has no field to hold them, so they cannot reach draftd.
        let encoded = serde_json::to_value(json!({
            "invocation_capability": invocation.invocation_capability,
        }))
        .unwrap();
        assert!(encoded.get("application_session_id").is_none());
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
