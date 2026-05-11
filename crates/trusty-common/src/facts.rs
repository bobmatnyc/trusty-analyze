//! Canonical fact triples shared across the search/analyzer boundary.
//!
//! Wire-compatible with `trusty_search_core::facts::FactRecord`. The
//! analyzer owns the FactStore now, but the record shape stays the same so
//! existing data files migrate without translation.

use serde::{Deserialize, Serialize};

/// One canonical fact about an indexed corpus.
///
/// # Caveats
///
/// **JavaScript precision loss.** `id` is a `u64`. Values above 2^53 (the
/// largest integer JavaScript's `Number` can represent exactly) will silently
/// lose precision when parsed by browser clients via `JSON.parse`. If the
/// HTTP API is ever consumed by browser/JS clients, serialize `id` as a
/// `String` (e.g. via `#[serde(with = "...")]`) to preserve precision.
///
/// **Hash stability across toolchains.** trusty-analyzer-core currently
/// computes `id` with `std::collections::hash_map::DefaultHasher`, whose
/// algorithm is **not stable across Rust toolchain versions** (see the
/// `DefaultHasher` docs). Persisted redb entries written by one toolchain
/// may be unreadable — or worse, silently mismatched — after upgrading
/// the compiler. For production persistence, switch to a stable, explicit
/// hash algorithm (FxHash, AHash with a fixed seed, or xxhash) so ids
/// remain stable across rebuilds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactRecord {
    /// Stable hash of `(subject, predicate, object)`.
    ///
    /// See the type-level docs for two important caveats: precision loss
    /// when consumed by JavaScript clients (values > 2^53), and hash
    /// instability across Rust toolchain versions when `DefaultHasher`
    /// is used to compute the value.
    pub id: u64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// Confidence score in [0.0, 1.0]. Latest value wins on upsert.
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// Chunk IDs supporting this fact. Set-merged on upsert.
    #[serde(default)]
    pub provenance: Vec<String>,
    /// Index this fact came from.
    pub index_id: String,
    /// Unix timestamp (seconds) at first creation.
    #[serde(default)]
    pub created_at: u64,
}

fn default_confidence() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_record_round_trips() {
        let f = FactRecord {
            id: 42,
            subject: "fn search".into(),
            predicate: "implements".into(),
            object: "trait Searcher".into(),
            confidence: 0.9,
            provenance: vec!["c1".into()],
            index_id: "i".into(),
            created_at: 1_700_000_000,
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: FactRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }
}
