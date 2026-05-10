//! Java adapter — Phase 2 stub.
//!
//! Why: Reserves the language slot so the registry has a `JavaAnalyzer` to
//! dispatch to. The full AST walk is deferred to Phase 2b.
//!
//! What: Returns an empty `StaticAnalysisResult`.
//!
//! Test: `java_supports_java_files` covers the default `supports` impl.

use trusty_common::CodeChunk;

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

pub struct JavaAnalyzer;

impl JavaAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JavaAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageAnalyzer for JavaAnalyzer {
    fn language(&self) -> &str {
        "java"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".java"]
    }

    fn analyze_chunks(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        tracing::debug!(
            "JavaAnalyzer stub invoked on {} chunks (Phase 2b)",
            chunks.len()
        );
        let _ = tree_sitter_java::LANGUAGE;
        StaticAnalysisResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_supports_java_files() {
        let a = JavaAnalyzer::new();
        assert!(a.supports("Foo.java"));
        assert!(!a.supports("foo.go"));
    }
}
