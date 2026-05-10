//! Go `LanguageAnalyzer` adapter backed by tree-sitter-go.
//!
//! Why: Extracts Go structure — functions, methods, types (struct /
//! interface), imports, test functions — into a language-neutral `KgGraph`.
//!
//! What: For each `CodeChunk`, parses with tree-sitter-go, walks the tree,
//! and emits:
//! - `Function` for `function_declaration`
//! - `Method` for `method_declaration` (has a receiver)
//! - `Class` for `type_declaration` wrapping a `struct_type`
//! - `Interface` for `type_declaration` wrapping an `interface_type`
//! - `Import` + `Imports` edges for `import_declaration` / `import_spec`
//! - `TestCase` for functions named `Test*` taking `*testing.T`
//! - `is_public` set when the identifier starts with an uppercase letter
//! - doc_comment captured from a `comment` immediately preceding the decl
//!
//! Test: `go_extracts_function` and `go_test_function_detected` cover the
//! happy paths.

use tree_sitter::{Node, Parser};
use trusty_common::{CodeChunk, KgEdge, KgEdgeKind, KgGraph, KgNode, KgNodeKind};

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

/// tree-sitter-go-backed analyzer.
pub struct GoAnalyzer;

impl GoAnalyzer {
    /// Construct a stateless analyzer.
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
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .is_err()
        {
            return StaticAnalysisResult {
                errors: vec!["failed to load tree-sitter-go grammar".into()],
                ..Default::default()
            };
        }

        let mut result = StaticAnalysisResult::default();
        let mut seen_files = std::collections::HashSet::new();

        for chunk in chunks {
            tracing::debug!(file = %chunk.file, "go analyze chunk");
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
        id: format!("go:File:{file}"),
        kind: KgNodeKind::File,
        name: file.to_string(),
        qualified_name: file.to_string(),
        language: "go".into(),
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

/// Capitalized identifier → exported (`is_public: true`) in Go.
fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Walk backward through preceding comment siblings and join them.
fn preceding_doc(node: Node, src: &[u8]) -> Option<String> {
    let mut sib = node.prev_sibling();
    let mut parts: Vec<String> = Vec::new();
    while let Some(s) = sib {
        if s.kind() == "comment" {
            parts.push(node_text(s, src));
            sib = s.prev_sibling();
        } else {
            break;
        }
    }
    if parts.is_empty() {
        None
    } else {
        parts.reverse();
        Some(parts.join("\n"))
    }
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
    KgNode {
        id: format!("go:{kind_str}:{}:{name}", chunk.file),
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        language: "go".into(),
        file: chunk.file.clone(),
        start_line: start,
        end_line: end,
        doc_comment: doc,
        is_public: is_exported(name),
        extra: serde_json::Value::Null,
    }
}

/// Inspect a `function_declaration` to decide if it's a Go test function
/// (name starts with `Test` and first parameter is `*testing.T`).
fn is_test_function(name: &str, fn_node: Node, src: &[u8]) -> bool {
    if !name.starts_with("Test") {
        return false;
    }
    let Some(params) = fn_node.child_by_field_name("parameters") else {
        return false;
    };
    let txt = node_text(params, src);
    txt.contains("testing.T")
}

fn walk(root: Node, src: &[u8], chunk: &CodeChunk, graph: &mut KgGraph) {
    let file_id = format!("go:File:{}", chunk.file);
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        emit_top_level(
            child,
            src,
            chunk,
            &file_id,
            &mut graph.nodes,
            &mut graph.edges,
        );
    }
}

fn emit_top_level(
    node: Node,
    src: &[u8],
    chunk: &CodeChunk,
    file_id: &str,
    nodes: &mut Vec<KgNode>,
    edges: &mut Vec<KgEdge>,
) {
    match node.kind() {
        "function_declaration" => {
            let Some(name) = name_of(node, src) else {
                return;
            };
            let doc = preceding_doc(node, src);
            let kind = if is_test_function(&name, node, src) {
                KgNodeKind::TestCase
            } else {
                KgNodeKind::Function
            };
            let n = make_node(kind, &name, chunk, node, doc);
            let id = n.id.clone();
            nodes.push(n);
            edges.push(KgEdge {
                from: file_id.to_string(),
                to: id,
                kind: KgEdgeKind::Contains,
                weight: 1.0,
            });
        }
        "method_declaration" => {
            let Some(name) = name_of(node, src) else {
                return;
            };
            let doc = preceding_doc(node, src);
            let n = make_node(KgNodeKind::Method, &name, chunk, node, doc);
            let id = n.id.clone();
            nodes.push(n);
            edges.push(KgEdge {
                from: file_id.to_string(),
                to: id,
                kind: KgEdgeKind::Contains,
                weight: 1.0,
            });
        }
        "type_declaration" => {
            // type_declaration → type_spec(s) → name + type
            let doc = preceding_doc(node, src);
            let mut cursor = node.walk();
            for spec in node.children(&mut cursor) {
                if spec.kind() != "type_spec" {
                    continue;
                }
                let Some(name) = name_of(spec, src) else {
                    continue;
                };
                let Some(type_node) = spec.child_by_field_name("type") else {
                    continue;
                };
                let kind = match type_node.kind() {
                    "struct_type" => KgNodeKind::Class,
                    "interface_type" => KgNodeKind::Interface,
                    _ => continue,
                };
                let n = make_node(kind, &name, chunk, spec, doc.clone());
                let id = n.id.clone();
                nodes.push(n);
                edges.push(KgEdge {
                    from: file_id.to_string(),
                    to: id,
                    kind: KgEdgeKind::Contains,
                    weight: 1.0,
                });
            }
        }
        "import_declaration" => {
            // import_declaration may contain a single import_spec or an
            // import_spec_list wrapping many.
            let mut stack = vec![node];
            while let Some(cur) = stack.pop() {
                if cur.kind() == "import_spec" {
                    let txt = node_text(cur, src).trim().to_string();
                    if !txt.is_empty() {
                        let n = make_node(KgNodeKind::Import, &txt, chunk, cur, None);
                        let id = n.id.clone();
                        nodes.push(n);
                        edges.push(KgEdge {
                            from: file_id.to_string(),
                            to: id,
                            kind: KgEdgeKind::Imports,
                            weight: 1.0,
                        });
                    }
                    continue;
                }
                let mut c = cur.walk();
                for child in cur.children(&mut c) {
                    stack.push(child);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(content: &str) -> CodeChunk {
        CodeChunk {
            id: "main.go:1:20".into(),
            file: "main.go".into(),
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
    fn go_supports_go_files() {
        let a = GoAnalyzer::new();
        assert!(a.supports("main.go"));
        assert!(!a.supports("main.rs"));
    }

    #[test]
    fn go_extracts_function() {
        let a = GoAnalyzer::new();
        let c = make_chunk("package main\n\nfunc Hello() {}\n");
        let r = a.analyze_chunks(&[c]);
        assert_eq!(r.analyzed_chunks, 1);
        let funcs: Vec<&KgNode> = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Function))
            .collect();
        assert_eq!(funcs.len(), 1, "graph: {:?}", r.graph.nodes);
        assert_eq!(funcs[0].name, "Hello");
        assert!(funcs[0].is_public, "Hello should be exported");
    }

    #[test]
    fn go_lowercase_function_is_not_public() {
        let a = GoAnalyzer::new();
        let c = make_chunk("package main\n\nfunc helper() {}\n");
        let r = a.analyze_chunks(&[c]);
        let f = r
            .graph
            .nodes
            .iter()
            .find(|n| matches!(n.kind, KgNodeKind::Function))
            .unwrap();
        assert!(!f.is_public);
    }

    #[test]
    fn go_test_function_detected() {
        let a = GoAnalyzer::new();
        let c = make_chunk("package main\n\nimport \"testing\"\n\nfunc TestFoo(t *testing.T) {}\n");
        let r = a.analyze_chunks(&[c]);
        assert!(
            r.graph
                .nodes
                .iter()
                .any(|n| matches!(n.kind, KgNodeKind::TestCase) && n.name == "TestFoo"),
            "graph: {:?}",
            r.graph.nodes
        );
    }

    #[test]
    fn go_extracts_struct_and_interface() {
        let a = GoAnalyzer::new();
        let c = make_chunk(
            "package main\n\
             \n\
             type Foo struct { X int }\n\
             type Bar interface { Run() }\n",
        );
        let r = a.analyze_chunks(&[c]);
        let kinds: Vec<&KgNodeKind> = r.graph.nodes.iter().map(|n| &n.kind).collect();
        assert!(kinds.iter().any(|k| matches!(k, KgNodeKind::Class)));
        assert!(kinds.iter().any(|k| matches!(k, KgNodeKind::Interface)));
    }

    #[test]
    fn go_extracts_method() {
        let a = GoAnalyzer::new();
        let c = make_chunk(
            "package main\n\
             \n\
             type Foo struct{}\n\
             func (f *Foo) Bar() {}\n",
        );
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::Method) && n.name == "Bar"));
    }

    #[test]
    fn go_extracts_imports() {
        let a = GoAnalyzer::new();
        let c = make_chunk("package main\n\nimport (\n    \"fmt\"\n    \"os\"\n)\n");
        let r = a.analyze_chunks(&[c]);
        let imports = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Import))
            .count();
        assert_eq!(imports, 2);
    }

    #[test]
    fn go_doc_comment_captured() {
        let a = GoAnalyzer::new();
        let c = make_chunk("package main\n\n// Hello greets the world.\nfunc Hello() {}\n");
        let r = a.analyze_chunks(&[c]);
        let f = r
            .graph
            .nodes
            .iter()
            .find(|n| matches!(n.kind, KgNodeKind::Function))
            .unwrap();
        assert!(f.doc_comment.is_some());
        assert!(f.doc_comment.as_ref().unwrap().contains("greets"));
    }
}
