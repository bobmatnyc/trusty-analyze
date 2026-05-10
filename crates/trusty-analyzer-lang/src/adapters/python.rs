//! Python `LanguageAnalyzer` adapter backed by tree-sitter-python.
//!
//! Why: Extracts Python structure — functions, classes, methods, imports,
//! and test cases — into a language-neutral `KgGraph`. Mirrors the Rust and
//! TypeScript adapters so the analyzer registry behaves uniformly across
//! languages.
//!
//! What: For each `CodeChunk`, parses the content with tree-sitter-python,
//! walks the tree, and emits:
//! - one `File` node per unique `chunk.file`
//! - `Function` nodes for top-level `function_definition`
//! - `Method` nodes for `function_definition` nested in a class
//! - `Class` nodes for `class_definition`
//! - `Import` nodes + `Imports` edges for `import_statement` /
//!   `import_from_statement`
//! - `TestCase` nodes for functions decorated with anything containing `test`
//!   or named `test_*`
//! - `Contains` edges from file to top-level items, and from classes to
//!   their methods
//!
//! Test: `python_extracts_function` and `python_extracts_class` cover the
//! basic happy paths.

use tree_sitter::{Node, Parser};
use trusty_common::{CodeChunk, KgEdge, KgEdgeKind, KgGraph, KgNode, KgNodeKind};

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

/// tree-sitter-python-backed analyzer.
pub struct PythonAnalyzer;

impl PythonAnalyzer {
    /// Construct a stateless analyzer.
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
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .is_err()
        {
            return StaticAnalysisResult {
                errors: vec!["failed to load tree-sitter-python grammar".into()],
                ..Default::default()
            };
        }

        let mut result = StaticAnalysisResult::default();
        let mut seen_files = std::collections::HashSet::new();

        for chunk in chunks {
            tracing::debug!(file = %chunk.file, "python analyze chunk");
            let Some(tree) = parser.parse(&chunk.content, None) else {
                result.errors.push(format!("parse failure: {}", chunk.file));
                continue;
            };
            result.analyzed_chunks += 1;
            if seen_files.insert(chunk.file.clone()) {
                result.analyzed_files += 1;
                result.graph.nodes.push(file_node(&chunk.file));
            }

            let src = chunk.content.as_bytes();
            walk(tree.root_node(), src, chunk, &mut result.graph);
        }

        result
    }
}

fn file_node(file: &str) -> KgNode {
    KgNode {
        id: format!("python:File:{file}"),
        kind: KgNodeKind::File,
        name: file.to_string(),
        qualified_name: file.to_string(),
        language: "python".into(),
        file: file.to_string(),
        start_line: 0,
        end_line: 0,
        doc_comment: None,
        is_public: false,
        extra: serde_json::Value::Null,
    }
}

fn node_text(node: Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or("").to_string()
}

fn name_of(node: Node, src: &[u8]) -> Option<String> {
    node.child_by_field_name("name").map(|n| node_text(n, src))
}

fn make_node(
    kind: KgNodeKind,
    name: &str,
    chunk: &CodeChunk,
    ast: Node,
    doc: Option<String>,
) -> KgNode {
    let start = (chunk.start_line as u32).saturating_add(ast.start_position().row as u32);
    let end = (chunk.start_line as u32).saturating_add(ast.end_position().row as u32);
    let kind_str = format!("{kind:?}");
    let is_public = !name.starts_with('_');
    KgNode {
        id: format!("python:{kind_str}:{}:{name}", chunk.file),
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        language: "python".into(),
        file: chunk.file.clone(),
        start_line: start,
        end_line: end,
        doc_comment: doc,
        is_public,
        extra: serde_json::Value::Null,
    }
}

/// First expression-statement-string child of `block` is the docstring.
fn extract_docstring(definition: Node, src: &[u8]) -> Option<String> {
    let body = definition.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            let mut c2 = child.walk();
            for inner in child.children(&mut c2) {
                if inner.kind() == "string" {
                    return Some(node_text(inner, src));
                }
            }
            return None;
        }
    }
    None
}

/// True if any decorator on `decorated_definition` matches a test pattern.
fn has_test_decorator(decorated: Node, src: &[u8]) -> bool {
    let mut cursor = decorated.walk();
    for child in decorated.children(&mut cursor) {
        if child.kind() == "decorator" {
            let txt = node_text(child, src);
            if txt.contains("test") || txt.contains("pytest") {
                return true;
            }
        }
    }
    false
}

fn walk(root: Node, src: &[u8], chunk: &CodeChunk, graph: &mut KgGraph) {
    let file_id = format!("python:File:{}", chunk.file);

    fn recurse(
        node: Node,
        src: &[u8],
        chunk: &CodeChunk,
        graph: &mut KgGraph,
        parent_id: &str,
        inside_class: bool,
    ) {
        match node.kind() {
            "function_definition" => {
                if let Some(name) = name_of(node, src) {
                    let is_test = inside_class.eq(&false) && name.starts_with("test_");
                    let kind = if is_test {
                        KgNodeKind::TestCase
                    } else if inside_class {
                        KgNodeKind::Method
                    } else {
                        KgNodeKind::Function
                    };
                    let doc = extract_docstring(node, src);
                    let n = make_node(kind, &name, chunk, node, doc);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: parent_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                }
                // Don't recurse into function body for symbol extraction.
                return;
            }
            "decorated_definition" => {
                // Find the inner definition (function or class).
                let mut cursor = node.walk();
                let mut inner_def: Option<Node> = None;
                for child in node.children(&mut cursor) {
                    if child.kind() == "function_definition" || child.kind() == "class_definition" {
                        inner_def = Some(child);
                        break;
                    }
                }
                let Some(def) = inner_def else {
                    return;
                };
                if def.kind() == "function_definition" {
                    if let Some(name) = name_of(def, src) {
                        let is_test = has_test_decorator(node, src) || name.starts_with("test_");
                        let kind = if is_test {
                            KgNodeKind::TestCase
                        } else if inside_class {
                            KgNodeKind::Method
                        } else {
                            KgNodeKind::Function
                        };
                        let doc = extract_docstring(def, src);
                        let n = make_node(kind, &name, chunk, def, doc);
                        let id = n.id.clone();
                        graph.nodes.push(n);
                        graph.edges.push(KgEdge {
                            from: parent_id.to_string(),
                            to: id,
                            kind: KgEdgeKind::Contains,
                            weight: 1.0,
                        });
                    }
                    return;
                }
                // class_definition: fall through to handle it normally.
                recurse(def, src, chunk, graph, parent_id, inside_class);
                return;
            }
            "class_definition" => {
                if let Some(name) = name_of(node, src) {
                    let doc = extract_docstring(node, src);
                    let n = make_node(KgNodeKind::Class, &name, chunk, node, doc);
                    let class_id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: parent_id.to_string(),
                        to: class_id.clone(),
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                    // Recurse into class body with class as new parent + inside_class=true.
                    if let Some(body) = node.child_by_field_name("body") {
                        let mut cursor = body.walk();
                        for child in body.children(&mut cursor) {
                            recurse(child, src, chunk, graph, &class_id, true);
                        }
                    }
                }
                return;
            }
            "import_statement" | "import_from_statement" => {
                let txt = node_text(node, src);
                let cleaned = txt.trim().to_string();
                if !cleaned.is_empty() {
                    let n = make_node(KgNodeKind::Import, &cleaned, chunk, node, None);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: parent_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Imports,
                        weight: 1.0,
                    });
                }
                return;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            recurse(child, src, chunk, graph, parent_id, inside_class);
        }
    }

    recurse(root, src, chunk, graph, &file_id, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(content: &str) -> CodeChunk {
        CodeChunk {
            id: "f.py:1:10".into(),
            file: "f.py".into(),
            start_line: 1,
            end_line: 10,
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
    fn python_supports_py_files() {
        let a = PythonAnalyzer::new();
        assert!(a.supports("foo.py"));
        assert!(a.supports("stubs.pyi"));
        assert!(!a.supports("foo.rs"));
    }

    #[test]
    fn python_extracts_function() {
        let a = PythonAnalyzer::new();
        let c = make_chunk("def hello():\n    pass\n");
        let r = a.analyze_chunks(&[c]);
        assert_eq!(r.analyzed_chunks, 1);
        let funcs: Vec<&KgNode> = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Function))
            .collect();
        assert_eq!(funcs.len(), 1, "graph: {:?}", r.graph.nodes);
        assert_eq!(funcs[0].name, "hello");
        assert_eq!(funcs[0].language, "python");
        assert!(funcs[0].is_public);
    }

    #[test]
    fn python_extracts_class() {
        let a = PythonAnalyzer::new();
        let c = make_chunk("class Foo:\n    def bar(self):\n        pass\n");
        let r = a.analyze_chunks(&[c]);
        let kinds: Vec<&KgNodeKind> = r.graph.nodes.iter().map(|n| &n.kind).collect();
        assert!(
            kinds.iter().any(|k| matches!(k, KgNodeKind::Class)),
            "expected Class, got {:?}",
            kinds
        );
        assert!(
            kinds.iter().any(|k| matches!(k, KgNodeKind::Method)),
            "expected Method, got {:?}",
            kinds
        );
    }

    #[test]
    fn python_private_function_is_not_public() {
        let a = PythonAnalyzer::new();
        let c = make_chunk("def _hidden():\n    pass\n");
        let r = a.analyze_chunks(&[c]);
        let f = r
            .graph
            .nodes
            .iter()
            .find(|n| matches!(n.kind, KgNodeKind::Function))
            .expect("function node");
        assert!(!f.is_public);
    }

    #[test]
    fn python_test_function_detected() {
        let a = PythonAnalyzer::new();
        let c = make_chunk("def test_login():\n    pass\n");
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::TestCase)));
    }

    #[test]
    fn python_extracts_imports() {
        let a = PythonAnalyzer::new();
        let c = make_chunk("import os\nfrom typing import List\n");
        let r = a.analyze_chunks(&[c]);
        let imports: Vec<&KgNode> = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Import))
            .collect();
        assert_eq!(imports.len(), 2);
    }
}
