//! Thin async HTTP client to the trusty-search daemon.
//!
//! Why: the analyzer is a sidecar — it never reads trusty-search's redb files
//! directly. Instead it pulls chunks over HTTP and runs analysis in-process.
//! Keeping the client tiny (one struct, three GETs) makes failure modes
//! obvious and lets us swap to a different transport later if needed.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use trusty_common::CodeChunk;

/// Summary of one registered index, as returned by `GET /indexes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSummary {
    pub id: String,
}

/// Reqwest-backed client. Cheap to clone — internally a `reqwest::Client`
/// (which is already an `Arc` under the hood).
#[derive(Clone)]
pub struct TrustySearchClient {
    base_url: String,
    http: reqwest::Client,
}

impl TrustySearchClient {
    /// Construct a client pointed at `base_url` (e.g. `http://127.0.0.1:7878`).
    /// Trailing slashes are tolerated.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base = base_url.into();
        if base.ends_with('/') {
            base.pop();
        }
        Self {
            base_url: base,
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /health` — true if the daemon answers 2xx.
    pub async fn health(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        let resp = self.http.get(&url).send().await.context("GET /health")?;
        Ok(resp.status().is_success())
    }

    /// `GET /indexes` — list every registered index id.
    pub async fn list_indexes(&self) -> Result<Vec<IndexSummary>> {
        #[derive(Deserialize)]
        struct Listing {
            indexes: Vec<String>,
        }
        let url = format!("{}/indexes", self.base_url);
        let body: Listing = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("non-2xx from {url}"))?
            .json()
            .await
            .with_context(|| format!("decode {url}"))?;
        Ok(body
            .indexes
            .into_iter()
            .map(|id| IndexSummary { id })
            .collect())
    }

    /// `GET /indexes/:id/chunks` — bulk export of every chunk for `index_id`.
    /// Trusty-search must expose this endpoint (added as part of issue #40).
    pub async fn get_chunks(&self, index_id: &str) -> Result<Vec<CodeChunk>> {
        #[derive(Deserialize)]
        struct ChunksBody {
            chunks: Vec<CodeChunk>,
        }
        let url = format!("{}/indexes/{}/chunks", self.base_url, index_id);
        let body: ChunksBody = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("non-2xx from {url}"))?
            .json()
            .await
            .with_context(|| format!("decode {url}"))?;
        Ok(body.chunks)
    }
}
