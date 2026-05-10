//! Complexity metrics passed between trusty-search and trusty-analyzer.
//!
//! Wire-compatible with `trusty_search_core::complexity::ComplexityMetrics`.
//! The variant names on `ComplexityGrade` and `CodeSmell` match exactly so
//! serde round-trips JSON produced by the search daemon without translation.

use serde::{Deserialize, Serialize};

/// Bundle of per-chunk complexity numbers and detected smells.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ComplexityMetrics {
    #[serde(default)]
    pub cyclomatic: u32,
    #[serde(default)]
    pub cognitive: u32,
    #[serde(default)]
    pub grade: ComplexityGrade,
    #[serde(default)]
    pub smells: Vec<CodeSmell>,
}

/// Letter grade derived from cyclomatic complexity. A is best, F is worst.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComplexityGrade {
    #[default]
    A,
    B,
    C,
    D,
    F,
}

impl ComplexityGrade {
    /// Map a cyclomatic complexity number to a letter grade. Bands match
    /// trusty-search exactly so the same chunk receives the same grade in
    /// both projects.
    pub fn from_cyclomatic(v: u32) -> Self {
        match v {
            0..=5 => Self::A,
            6..=10 => Self::B,
            11..=15 => Self::C,
            16..=20 => Self::D,
            _ => Self::F,
        }
    }
}

/// A single detected code smell within a chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CodeSmell {
    LongFunction { lines: usize },
    DeepNesting { max_depth: u8 },
    TooManyParams { count: usize },
    MissingDocstring,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_from_cyclomatic_thresholds() {
        assert_eq!(ComplexityGrade::from_cyclomatic(0), ComplexityGrade::A);
        assert_eq!(ComplexityGrade::from_cyclomatic(5), ComplexityGrade::A);
        assert_eq!(ComplexityGrade::from_cyclomatic(6), ComplexityGrade::B);
        assert_eq!(ComplexityGrade::from_cyclomatic(11), ComplexityGrade::C);
        assert_eq!(ComplexityGrade::from_cyclomatic(16), ComplexityGrade::D);
        assert_eq!(ComplexityGrade::from_cyclomatic(50), ComplexityGrade::F);
    }

    #[test]
    fn metrics_round_trip_json() {
        let m = ComplexityMetrics {
            cyclomatic: 7,
            cognitive: 12,
            grade: ComplexityGrade::B,
            smells: vec![CodeSmell::LongFunction { lines: 80 }],
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: ComplexityMetrics = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn metrics_default_when_field_missing() {
        // Forward-compat: trusty-search may emit chunks without a
        // `complexity` field. Default deserialization must not fail.
        let s = r#"{}"#;
        let m: ComplexityMetrics = serde_json::from_str(s).unwrap();
        assert_eq!(m, ComplexityMetrics::default());
    }
}
