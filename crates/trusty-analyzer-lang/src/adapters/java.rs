//! Java `LanguageAnalyzer` adapter backed by tree-sitter-java.
//!
//! Why: Extracts Java structure — classes, interfaces, methods, imports,
//! superclass/superinterface relationships — into a language-neutral
//! `KgGraph`. Mirrors the Rust/TypeScript/Python adapters.
//!
//! What: For each `CodeChunk`, parses the content with tree-sitter-java,
//! walks the tree, and emits:
//! - `Class` for `class_declaration`
//! - `Interface` for `interface_declaration`
//! - `Method` for `method_declaration` inside a class/interface
//! - `Import` + `Imports` edges for `import_declaration`
//! - `TestCase` for methods annotated `@Test`
//! - `Extends` edges for `superclass` clauses
//! - `Implements` edges for `super_interfaces` clauses
//! - `Contains` edges from the file to top-level types and from types to
//!   their members
//!
//! Test: `java_extracts_class_and_method` covers a minimal class with one
//! method.

use tree_sitter::{Node, Parser};
use trusty_common::{CodeChunk, KgEdge, KgEdgeKind, KgGraph, KgNode, KgNodeKind};

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

/// tree-sitter-java-backed analyzer.
pub struct JavaAnalyzer;

impl JavaAnalyzer {
    /// Construct a stateless analyzer.
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
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .is_err()
        {
            return StaticAnalysisResult {
                errors: vec!["failed to load tree-sitter-java grammar".into()],
                ..Default::default()
            };
        }

        let mut result = StaticAnalysisResult::default();
        let mut seen_files = std::collections::HashSet::new();

        for chunk in chunks {
            tracing::debug!(file = %chunk.file, "java analyze chunk");
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
        id: format!("java:File:{file}"),
        kind: KgNodeKind::File,
        name: file.to_string(),
        qualified_name: file.to_string(),
        language: "java".into(),
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

fn is_public(node: Node, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            return node_text(child, src).contains("public");
        }
    }
    false
}

/// True if any modifier annotation on `node` is `@Test`.
fn has_test_annotation(node: Node, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut c2 = child.walk();
            for m in child.children(&mut c2) {
                if m.kind() == "annotation" || m.kind() == "marker_annotation" {
                    let txt = node_text(m, src);
                    if txt.starts_with("@Test") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Find the immediately preceding block_comment if it's a Javadoc `/** ... */`.
fn javadoc(node: Node, src: &[u8]) -> Option<String> {
    let prev = node.prev_sibling()?;
    if prev.kind() == "block_comment" {
        let txt = node_text(prev, src);
        if txt.starts_with("/**") {
            return Some(txt);
        }
    }
    None
}

fn make_node(
    kind: KgNodeKind,
    name: &str,
    chunk: &CodeChunk,
    ast: Node,
    is_pub: bool,
    doc: Option<String>,
) -> KgNode {
    let start = (chunk.start_line as u32).saturating_add(ast.start_position().row as u32);
    let end = (chunk.start_line as u32).saturating_add(ast.end_position().row as u32);
    let kind_str = format!("{kind:?}");
    KgNode {
        id: format!("java:{kind_str}:{}:{name}", chunk.file),
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        language: "java".into(),
        file: chunk.file.clone(),
        start_line: start,
        end_line: end,
        doc_comment: doc,
        is_public: is_pub,
        extra: serde_json::Value::Null,
    }
}

fn walk(root: Node, src: &[u8], chunk: &CodeChunk, graph: &mut KgGraph) {
    let file_id = format!("java:File:{}", chunk.file);

    fn recurse(
        node: Node,
        src: &[u8],
        chunk: &CodeChunk,
        graph: &mut KgGraph,
        parent_id: &str,
        inside_type: bool,
    ) {
        match node.kind() {
            "class_declaration" | "interface_declaration" => {
                let is_iface = node.kind() == "interface_declaration";
                let Some(name) = name_of(node, src) else {
                    return;
                };
                let pub_ = is_public(node, src);
                let doc = javadoc(node, src);
                let class_kind = if is_iface {
                    KgNodeKind::Interface
                } else {
                    KgNodeKind::Class
                };
                let n = make_node(class_kind.clone(), &name, chunk, node, pub_, doc);
                let class_id = n.id.clone();
                graph.nodes.push(n);
                graph.edges.push(KgEdge {
                    from: parent_id.to_string(),
                    to: class_id.clone(),
                    kind: KgEdgeKind::Contains,
                    weight: 1.0,
                });

                // superclass → Extends edge
                if let Some(sup) = node.child_by_field_name("superclass") {
                    // sup is a `superclass` node wrapping a type identifier
                    let mut c = sup.walk();
                    for ch in sup.children(&mut c) {
                        if ch.kind() == "type_identifier" {
                            let target = node_text(ch, src);
                            let to_id = format!("java:Class:{}:{target}", chunk.file);
                            graph.edges.push(KgEdge {
                                from: class_id.clone(),
                                to: to_id,
                                kind: KgEdgeKind::Extends,
                                weight: 1.0,
                            });
                        }
                    }
                }
                // super_interfaces → Implements edges
                if let Some(supi) = node.child_by_field_name("interfaces") {
                    add_super_interface_edges(supi, src, chunk, &class_id, graph);
                } else {
                    // tree-sitter-java sometimes attaches it as a sibling child
                    let mut c = node.walk();
                    for ch in node.children(&mut c) {
                        if ch.kind() == "super_interfaces" {
                            add_super_interface_edges(ch, src, chunk, &class_id, graph);
                        }
                    }
                }

                // Recurse into body
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for child in body.children(&mut cursor) {
                        recurse(child, src, chunk, graph, &class_id, true);
                    }
                }
                return;
            }
            "method_declaration" => {
                let Some(name) = name_of(node, src) else {
                    return;
                };
                let pub_ = is_public(node, src);
                let doc = javadoc(node, src);
                let is_test = has_test_annotation(node, src);
                let kind = if is_test {
                    KgNodeKind::TestCase
                } else if inside_type {
                    KgNodeKind::Method
                } else {
                    KgNodeKind::Function
                };
                let n = make_node(kind, &name, chunk, node, pub_, doc);
                let id = n.id.clone();
                graph.nodes.push(n);
                graph.edges.push(KgEdge {
                    from: parent_id.to_string(),
                    to: id,
                    kind: KgEdgeKind::Contains,
                    weight: 1.0,
                });
                return;
            }
            "import_declaration" => {
                let txt = node_text(node, src).trim().to_string();
                if !txt.is_empty() {
                    let n = make_node(KgNodeKind::Import, &txt, chunk, node, false, None);
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
            recurse(child, src, chunk, graph, parent_id, inside_type);
        }
    }

    recurse(root, src, chunk, graph, &file_id, false);
}

fn add_super_interface_edges(
    super_node: Node,
    src: &[u8],
    chunk: &CodeChunk,
    from_id: &str,
    graph: &mut KgGraph,
) {
    let mut stack = vec![super_node];
    while let Some(n) = stack.pop() {
        if n.kind() == "type_identifier" {
            let target = node_text(n, src);
            let to_id = format!("java:Interface:{}:{target}", chunk.file);
            graph.edges.push(KgEdge {
                from: from_id.to_string(),
                to: to_id,
                kind: KgEdgeKind::Implements,
                weight: 1.0,
            });
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(content: &str) -> CodeChunk {
        CodeChunk {
            id: "Foo.java:1:20".into(),
            file: "Foo.java".into(),
            start_line: 1,
            end_line: 20,
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
    fn java_supports_java_files() {
        let a = JavaAnalyzer::new();
        assert!(a.supports("Foo.java"));
        assert!(!a.supports("foo.go"));
    }

    #[test]
    fn java_extracts_class_and_method() {
        let a = JavaAnalyzer::new();
        let c = make_chunk("public class Foo {\n    public void bar() {}\n}\n");
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
        let class = r
            .graph
            .nodes
            .iter()
            .find(|n| matches!(n.kind, KgNodeKind::Class))
            .unwrap();
        assert!(class.is_public);
    }

    #[test]
    fn java_extracts_interface() {
        let a = JavaAnalyzer::new();
        let c = make_chunk("public interface Bar { void baz(); }\n");
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::Interface)));
    }

    #[test]
    fn java_test_method_detected() {
        let a = JavaAnalyzer::new();
        let c = make_chunk("class FooTest {\n    @Test\n    public void shouldWork() {}\n}\n");
        let r = a.analyze_chunks(&[c]);
        assert!(
            r.graph
                .nodes
                .iter()
                .any(|n| matches!(n.kind, KgNodeKind::TestCase)),
            "graph: {:?}",
            r.graph.nodes
        );
    }

    #[test]
    fn java_extracts_imports() {
        let a = JavaAnalyzer::new();
        let c = make_chunk("import java.util.List;\nclass A {}\n");
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::Import)));
    }

    #[test]
    fn java_implements_edge_emitted() {
        let a = JavaAnalyzer::new();
        let c = make_chunk(
            "class Foo implements Runnable, AutoCloseable {\n    public void run() {}\n    public void close() {}\n}\n",
        );
        let r = a.analyze_chunks(&[c]);
        let impls = r
            .graph
            .edges
            .iter()
            .filter(|e| matches!(e.kind, KgEdgeKind::Implements))
            .count();
        assert!(impls >= 2, "expected >= 2 Implements edges, got {impls}");
    }
}
