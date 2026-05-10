//! Go adapter — Phase 2 stub.
//!
//! Why: Reserves the language slot so the registry has a `GoAnalyzer` to
//! dispatch to. The full AST walk is deferred to Phase 2b.
//!
//! What: Returns an empty `StaticAnalysisResult`.
//!
//! Test: `go_supports_go_files` covers the default `supports` impl.

use trusty_common::CodeChunk;

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

pub struct GoAnalyzer;

impl GoAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GoAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageAnalyzer for GoAnalyzer {
    fn language(&self) -> &str {
        "go"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".go"]
    }

    fn analyze_chunks(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        tracing::debug!(
            "GoAnalyzer stub invoked on {} chunks (Phase 2b)",
            chunks.len()
        );
        let _ = tree_sitter_go::LANGUAGE;
        StaticAnalysisResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_supports_go_files() {
        let a = GoAnalyzer::new();
        assert!(a.supports("main.go"));
        assert!(!a.supports("main.rs"));
    }
}
