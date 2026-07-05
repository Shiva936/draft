//! AG-UI Review Cockpit backend (PRD §9.19, TDD §39, §44).
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
    http::{HeaderMap, StatusCode},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use draft_core::App;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("index.html");

struct AppState {
    root: PathBuf,
    csrf: String,
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

/// Start the cockpit, blocking until Ctrl-C. `bind` is typically `127.0.0.1`.
pub fn serve(root: PathBuf, bind: &str, port: u16) -> Result<(), String> {
    let csrf = random_token();
    let state = Arc::new(AppState { root, csrf });
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
        println!("Draft Review Cockpit → http://{addr}  (Ctrl-C to stop)");
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
        .route("/packs", get(list_packs))
        .route("/packs/:id", get(get_pack))
        .route("/packs/:id/diff", get(get_diff))
        .route("/packs/:id/risk", get(get_risk))
        .route("/packs/:id/receipts", get(get_receipts))
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
    Html(INDEX_HTML.replace("__CSRF__", &st.csrf))
}

async fn list_packs(State(st): State<Arc<AppState>>) -> ApiResult {
    json(App::new().list_canonical_packs(&st.root))
}

async fn get_pack(State(st): State<Arc<AppState>>, AxPath(id): AxPath<String>) -> ApiResult {
    json(App::new().pack_inspect(&st.root, &id))
}

async fn get_diff(State(st): State<Arc<AppState>>, AxPath(id): AxPath<String>) -> ApiResult {
    App::new()
        .pack_diff_text(&st.root, &id)
        .map(|s| Json(Value::String(s)))
        .map_err(to_http)
}

async fn get_risk(State(st): State<Arc<AppState>>, AxPath(id): AxPath<String>) -> ApiResult {
    App::new()
        .pack_risk_json(&st.root, &id)
        .map(Json)
        .map_err(to_http)
}

async fn get_receipts(State(st): State<Arc<AppState>>, AxPath(id): AxPath<String>) -> ApiResult {
    json(App::new().pack_receipts_v2(&st.root, &id))
}

async fn get_events(State(st): State<Arc<AppState>>) -> ApiResult {
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
    check_csrf(&headers, &st)?;
    let reason = body.and_then(|b| b.0.reason);
    App::new()
        .cockpit_decide(&st.root, &id, true, reason)
        .map(|pid| Json(serde_json::json!({ "approved": pid })))
        .map_err(to_http)
}

async fn reject(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
    body: Option<Json<DecideBody>>,
) -> ApiResult {
    check_csrf(&headers, &st)?;
    let reason = body.and_then(|b| b.0.reason);
    App::new()
        .cockpit_decide(&st.root, &id, false, reason)
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
    check_csrf(&headers, &st)?;
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
    check_csrf(&headers, &st)?;
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

    #[test]
    fn csrf_enforced() {
        let st = AppState {
            root: PathBuf::from("."),
            csrf: "secret".into(),
        };
        let mut ok = HeaderMap::new();
        ok.insert("x-draft-csrf", "secret".parse().unwrap());
        assert!(check_csrf(&ok, &st).is_ok());
        let bad = HeaderMap::new();
        assert!(check_csrf(&bad, &st).is_err());
    }

    #[test]
    fn index_injects_csrf_and_hides_no_keys() {
        let st = AppState {
            root: PathBuf::from("."),
            csrf: "tok123".into(),
        };
        let html = INDEX_HTML.replace("__CSRF__", &st.csrf);
        assert!(html.contains("tok123"));
        assert!(!html.contains("signing.key"));
    }

    #[test]
    fn random_tokens_differ() {
        assert_ne!(random_token(), random_token());
    }
}
