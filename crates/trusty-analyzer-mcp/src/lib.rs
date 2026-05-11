//! MCP (Model Context Protocol) server for trusty-analyzer.
//!
//! Why: full parity with the HTTP surface so an MCP client gets the same
//! capabilities as a curl user. The dispatcher is a pure translator — JSON-RPC
//! in, HTTP out — and owns no state beyond a `reqwest::Client` and the
//! analyzer daemon's base URL.
//!
//! Tools (mirrors `trusty-analyzer-service`):
//!
//! | MCP tool              | Daemon endpoint                              |
//! |-----------------------|----------------------------------------------|
//! | `complexity_hotspots` | `GET /indexes/:id/complexity_hotspots`       |
//! | `find_smells`         | `GET /indexes/:id/smells`                    |
//! | `analyze_quality`     | `GET /indexes/:id/quality`                   |
//! | `list_facts`          | `GET /facts`                                 |
//! | `upsert_fact`         | `POST /facts`                                |
//! | `delete_fact`         | `DELETE /facts/:id`                          |
//! | `cluster_concepts`    | `GET /indexes/:id/clusters`                  |
//! | `ingest_scip`         | `POST /indexes/:id/scip`                     |
//! | `analyzer_health`     | `GET /health`                                |

pub mod stdio;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod error_codes {
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    #[serde(skip)]
    pub suppress: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
            suppress: false,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
            suppress: false,
        }
    }

    pub fn suppressed() -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Value::Null,
            result: None,
            error: None,
            suppress: true,
        }
    }
}

/// MCP dispatcher backed by an HTTP client targeting the analyzer daemon.
#[derive(Clone)]
pub struct AnalyzerMcpServer {
    base_url: String,
    http: reqwest::Client,
}

impl AnalyzerMcpServer {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Translate one JSON-RPC request into a daemon HTTP call. Always returns
    /// a `Response`; transport / daemon failures are reported in-band.
    pub async fn dispatch(&self, req: Request) -> Response {
        let is_notification = req.id.is_none();
        let id = req.id.clone().unwrap_or(Value::Null);

        if req.jsonrpc != "2.0" {
            if is_notification {
                return Response::suppressed();
            }
            return Response::err(id, error_codes::INVALID_REQUEST, "jsonrpc must be \"2.0\"");
        }

        match req.method.as_str() {
            "initialize" => {
                return Response::ok(
                    id,
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "trusty-analyzer",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                );
            }
            "notifications/initialized" | "initialized" => {
                return Response::suppressed();
            }
            _ => {}
        }

        let (tool, arguments, via_tools_call) = match req.method.as_str() {
            "tools/call" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let args = req
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                match name {
                    Some(n) => (n, args, true),
                    None => {
                        return Response::err(
                            id,
                            error_codes::INVALID_PARAMS,
                            "tools/call requires a 'name' field",
                        )
                    }
                }
            }
            "tools/list" => {
                return Response::ok(id, serde_json::json!({ "tools": tool_descriptors() }));
            }
            other => (other.to_string(), req.params.clone(), false),
        };

        let outcome = self.call_tool(&tool, &arguments).await;

        if via_tools_call {
            match outcome {
                Ok(value) => Response::ok(id, wrap_tool_result(&value)),
                Err(DispatchError::UnknownTool) => Response::err(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown tool: {tool}"),
                ),
                Err(DispatchError::InvalidParams(msg)) => Response::ok(id, wrap_tool_error(&msg)),
                Err(DispatchError::Transport(msg)) => Response::ok(id, wrap_tool_error(&msg)),
            }
        } else {
            match outcome {
                Ok(value) => Response::ok(id, wrap_text_content(&value)),
                Err(DispatchError::UnknownTool) => Response::err(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown tool: {tool}"),
                ),
                Err(DispatchError::InvalidParams(msg)) => {
                    Response::err(id, error_codes::INVALID_PARAMS, msg)
                }
                Err(DispatchError::Transport(msg)) => {
                    Response::err(id, error_codes::INTERNAL_ERROR, msg)
                }
            }
        }
    }

    /// Top-level tool dispatch. Each tool delegates to a `handle_<tool>`
    /// function that owns parameter parsing and HTTP call construction.
    ///
    /// Why: A 130-line match block hid the per-tool logic. Per-handler
    /// functions cap dispatch cyclo at the number of tools and let each
    /// handler be tested without going through the JSON-RPC envelope.
    /// What: Looks up the tool name and forwards `(args, self)` to the
    /// handler.
    /// Test: `unknown_tool_returns_method_not_found` covers the fall-through
    /// arm; `handle_analyzer_health_calls_health_endpoint` exercises one
    /// handler directly.
    async fn call_tool(&self, tool: &str, args: &Value) -> Result<Value, DispatchError> {
        match tool {
            "complexity_hotspots" => self.handle_complexity_hotspots(args).await,
            "find_smells" => self.handle_find_smells(args).await,
            "analyze_quality" => self.handle_analyze_quality(args).await,
            "list_facts" => self.handle_list_facts(args).await,
            "upsert_fact" => self.handle_upsert_fact(args).await,
            "delete_fact" => self.handle_delete_fact(args).await,
            "extract_graph" => self.handle_extract_graph(args).await,
            "list_entities" => self.handle_list_entities(args).await,
            "cluster_concepts" => self.handle_cluster_concepts(args).await,
            "analyzer_health" => self.handle_analyzer_health(args).await,
            "ingest_scip" => self.handle_ingest_scip(args).await,
            _ => Err(DispatchError::UnknownTool),
        }
    }

    async fn handle_complexity_hotspots(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        let top_n = args.get("top_n").and_then(Value::as_u64).unwrap_or(20);
        self.get(&format!(
            "/indexes/{index_id}/complexity_hotspots?top_n={top_n}"
        ))
        .await
    }

    async fn handle_find_smells(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        self.get(&format!("/indexes/{index_id}/smells")).await
    }

    async fn handle_analyze_quality(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        self.get(&format!("/indexes/{index_id}/quality")).await
    }

    async fn handle_list_facts(&self, args: &Value) -> Result<Value, DispatchError> {
        let q = build_query(args, &["subject", "predicate", "object"]);
        self.get(&format!("/facts{q}")).await
    }

    async fn handle_upsert_fact(&self, args: &Value) -> Result<Value, DispatchError> {
        let subject = require_str(args, "subject")?;
        let predicate = require_str(args, "predicate")?;
        let object = require_str(args, "object")?;
        let index_id = require_str(args, "index_id")?;
        let confidence = args
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let provenance = args
            .get("provenance")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));
        let body = serde_json::json!({
            "subject": subject,
            "predicate": predicate,
            "object": object,
            "index_id": index_id,
            "confidence": confidence,
            "provenance": provenance,
        });
        self.post("/facts", &body).await
    }

    async fn handle_delete_fact(&self, args: &Value) -> Result<Value, DispatchError> {
        let id = args
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| DispatchError::InvalidParams("missing 'id' (u64)".into()))?;
        self.delete(&format!("/facts/{id}")).await
    }

    async fn handle_extract_graph(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        let mut path = format!("/indexes/{index_id}/graph");
        if let Some(lang) = args.get("language").and_then(Value::as_str) {
            path.push_str(&format!("?language={}", urlencode(lang)));
        }
        self.get(&path).await
    }

    async fn handle_list_entities(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        let q = build_query(args, &["kind", "language"]);
        self.get(&format!("/indexes/{index_id}/entities{q}")).await
    }

    async fn handle_cluster_concepts(&self, args: &Value) -> Result<Value, DispatchError> {
        let index_id = index_id_or_default(args);
        let k = args.get("k").and_then(Value::as_u64).unwrap_or(8);
        let path = match args.get("method").and_then(Value::as_str) {
            Some(m) => format!("/indexes/{index_id}/clusters?k={k}&method={m}"),
            None => format!("/indexes/{index_id}/clusters?k={k}"),
        };
        self.get(&path).await
    }

    async fn handle_analyzer_health(&self, _args: &Value) -> Result<Value, DispatchError> {
        self.get("/health").await
    }

    async fn handle_ingest_scip(&self, args: &Value) -> Result<Value, DispatchError> {
        use base64::Engine;
        let index_id = index_id_or_default(args);
        let b64 = require_str(args, "scip_base64")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| {
                DispatchError::InvalidParams(format!("scip_base64 is not valid base64: {e}"))
            })?;
        self.post_bytes(&format!("/indexes/{index_id}/scip"), bytes)
            .await
    }

    async fn post_bytes(&self, path: &str, body: Vec<u8>) -> Result<Value, DispatchError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| DispatchError::Transport(format!("POST {url}: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| DispatchError::Transport(format!("decode {url}: {e}")))?;
        if !status.is_success() {
            return Err(DispatchError::Transport(format!(
                "POST {url} returned {status}: {body}"
            )));
        }
        Ok(body)
    }

    async fn get(&self, path: &str) -> Result<Value, DispatchError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| DispatchError::Transport(format!("GET {url}: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| DispatchError::Transport(format!("decode {url}: {e}")))?;
        if !status.is_success() {
            return Err(DispatchError::Transport(format!(
                "GET {url} returned {status}: {body}"
            )));
        }
        Ok(body)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, DispatchError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| DispatchError::Transport(format!("POST {url}: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| DispatchError::Transport(format!("decode {url}: {e}")))?;
        if !status.is_success() {
            return Err(DispatchError::Transport(format!(
                "POST {url} returned {status}: {body}"
            )));
        }
        Ok(body)
    }

    async fn delete(&self, path: &str) -> Result<Value, DispatchError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| DispatchError::Transport(format!("DELETE {url}: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| DispatchError::Transport(format!("decode {url}: {e}")))?;
        if !status.is_success() {
            return Err(DispatchError::Transport(format!(
                "DELETE {url} returned {status}: {body}"
            )));
        }
        Ok(body)
    }
}

#[derive(Debug)]
enum DispatchError {
    UnknownTool,
    InvalidParams(String),
    Transport(String),
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, DispatchError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::InvalidParams(format!("missing or non-string '{key}'")))
}

/// Read `index` (preferred) or `index_id` (legacy alias) from `args`,
/// falling back to `"default"`.
///
/// Why: Multiple tools accept either parameter name and need the same
/// fallback behaviour; centralising removes 9 copies of the same chain.
/// What: Tries `index`, then `index_id`, then `"default"`.
/// Test: Covered indirectly by every per-tool handler test.
fn index_id_or_default(args: &Value) -> &str {
    args.get("index")
        .or_else(|| args.get("index_id"))
        .and_then(Value::as_str)
        .unwrap_or("default")
}

/// Build a `?key=val&...` query string from whichever of `keys` is present
/// in `args` (skipping missing or non-string values). Returns an empty
/// string if no keys were found.
fn build_query(args: &Value, keys: &[&str]) -> String {
    let mut q = String::new();
    for key in keys {
        if let Some(v) = args.get(*key).and_then(Value::as_str) {
            let sep = if q.is_empty() { '?' } else { '&' };
            q.push(sep);
            q.push_str(key);
            q.push('=');
            q.push_str(&urlencode(v));
        }
    }
    q
}

/// Minimal URL encoding for the bits we pass through to `/facts?subject=...`.
/// Avoids pulling a full url crate into the MCP server.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn wrap_text_content(value: &Value) -> Value {
    serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        }]
    })
}

fn wrap_tool_result(value: &Value) -> Value {
    serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        }],
        "isError": false,
    })
}

fn wrap_tool_error(msg: &str) -> Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": format!("Error: {msg}") }],
        "isError": true,
    })
}

pub fn tool_descriptors() -> Value {
    serde_json::json!([
        {
            "name": "complexity_hotspots",
            "description": "Top-N chunks ranked by cyclomatic complexity",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": { "type": "string" },
                    "index_id": { "type": "string" },
                    "top_n": { "type": "number" }
                }
            }
        },
        {
            "name": "find_smells",
            "description": "Chunks with at least one detected code smell",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": { "type": "string" },
                    "index_id": { "type": "string" }
                }
            }
        },
        {
            "name": "analyze_quality",
            "description": "Aggregate quality stats: avg cyclomatic, %A, smell count",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": { "type": "string" },
                    "index_id": { "type": "string" }
                }
            }
        },
        {
            "name": "list_facts",
            "description": "List canonical facts, optionally filtered by subject/predicate/object",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject":   { "type": "string" },
                    "predicate": { "type": "string" },
                    "object":    { "type": "string" }
                }
            }
        },
        {
            "name": "upsert_fact",
            "description": "Insert or update a canonical fact triple",
            "inputSchema": {
                "type": "object",
                "required": ["subject", "predicate", "object", "index_id"],
                "properties": {
                    "subject":    { "type": "string" },
                    "predicate":  { "type": "string" },
                    "object":     { "type": "string" },
                    "index_id":   { "type": "string" },
                    "confidence": { "type": "number" },
                    "provenance": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "delete_fact",
            "description": "Delete a fact by its u64 id",
            "inputSchema": {
                "type": "object",
                "required": ["id"],
                "properties": { "id": { "type": "number" } }
            }
        },
        {
            "name": "analyzer_health",
            "description": "Probe analyzer daemon liveness and version",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "extract_graph",
            "description": "Build the multi-language knowledge graph (nodes + edges) for an index",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index":    { "type": "string" },
                    "index_id": { "type": "string" },
                    "language": { "type": "string" }
                }
            }
        },
        {
            "name": "cluster_concepts",
            "description": "Group chunks into concept clusters using k-means over embeddings (BOW or neural)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index":    { "type": "string" },
                    "index_id": { "type": "string" },
                    "k":        { "type": "number" },
                    "method":   { "type": "string", "description": "Embedding method: 'bow' (default, fast) or 'neural' (semantic, requires fastembed model)" }
                }
            }
        },
        {
            "name": "ingest_scip",
            "description": "Ingest a SCIP (Scalable and Precise Index for Code) protobuf index for a given index_id, enriching the knowledge graph with fully-resolved symbols and cross-file relationships. The SCIP bytes must be base64-encoded.",
            "inputSchema": {
                "type": "object",
                "required": ["scip_base64"],
                "properties": {
                    "index":        { "type": "string" },
                    "index_id":     { "type": "string" },
                    "scip_base64":  { "type": "string", "description": "Base64-encoded SCIP Index protobuf payload" }
                }
            }
        },
        {
            "name": "list_entities",
            "description": "List symbol-level entities (functions, classes, ...) for an index",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index":    { "type": "string" },
                    "index_id": { "type": "string" },
                    "kind":     { "type": "string" },
                    "language": { "type": "string" }
                }
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: "2.0".into(),
            id: Some(Value::from(1u64)),
            method: method.into(),
            params,
        }
    }

    #[tokio::test]
    async fn tools_list_contains_full_surface() {
        let server = AnalyzerMcpServer::new("http://127.0.0.1:1");
        let resp = server.dispatch(req("tools/list", Value::Null)).await;
        let result = resp.result.expect("expected result");
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .expect("array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        for required in [
            "complexity_hotspots",
            "find_smells",
            "analyze_quality",
            "list_facts",
            "upsert_fact",
            "delete_fact",
            "analyzer_health",
            "ingest_scip",
        ] {
            assert!(
                names.contains(&required),
                "missing tool '{required}' (got {names:?})"
            );
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_method_not_found() {
        let server = AnalyzerMcpServer::new("http://127.0.0.1:1");
        let resp = server
            .dispatch(req(
                "tools/call",
                serde_json::json!({ "name": "no_such_tool", "arguments": {} }),
            ))
            .await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_analyzer_health_calls_health_endpoint() {
        // Direct handler invocation, bypassing dispatch. Daemon is unreachable,
        // so we expect a Transport error referencing /health, which proves the
        // handler constructed the right URL without us going through tools/call.
        let server = AnalyzerMcpServer::new("http://127.0.0.1:1");
        let err = server
            .handle_analyzer_health(&Value::Null)
            .await
            .expect_err("daemon unreachable, expected transport error");
        match err {
            DispatchError::Transport(msg) => {
                assert!(
                    msg.contains("/health"),
                    "expected transport error to mention /health, got: {msg}"
                );
            }
            other => panic!("expected DispatchError::Transport, got {other:?}"),
        }
    }

    #[test]
    fn index_id_or_default_prefers_index_then_alias_then_default() {
        let with_index = serde_json::json!({ "index": "primary" });
        assert_eq!(index_id_or_default(&with_index), "primary");

        let with_alias = serde_json::json!({ "index_id": "alias" });
        assert_eq!(index_id_or_default(&with_alias), "alias");

        let empty = serde_json::json!({});
        assert_eq!(index_id_or_default(&empty), "default");
    }

    #[test]
    fn build_query_skips_missing_keys() {
        let args = serde_json::json!({ "subject": "fn auth", "object": "JWT" });
        let q = build_query(&args, &["subject", "predicate", "object"]);
        // urlencoded space → %20
        assert!(q.starts_with('?'), "expected leading '?', got {q}");
        assert!(q.contains("subject=fn%20auth"), "got {q}");
        assert!(q.contains("object=JWT"), "got {q}");
        assert!(!q.contains("predicate"), "got {q}");
    }

    #[tokio::test]
    async fn rejects_wrong_jsonrpc_version() {
        let server = AnalyzerMcpServer::new("http://127.0.0.1:1");
        let r = Request {
            jsonrpc: "1.0".into(),
            id: Some(Value::from(7u64)),
            method: "tools/list".into(),
            params: Value::Null,
        };
        let resp = server.dispatch(r).await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, error_codes::INVALID_REQUEST);
    }
}
