//! Sidecar HTTP daemon for trusty-analyzer.
//!
//! Why: Keeps analysis isolated from trusty-search. The daemon fetches chunks
//! from the search daemon over HTTP (`TrustySearchClient::get_chunks`) and
//! computes complexity / smells / quality / facts in-process. It does not
//! talk to trusty-search's redb files directly — the search daemon is the
//! single source of truth for chunk data.
//!
//! What: an axum router with a small surface:
//! - `GET  /health`
//! - `GET  /indexes`                            proxy to trusty-search
//! - `GET  /indexes/:id/complexity_hotspots`    top-N by cyclomatic
//! - `GET  /indexes/:id/smells`                 chunks with at least one smell
//! - `GET  /indexes/:id/quality`                aggregate report
//! - `GET  /facts`                              list / filter facts
//! - `POST /facts`                              upsert a fact
//! - `DELETE /facts/:id`                        delete a fact
//!
//! Test: `cargo test -p trusty-analyzer-service` boots the router with a stub
//! search client and exercises every route end-to-end.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use trusty_analyzer_core::{
    facts::new_fact, quality, AnalyzerRegistry, FactStore, IndexSummary, TrustySearchClient,
};
use trusty_common::{KgGraph, KgNode};

/// Default port the analyzer daemon binds to. Picked to sit next to
/// trusty-search's 7878.
pub const DEFAULT_PORT: u16 = 7879;

/// Shared state for every handler. Cheap to clone (everything is `Arc`-ish).
#[derive(Clone)]
pub struct AnalyzerAppState {
    pub search: TrustySearchClient,
    pub facts: FactStore,
    pub registry: Arc<AnalyzerRegistry>,
}

impl AnalyzerAppState {
    pub fn new(search: TrustySearchClient, facts: FactStore) -> Self {
        Self {
            search,
            facts,
            registry: Arc::new(AnalyzerRegistry::default_registry()),
        }
    }

    /// Construct with an explicit registry (useful for tests and plug-ins).
    pub fn with_registry(
        search: TrustySearchClient,
        facts: FactStore,
        registry: Arc<AnalyzerRegistry>,
    ) -> Self {
        Self {
            search,
            facts,
            registry,
        }
    }
}

/// Build the axum router around `state`.
pub fn build_router(state: AnalyzerAppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/indexes", get(list_indexes))
        .route("/indexes/:id/complexity_hotspots", get(complexity_hotspots))
        .route("/indexes/:id/smells", get(smells))
        .route("/indexes/:id/quality", get(quality_report))
        .route("/indexes/:id/graph", get(graph_for_index))
        .route("/indexes/:id/entities", get(entities_for_index))
        .route("/facts", get(list_facts).post(upsert_fact))
        .route("/facts/:id", delete(delete_fact))
        .with_state(Arc::new(state))
}

/// Bind to `addr` (or auto-pick a free port from `start_port` upward) and run
/// the daemon until the future returns. The actually-bound `SocketAddr` is
/// returned so callers can advertise it.
pub async fn serve(state: AnalyzerAppState, start_port: u16) -> Result<()> {
    let addr = pick_port(start_port).await?;
    let listener = TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    tracing::info!("trusty-analyzer listening on http://{actual}");
    let app = build_router(state);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Find the lowest port >= `start` that we can bind. Mirrors the trusty-search
/// "auto-detect free port" UX.
async fn pick_port(start: u16) -> Result<SocketAddr> {
    for port in start..start.saturating_add(64) {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        if TcpListener::bind(addr).await.is_ok() {
            // Re-bind in `serve()` after returning — this is a probe.
            return Ok(addr);
        }
    }
    Err(anyhow::anyhow!(
        "no free port found between {start} and {}",
        start.saturating_add(64)
    ))
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    search_reachable: bool,
}

/// Why: Reflects the hard runtime dependency on trusty-search — there is no
/// meaningful "ok" state when the search daemon is unreachable.
/// What: Probes trusty-search GET /health; returns 200 + "ok" when reachable,
/// 503 + "degraded" when not.
/// Test: point the client at a dead search URL and assert HTTP 503 with
/// `status == "degraded"` and `search_reachable == false`.
async fn health(
    State(state): State<Arc<AnalyzerAppState>>,
) -> Result<Json<HealthResponse>, (StatusCode, Json<HealthResponse>)> {
    let search_reachable = state.search.health().await.unwrap_or(false);
    let response = HealthResponse {
        status: if search_reachable { "ok" } else { "degraded" },
        version: env!("CARGO_PKG_VERSION"),
        search_reachable,
    };
    if search_reachable {
        Ok(Json(response))
    } else {
        Err((StatusCode::SERVICE_UNAVAILABLE, Json(response)))
    }
}

async fn list_indexes(
    State(state): State<Arc<AnalyzerAppState>>,
) -> Result<Json<Vec<IndexSummary>>, StatusCode> {
    state.search.list_indexes().await.map(Json).map_err(|e| {
        tracing::warn!("list_indexes proxy failed: {e:#}");
        StatusCode::BAD_GATEWAY
    })
}

#[derive(Deserialize)]
pub struct HotspotsParams {
    #[serde(default = "default_top_n")]
    pub top_n: usize,
}

fn default_top_n() -> usize {
    20
}

async fn complexity_hotspots(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    Query(params): Query<HotspotsParams>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    let hotspots = quality::complexity_hotspots(&chunks, params.top_n);
    Ok(Json(serde_json::json!({
        "index_id": id,
        "top_n": params.top_n,
        "hotspots": hotspots,
    })))
}

async fn smells(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    let smelly = quality::smelly_chunks(&chunks);
    Ok(Json(serde_json::json!({
        "index_id": id,
        "count": smelly.len(),
        "chunks": smelly,
    })))
}

async fn quality_report(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
) -> Result<Json<quality::QualityReport>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    Ok(Json(quality::aggregate_quality(&chunks)))
}

#[derive(Deserialize)]
pub struct GraphQueryParams {
    /// Restrict to a single language (`"rust"`, `"typescript"`, ...).
    pub language: Option<String>,
}

/// Why: Phase 2 surfaces the language-neutral knowledge graph to consumers
/// (Claude Code, web UIs, etc.) so they can navigate symbols across files.
/// What: Fetch chunks for `index`, run the language registry, optionally
/// filter to `?language=`, and return the merged `KgGraph` as JSON.
/// Test: with a mock index containing a Rust chunk, GET returns at least
/// one Function node tagged `language=rust`.
async fn graph_for_index(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    Query(params): Query<GraphQueryParams>,
) -> Result<Json<KgGraph>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    let res = state.registry.analyze(&chunks);
    let mut graph = res.graph;
    if let Some(lang) = params.language.as_deref() {
        let keep_nodes: std::collections::HashSet<String> = graph
            .nodes
            .iter()
            .filter(|n| n.language == lang)
            .map(|n| n.id.clone())
            .collect();
        graph.nodes.retain(|n| keep_nodes.contains(&n.id));
        graph
            .edges
            .retain(|e| keep_nodes.contains(&e.from) && keep_nodes.contains(&e.to));
    }
    Ok(Json(graph))
}

#[derive(Deserialize)]
pub struct EntitiesQueryParams {
    pub kind: Option<String>,
    pub language: Option<String>,
}

/// Why: Many consumers only want a flat node listing, sorted, for browsing
/// (autocomplete, file outlines).
/// What: Same pipeline as `/graph`, but returns just `Vec<KgNode>` sorted by
/// `(kind, name)`. Optional `?kind=` and `?language=` filters.
/// Test: filtering by `kind=Function` returns only Function nodes.
async fn entities_for_index(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    Query(params): Query<EntitiesQueryParams>,
) -> Result<Json<Vec<KgNode>>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    let res = state.registry.analyze(&chunks);
    let mut nodes = res.graph.nodes;
    if let Some(lang) = params.language.as_deref() {
        nodes.retain(|n| n.language == lang);
    }
    if let Some(kind) = params.kind.as_deref() {
        nodes.retain(|n| format!("{:?}", n.kind) == kind);
    }
    nodes.sort_by(|a, b| {
        format!("{:?}", a.kind)
            .cmp(&format!("{:?}", b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(Json(nodes))
}

async fn fetch_chunks(
    state: &AnalyzerAppState,
    id: &str,
) -> Result<Vec<trusty_common::CodeChunk>, StatusCode> {
    state.search.get_chunks(id).await.map_err(|e| {
        tracing::warn!("get_chunks({id}) failed: {e:#}");
        StatusCode::BAD_GATEWAY
    })
}

#[derive(Deserialize)]
pub struct FactQueryParams {
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
}

async fn list_facts(
    State(state): State<Arc<AnalyzerAppState>>,
    Query(p): Query<FactQueryParams>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let hits = state
        .facts
        .query(
            p.subject.as_deref(),
            p.predicate.as_deref(),
            p.object.as_deref(),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let count = hits.len();
    Ok(Json(serde_json::json!({ "facts": hits, "count": count })))
}

#[derive(Deserialize)]
pub struct UpsertFactRequest {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub index_id: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub provenance: Vec<String>,
}

fn default_confidence() -> f32 {
    1.0
}

async fn upsert_fact(
    State(state): State<Arc<AnalyzerAppState>>,
    Json(req): Json<UpsertFactRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut fact = new_fact(req.subject, req.predicate, req.object, req.index_id);
    fact.confidence = req.confidence.clamp(0.0, 1.0);
    fact.provenance = req.provenance;
    let id = fact.id;
    state
        .facts
        .upsert(fact)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "id": id, "upserted": true })))
}

async fn delete_fact(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let removed = state
        .facts
        .delete(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "id": id, "removed": removed })))
}

/// Re-export so the binary can construct facts via the same path.
pub use trusty_common::FactRecord as PublicFactRecord;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request};
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_state() -> (AnalyzerAppState, TempDir) {
        let tmp = TempDir::new().unwrap();
        let facts = FactStore::open(&tmp.path().join("facts.redb")).unwrap();
        let search = TrustySearchClient::new("http://127.0.0.1:1");
        (AnalyzerAppState::new(search, facts), tmp)
    }

    async fn json_get(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    #[tokio::test]
    async fn health_degraded_when_search_unreachable() {
        // The stub search client points at port 1 (nothing listening).
        // Expect: 503 SERVICE_UNAVAILABLE, status == "degraded",
        // search_reachable == false.
        let (state, _tmp) = make_state();
        let app = build_router(state);
        let (status, body) = json_get(app, "/health").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "degraded");
        assert_eq!(body["search_reachable"], false);
    }

    #[tokio::test]
    async fn health_response_includes_version() {
        let (state, _tmp) = make_state();
        let app = build_router(state);
        let (_status, body) = json_get(app, "/health").await;
        // Version is always present regardless of search reachability.
        assert!(body["version"].is_string());
        assert!(!body["version"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upsert_then_list_facts_round_trip() {
        let (state, _tmp) = make_state();
        let app = build_router(state);

        let body = serde_json::json!({
            "subject": "fn search",
            "predicate": "implements",
            "object": "trait Searcher",
            "index_id": "test"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/facts")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (status, listing) = json_get(app, "/facts").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listing["count"], 1);
    }

    #[tokio::test]
    async fn list_indexes_proxies_failure_to_502() {
        // Search daemon at port 1 won't answer — proxy should surface 502.
        let (state, _tmp) = make_state();
        let app = build_router(state);
        let (status, _) = json_get(app, "/indexes").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }
}
