//! Canonical fact triples shared across the search/analyzer boundary.
//!
//! Wire-compatible with `trusty_search_core::facts::FactRecord`. The
//! analyzer owns the FactStore now, but the record shape stays the same so
//! existing data files migrate without translation.

use serde::{Deserialize, Serialize};

/// One canonical fact about an indexed corpus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactRecord {
    /// Stable hash of `(subject, predicate, object)`.
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
