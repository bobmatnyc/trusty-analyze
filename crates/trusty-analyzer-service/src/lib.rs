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
//! - `GET  /indexes/{id}/complexity_hotspots`   top-N by cyclomatic
//! - `GET  /indexes/{id}/smells`                chunks with at least one smell
//! - `GET  /indexes/{id}/quality`               aggregate report
//! - `GET  /facts`                              list / filter facts
//! - `POST /facts`                              upsert a fact
//! - `DELETE /facts/{id}`                       delete a fact
//!
//! Test: `cargo test -p trusty-analyzer-service` boots the router with a stub
//! search client and exercises every route end-to-end.

mod ui;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use trusty_analyzer_core::{
    bow_embedding, cluster as run_cluster, extract_doc_comments, extract_kg_from_scip,
    facts::new_fact, quality, AnalyzerRegistry, ClusterResult, FactStore, IndexSummary,
    NerExtractor, ScipIngestSummary, TrustySearchClient,
};
use trusty_analyzer_embedder::{BowEmbedder, Embedder, EmbedderKind};
use trusty_analyzer_types::{KgGraph, KgNode, RawEntity};

/// Default port the analyzer daemon binds to. Picked to sit next to
/// trusty-search's 7878.
pub const DEFAULT_PORT: u16 = 7879;

/// Shared state for every handler. Cheap to clone (everything is `Arc`-ish).
#[derive(Clone)]
pub struct AnalyzerAppState {
    pub search: TrustySearchClient,
    pub facts: FactStore,
    pub registry: Arc<AnalyzerRegistry>,
    /// Neural / BOW embedder used by `/indexes/{id}/clusters` when the request
    /// asks for `method=neural`. Falls back to a fresh `BowEmbedder` when the
    /// request asks for `method=bow` (the default).
    pub embedder: Arc<dyn Embedder>,
    /// Per-index SCIP-derived knowledge graph overlay, populated by
    /// `POST /indexes/{id}/scip`. Merged into the response of
    /// `GET /indexes/{id}/graph` so consumers see the union of tree-sitter
    /// extraction and any precise SCIP indexes the user has uploaded.
    pub scip_overlays: Arc<RwLock<HashMap<String, KgGraph>>>,
}

impl AnalyzerAppState {
    /// Construct with the default registry and a BOW embedder. Use this when
    /// neural embeddings aren't required (tests, BOW-only deployments).
    pub fn new(search: TrustySearchClient, facts: FactStore) -> Self {
        Self {
            search,
            facts,
            registry: Arc::new(AnalyzerRegistry::default_registry()),
            embedder: Arc::new(BowEmbedder::default()),
            scip_overlays: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Construct with an explicit registry (useful for tests and plug-ins).
    /// Embedder defaults to BOW.
    pub fn with_registry(
        search: TrustySearchClient,
        facts: FactStore,
        registry: Arc<AnalyzerRegistry>,
    ) -> Self {
        Self {
            search,
            facts,
            registry,
            embedder: Arc::new(BowEmbedder::default()),
            scip_overlays: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replace the embedder on an existing state. Useful when the binary
    /// builds state first and then tries to load fastembed, falling back
    /// silently when the model isn't available.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = embedder;
        self
    }
}

/// Build the axum router around `state`.
///
/// Why: Composes the analyzer's HTTP surface in one place so callers (binary,
/// tests, embedded use) all get the same routes and middleware stack. The
/// shared `trusty_common::server::with_standard_middleware` layer keeps CORS,
/// tracing, and gzip behavior consistent across every trusty-* daemon.
/// What: Wires every route handler to its path (axum 0.8 `{name}` capture
/// syntax), binds the shared state, then applies the standard middleware
/// stack.
/// Test: `cargo test -p trusty-analyzer-service` drives every route through
/// the returned router; the middleware composition is smoke-tested
/// transitively (any layering regression breaks the suite).
pub fn build_router(state: AnalyzerAppState) -> Router {
    let router = Router::new()
        .route("/health", get(health))
        .route("/indexes", get(list_indexes))
        .route(
            "/indexes/{id}/complexity_hotspots",
            get(complexity_hotspots),
        )
        .route("/indexes/{id}/smells", get(smells))
        .route("/indexes/{id}/quality", get(quality_report))
        .route("/indexes/{id}/graph", get(graph_for_index))
        .route("/indexes/{id}/entities", get(entities_for_index))
        .route("/indexes/{id}/clusters", get(clusters_for_index))
        .route("/indexes/{id}/ner", get(ner_for_index))
        .route("/indexes/{id}/scip", post(ingest_scip))
        .route("/facts", get(list_facts).post(upsert_fact))
        .route("/facts/{id}", delete(delete_fact))
        .route("/ui", get(ui::ui_index_handler))
        .route("/ui/", get(ui::ui_index_handler))
        .route("/ui/{*path}", get(ui::ui_asset_handler))
        .with_state(Arc::new(state));
    trusty_common::server::with_standard_middleware(router)
}

/// Bind to `start_port` (or auto-pick a free port walking forward) and run
/// the daemon until the future returns. The actually-bound address is also
/// written to the shared trusty-* daemon address file so other tools can
/// discover the live port without re-implementing the search.
///
/// Why: port auto-detection and daemon-addr handshake are duplicated across
/// every trusty-* daemon. Using the shared `trusty_common` helpers keeps
/// behavior consistent (warn logging, fixed walk window, addr file shape).
/// What: walks up to 64 ports forward from `start_port`, logs the live URL,
/// then `axum::serve`s the router.
/// Test: integration tests bind their own listener — exercised by
/// `cargo test -p trusty-analyzer-service`.
pub async fn serve(state: AnalyzerAppState, start_port: u16) -> Result<()> {
    let start_addr: SocketAddr = ([127, 0, 0, 1], start_port).into();
    let listener = trusty_common::bind_with_auto_port(start_addr, 64).await?;
    let actual = listener.local_addr()?;
    trusty_common::write_daemon_addr("trusty-analyzer", &actual.to_string())?;
    tracing::info!("trusty-analyzer listening on http://{actual}");
    let app = build_router(state);
    axum::serve(listener, app).await?;
    Ok(())
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
    // Merge any SCIP-derived overlay that the user has uploaded for this
    // index. SCIP supplies fully-resolved cross-file symbols which the
    // tree-sitter adapters cannot derive on their own, so the union is
    // strictly more useful than either alone.
    if let Some(overlay) = state.scip_overlays.read().await.get(&id).cloned() {
        graph.merge(overlay);
        graph = trusty_analyzer_core::link(graph);
    }
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

#[derive(Deserialize)]
pub struct ClusterQueryParams {
    /// Number of clusters to compute. Defaults to 8, clamped to [1, 50].
    pub k: Option<usize>,
    /// Embedding method: `"bow"` (default, deterministic 256-dim) or
    /// `"neural"` (fastembed all-MiniLM-L6-v2, 384-dim).
    #[serde(default)]
    pub method: Option<EmbedderKind>,
}

#[derive(Serialize)]
pub struct ClusterResponseItem {
    pub id: usize,
    pub label: String,
    pub members: Vec<String>,
    pub cohesion: f32,
    pub size: usize,
}

#[derive(Serialize)]
pub struct ClusterResponse {
    pub k: usize,
    /// Which embedder produced the vectors (`"bow"` or `"neural"`).
    pub method: String,
    /// Dimension of the embedding vectors used.
    pub dim: usize,
    pub iterations: usize,
    pub chunk_count: usize,
    pub clusters: Vec<ClusterResponseItem>,
}

fn cluster_items_from(r: ClusterResult) -> Vec<ClusterResponseItem> {
    r.clusters
        .into_iter()
        .map(|c| ClusterResponseItem {
            id: c.id,
            label: c.label,
            size: c.members.len(),
            members: c.members,
            cohesion: c.cohesion,
        })
        .collect()
}

/// Why: surfaces "what themes does this codebase contain?" without needing a
/// full knowledge graph or neural embedder. Useful for codebase exploration
/// and high-level summaries.
/// What: fetches chunks for `index`, derives a 256-dim bag-of-words vector
/// per chunk, runs seeded k-means, and returns the cluster assignments.
/// Test: covered indirectly by trusty-analyzer-core's `concept_cluster` tests;
/// the route wiring is exercised by `clusters_route_returns_502_when_search_down`.
async fn clusters_for_index(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    Query(params): Query<ClusterQueryParams>,
) -> Result<Json<ClusterResponse>, StatusCode> {
    const BOW_DIM: usize = 256;
    let k = params.k.unwrap_or(8).clamp(1, 50);
    let method = params.method.clone().unwrap_or_default();
    let chunks = fetch_chunks(&state, &id).await?;
    if chunks.is_empty() {
        return Ok(Json(ClusterResponse {
            k,
            method: method.as_str().to_string(),
            dim: 0,
            iterations: 0,
            chunk_count: 0,
            clusters: Vec::new(),
        }));
    }

    // Resolve embedder. For neural, defer to the shared state embedder (which
    // may itself be BOW if fastembed failed to load at startup). For BOW,
    // construct a fresh stateless BowEmbedder so we never go through fastembed
    // when the user explicitly asked for BOW.
    let neural_embedder: Arc<dyn Embedder> = state.embedder.clone();
    let bow_embedder = BowEmbedder::with_dim(BOW_DIM);
    let (chosen, effective_kind): (&dyn Embedder, EmbedderKind) = match method {
        EmbedderKind::Neural => (neural_embedder.as_ref(), neural_embedder.kind()),
        EmbedderKind::Bow => (&bow_embedder, EmbedderKind::Bow),
    };

    let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let (vecs, effective_kind, dim) = match chosen.embed_batch(&texts) {
        Ok(v) => (v, effective_kind, chosen.dim()),
        Err(e) => {
            tracing::warn!(
                "embedder ({:?}) failed ({e:#}); falling back to BOW",
                effective_kind
            );
            let fallback: Vec<Vec<f32>> = texts.iter().map(|t| bow_embedding(t, BOW_DIM)).collect();
            (fallback, EmbedderKind::Bow, BOW_DIM)
        }
    };

    let embeddings: Vec<(String, Vec<f32>)> = chunks
        .iter()
        .zip(vecs)
        .map(|(c, v)| (c.id.clone(), v))
        .collect();
    let result = run_cluster(&embeddings, k, 100, 42);
    let iterations = result.iterations;
    Ok(Json(ClusterResponse {
        k,
        method: effective_kind.as_str().to_string(),
        dim,
        iterations,
        chunk_count: chunks.len(),
        clusters: cluster_items_from(result),
    }))
}

#[derive(Deserialize)]
pub struct NerQueryParams {
    /// Cap on the number of entities returned (after extraction).
    pub top_k: Option<usize>,
}

/// Why: surfaces named-entity candidates pulled from doc comments so callers
/// (Claude Code, UI dashboards) can browse natural-language concepts side by
/// side with structural symbols. The route is always available; the actual
/// ONNX NER model is feature-gated and opportunistically loaded at startup.
/// What: fetches chunks for `id`, runs `extract_doc_comments` on each chunk's
/// content, runs the NER extractor (no-op when the `ner` feature is disabled
/// or the model file is missing), and returns the entities truncated to
/// `top_k` (default 50).
/// Test: with a stub search client returning no chunks the handler returns an
/// empty array and HTTP 200; the NER feature flag is exercised by the core
/// crate's `ner` module tests.
async fn ner_for_index(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    Query(params): Query<NerQueryParams>,
) -> Result<Json<Vec<RawEntity>>, StatusCode> {
    let chunks = fetch_chunks(&state, &id).await?;
    let top_k = params.top_k.unwrap_or(50);
    let extractor = NerExtractor::try_load();

    let mut entities: Vec<RawEntity> = Vec::new();
    for chunk in &chunks {
        let docs = extract_doc_comments(&chunk.content);
        if docs.is_empty() {
            continue;
        }
        entities.extend(extractor.extract(&docs, &chunk.file));
        if entities.len() >= top_k {
            break;
        }
    }
    entities.truncate(top_k);
    Ok(Json(entities))
}

#[derive(Serialize)]
pub struct ScipIngestResponse {
    pub index_id: String,
    #[serde(flatten)]
    pub summary: ScipIngestSummary,
}

/// Why: SCIP indexes carry fully-resolved cross-file symbols that the
/// tree-sitter adapters can't derive (call resolution, trait implementations
/// across files, generics). Ingesting them is how the analyzer goes from
/// "approximate" to "precise" for languages with a real SCIP indexer.
/// What: accepts a SCIP `Index` protobuf as raw bytes, converts it to a
/// `KgGraph`, stores it as a per-index overlay, and returns ingest stats.
/// The overlay is merged into `/indexes/{id}/graph` responses.
/// Test: `scip_ingest_round_trip` POSTs a hand-built SCIP index and verifies
/// the resulting graph appears in the `/graph` response.
async fn ingest_scip(
    State(state): State<Arc<AnalyzerAppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<ScipIngestResponse>, (StatusCode, Json<serde_json::Value>)> {
    let (graph, summary) = extract_kg_from_scip(&body).map_err(|e| {
        tracing::warn!("SCIP ingest for {id} failed: {e:#}");
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid SCIP protobuf: {e:#}") })),
        )
    })?;
    state.scip_overlays.write().await.insert(id.clone(), graph);
    Ok(Json(ScipIngestResponse {
        index_id: id,
        summary,
    }))
}

async fn fetch_chunks(
    state: &AnalyzerAppState,
    id: &str,
) -> Result<Vec<trusty_analyzer_types::CodeChunk>, StatusCode> {
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
pub use trusty_analyzer_types::FactRecord as PublicFactRecord;

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
    async fn scip_ingest_accepts_valid_index_and_stores_overlay() {
        use protobuf::{EnumOrUnknown, Message};
        use scip::types::{
            symbol_information::Kind as ScipKind, Document, Index, Occurrence, SymbolInformation,
        };

        let (state, _tmp) = make_state();
        let overlays = state.scip_overlays.clone();
        let app = build_router(state);

        // Build a one-symbol SCIP index.
        let mut sym = SymbolInformation::new();
        sym.symbol = "rust . . hello().".into();
        sym.kind = EnumOrUnknown::new(ScipKind::Function);
        sym.display_name = "hello".into();
        let mut occ = Occurrence::new();
        occ.symbol = sym.symbol.clone();
        occ.symbol_roles = 0x1;
        occ.range = vec![1, 0, 5];
        let mut doc = Document::new();
        doc.relative_path = "src/lib.rs".into();
        doc.language = "rust".into();
        doc.symbols.push(sym);
        doc.occurrences.push(occ);
        let mut index = Index::new();
        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode scip index");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/indexes/myidx/scip")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["index_id"], "myidx");
        assert_eq!(parsed["documents"], 1);
        assert_eq!(parsed["kg_nodes"], 1);

        // The overlay should be persisted in state.
        let overlays = overlays.read().await;
        let g = overlays.get("myidx").expect("overlay stored");
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.nodes[0].name, "hello");
    }

    #[tokio::test]
    async fn scip_ingest_rejects_garbage_bytes() {
        let (state, _tmp) = make_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/indexes/x/scip")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(vec![0xFF, 0xFF, 0xFF, 0xFF]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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
