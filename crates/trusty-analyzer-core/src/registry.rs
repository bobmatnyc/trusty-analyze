//! `AnalyzerRegistry`: dispatches `CodeChunk`s to the right `LanguageAnalyzer`.
//!
//! Why: The service crate doesn't want to know which adapters exist — it
//! just wants to call `registry.analyze(&chunks)` and get back a merged
//! `StaticAnalysisResult`. This module owns the list of built-in adapters
//! and the file-extension-based routing.
//!
//! What: A thin facade over `Vec<Arc<dyn LanguageAnalyzer>>`. The
//! `default_registry` constructor registers every adapter currently
//! shipping. `analyze` partitions chunks by adapter and merges the
//! per-language results.
//!
//! Test: `default_registry_constructs` ensures we can build the registry
//! without panicking and that every advertised adapter is present.

use std::sync::Arc;

use trusty_analyzer_lang::{
    GoAnalyzer, JavaAnalyzer, JavaScriptAnalyzer, LanguageAnalyzer, PythonAnalyzer, RustAnalyzer,
    StaticAnalysisResult, TypeScriptAnalyzer,
};
use trusty_common::CodeChunk;

/// Holds all registered language analyzers and dispatches to the right one.
pub struct AnalyzerRegistry {
    analyzers: Vec<Arc<dyn LanguageAnalyzer>>,
}

impl AnalyzerRegistry {
    /// Create a registry with all built-in adapters registered.
    pub fn default_registry() -> Self {
        let analyzers: Vec<Arc<dyn LanguageAnalyzer>> = vec![
            Arc::new(RustAnalyzer::new()),
            Arc::new(TypeScriptAnalyzer::new()),
            Arc::new(JavaScriptAnalyzer::new()),
            Arc::new(PythonAnalyzer::new()),
            Arc::new(JavaAnalyzer::new()),
            Arc::new(GoAnalyzer::new()),
        ];
        Self { analyzers }
    }

    /// Empty registry (useful for tests).
    pub fn empty() -> Self {
        Self {
            analyzers: Vec::new(),
        }
    }

    /// Register an additional analyzer (e.g. a custom plugin).
    pub fn register(&mut self, analyzer: Arc<dyn LanguageAnalyzer>) {
        self.analyzers.push(analyzer);
    }

    /// Iterate over all registered analyzers.
    pub fn analyzers(&self) -> &[Arc<dyn LanguageAnalyzer>] {
        &self.analyzers
    }

    /// Get the first analyzer that claims to support `file`, if any.
    pub fn analyzer_for(&self, file: &str) -> Option<Arc<dyn LanguageAnalyzer>> {
        self.analyzers.iter().find(|a| a.supports(file)).cloned()
    }

    /// Analyze a corpus by dispatching each chunk to the matching adapter
    /// by file extension. Chunks with no matching adapter are skipped and
    /// logged at debug level. Results from every adapter are merged.
    pub fn analyze(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        let mut by_adapter: Vec<(Arc<dyn LanguageAnalyzer>, Vec<CodeChunk>)> = self
            .analyzers
            .iter()
            .map(|a| (a.clone(), Vec::new()))
            .collect();

        let mut unrouted = 0usize;
        'outer: for chunk in chunks {
            for slot in by_adapter.iter_mut() {
                if slot.0.supports(&chunk.file) {
                    slot.1.push(chunk.clone());
                    continue 'outer;
                }
            }
            unrouted += 1;
        }

        if unrouted > 0 {
            tracing::debug!("AnalyzerRegistry: {unrouted} chunks had no matching language adapter");
        }

        let mut merged = StaticAnalysisResult::default();
        for (adapter, chunks) in by_adapter {
            if chunks.is_empty() {
                continue;
            }
            tracing::info!(
                "running {} adapter on {} chunks",
                adapter.language(),
                chunks.len()
            );
            let res = adapter.analyze_chunks(&chunks);
            merged.analyzed_files += res.analyzed_files;
            merged.analyzed_chunks += res.analyzed_chunks;
            merged.errors.extend(res.errors);
            merged.graph.merge(res.graph);
        }
        merged
    }
}

impl Default for AnalyzerRegistry {
    fn default() -> Self {
        Self::default_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trusty_common::KgNodeKind;

    fn chunk(file: &str, content: &str) -> CodeChunk {
        CodeChunk {
            id: format!("{file}:1:5"),
            file: file.into(),
            start_line: 1,
            end_line: 5,
            content: content.into(),
            function_name: None,
            score: 0.0,
            compact_snippet: None,
            match_reason: String::new(),
            complexity: None,
            blame: None,
        }
    }

    #[test]
    fn default_registry_constructs() {
        let r = AnalyzerRegistry::default_registry();
        let langs: Vec<&str> = r.analyzers().iter().map(|a| a.language()).collect();
        for required in ["rust", "typescript", "javascript", "python", "java", "go"] {
            assert!(langs.contains(&required), "missing {required} in {langs:?}");
        }
    }

    #[test]
    fn analyzer_for_dispatches_by_extension() {
        let r = AnalyzerRegistry::default_registry();
        assert_eq!(
            r.analyzer_for("foo.rs").map(|a| a.language().to_string()),
            Some("rust".into())
        );
        assert_eq!(
            r.analyzer_for("foo.ts").map(|a| a.language().to_string()),
            Some("typescript".into())
        );
        assert!(r.analyzer_for("README.md").is_none());
    }

    #[test]
    fn analyze_routes_chunks_per_language() {
        let r = AnalyzerRegistry::default_registry();
        let chunks = vec![
            chunk("a.rs", "fn rust_fn() {}\n"),
            chunk("b.ts", "function ts_fn() { return 1; }\n"),
        ];
        let res = r.analyze(&chunks);
        let names: Vec<&str> = res
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Function))
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"rust_fn"));
        assert!(names.contains(&"ts_fn"));
    }
}
