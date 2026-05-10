//! Python adapter — Phase 2 stub.
//!
//! Why: Reserves the language slot so the registry has a `PythonAnalyzer`
//! to dispatch to. The full AST walk is deferred to Phase 2b.
//!
//! What: Returns an empty `StaticAnalysisResult`; counts chunks but emits
//! nothing into the graph.
//!
//! Test: `python_supports_py_files` exercises the default `supports` impl.

use trusty_common::CodeChunk;

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

pub struct PythonAnalyzer;

impl PythonAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PythonAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageAnalyzer for PythonAnalyzer {
    fn language(&self) -> &str {
        "python"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".py", ".pyi"]
    }

    fn analyze_chunks(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        // Phase 2b will wire tree-sitter-python here.
        tracing::debug!(
            "PythonAnalyzer stub invoked on {} chunks (Phase 2b)",
            chunks.len()
        );
        let _ = tree_sitter_python::LANGUAGE; // keep dependency live
        StaticAnalysisResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_supports_py_files() {
        let a = PythonAnalyzer::new();
        assert!(a.supports("foo.py"));
        assert!(a.supports("stubs.pyi"));
        assert!(!a.supports("foo.rs"));
    }
}
