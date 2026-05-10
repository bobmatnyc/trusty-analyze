//! Rust `LanguageAnalyzer` adapter backed by tree-sitter-rust.
//!
//! Why: Extracts the structural backbone of Rust source — fns, methods,
//! structs/enums, traits, modules, imports, tests — into a language-neutral
//! `KgGraph`. The walker is deliberately conservative: it only emits nodes
//! and edges we can derive directly from the AST without any name
//! resolution.
//!
//! What: For each `CodeChunk`, parse `chunk.content`, walk the syntax tree
//! once, and emit:
//! - one `File` node per unique chunk.file
//! - `Function` / `Method` nodes for `function_item`
//! - `Class` nodes for `struct_item` / `enum_item` / `union_item`
//! - `Interface` nodes for `trait_item`
//! - `Module` nodes for `mod_item`
//! - `Import` nodes + `Imports` edges for `use_declaration`
//! - `TestCase` for `#[test]` fns
//! - `Implements` edges for `impl Trait for Type`
//! - `Contains` edges from file to top-level items
//!
//! Test: `rust_analyzer_extracts_function` parses a minimal `fn hello() {}`
//! chunk and asserts a Function node is produced.

use tree_sitter::{Node, Parser};
use trusty_common::{CodeChunk, KgEdge, KgEdgeKind, KgGraph, KgNode, KgNodeKind};

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

/// tree-sitter-rust-backed analyzer.
pub struct RustAnalyzer;

impl RustAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RustAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageAnalyzer for RustAnalyzer {
    fn language(&self) -> &str {
        "rust"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".rs"]
    }

    fn analyze_chunks(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            return StaticAnalysisResult {
                errors: vec!["failed to load tree-sitter-rust grammar".into()],
                ..Default::default()
            };
        }

        let mut result = StaticAnalysisResult::default();
        let mut seen_files = std::collections::HashSet::new();

        for chunk in chunks {
            let tree = match parser.parse(&chunk.content, None) {
                Some(t) => t,
                None => {
                    result.errors.push(format!("parse failure: {}", chunk.file));
                    continue;
                }
            };
            result.analyzed_chunks += 1;
            if seen_files.insert(chunk.file.clone()) {
                result.analyzed_files += 1;
                let file_node = file_node(&chunk.file, "rust");
                result.graph.nodes.push(file_node);
            }

            let src = chunk.content.as_bytes();
            walk_rust(tree.root_node(), src, chunk, &mut result.graph);
        }

        result
    }
}

fn file_node(file: &str, language: &str) -> KgNode {
    KgNode {
        id: format!("{language}:File:{file}"),
        kind: KgNodeKind::File,
        name: file.to_string(),
        qualified_name: file.to_string(),
        language: language.to_string(),
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

fn child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

fn ident_text(node: Node, src: &[u8]) -> Option<String> {
    child_by_field(node, "name").map(|n| node_text(n, src))
}

fn is_public(node: Node, src: &[u8]) -> bool {
    // Look for a child `visibility_modifier` like `pub`.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return node_text(child, src).starts_with("pub");
        }
    }
    false
}

fn make_node(kind: KgNodeKind, name: &str, chunk: &CodeChunk, ast: Node, is_pub: bool) -> KgNode {
    // Lines from the chunk start are 0-based in tree-sitter; offset by chunk start.
    let start = (chunk.start_line as u32).saturating_add(ast.start_position().row as u32);
    let end = (chunk.start_line as u32).saturating_add(ast.end_position().row as u32);
    let kind_str = format!("{kind:?}");
    KgNode {
        id: format!("rust:{kind_str}:{}:{name}", chunk.file),
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        language: "rust".into(),
        file: chunk.file.clone(),
        start_line: start,
        end_line: end,
        doc_comment: None,
        is_public: is_pub,
        extra: serde_json::Value::Null,
    }
}

/// Recursive walker over a Rust AST. We push nodes/edges into `graph` as we
/// encounter declarations. Nested traversal is enough — we don't need a real
/// scope stack to emit symbol nodes.
fn walk_rust(node: Node, src: &[u8], chunk: &CodeChunk, graph: &mut KgGraph) {
    let file_id = format!("rust:File:{}", chunk.file);

    fn recurse(
        node: Node,
        src: &[u8],
        chunk: &CodeChunk,
        graph: &mut KgGraph,
        file_id: &str,
        inside_impl: bool,
    ) {
        match node.kind() {
            "function_item" => {
                if let Some(name) = ident_text(node, src) {
                    let is_test = has_test_attribute(node, src);
                    let kind = if is_test {
                        KgNodeKind::TestCase
                    } else if inside_impl {
                        KgNodeKind::Method
                    } else {
                        KgNodeKind::Function
                    };
                    let pub_ = is_public(node, src);
                    let n = make_node(kind, &name, chunk, node, pub_);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                }
            }
            "struct_item" | "enum_item" | "union_item" => {
                if let Some(name) = ident_text(node, src) {
                    let pub_ = is_public(node, src);
                    let n = make_node(KgNodeKind::Class, &name, chunk, node, pub_);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                }
            }
            "trait_item" => {
                if let Some(name) = ident_text(node, src) {
                    let pub_ = is_public(node, src);
                    let n = make_node(KgNodeKind::Interface, &name, chunk, node, pub_);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                }
            }
            "mod_item" => {
                if let Some(name) = ident_text(node, src) {
                    let pub_ = is_public(node, src);
                    let n = make_node(KgNodeKind::Module, &name, chunk, node, pub_);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                }
            }
            "use_declaration" => {
                // Pull the argument text as a single identifier name.
                let txt = node_text(node, src);
                let name = txt
                    .trim_start_matches("pub ")
                    .trim_start_matches("use ")
                    .trim_end_matches(';')
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    let n = make_node(KgNodeKind::Import, &name, chunk, node, false);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Imports,
                        weight: 1.0,
                    });
                }
            }
            "impl_item" => {
                // impl Trait for Type → Implements edge between the trait and the type.
                let type_ = child_by_field(node, "type").map(|n| node_text(n, src));
                let trait_ = child_by_field(node, "trait").map(|n| node_text(n, src));
                if let (Some(t), Some(tr)) = (type_.as_ref(), trait_.as_ref()) {
                    let type_id = format!("rust:Class:{}:{t}", chunk.file);
                    let trait_id = format!("rust:Interface:{}:{tr}", chunk.file);
                    graph.edges.push(KgEdge {
                        from: type_id,
                        to: trait_id,
                        kind: KgEdgeKind::Implements,
                        weight: 1.0,
                    });
                }
                // Recurse into the impl block so member fns get tagged as methods.
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    recurse(child, src, chunk, graph, file_id, true);
                }
                return;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            recurse(child, src, chunk, graph, file_id, inside_impl);
        }
    }

    recurse(node, src, chunk, graph, &file_id, false);
}

/// Returns true if any sibling/preceding `attribute_item` contains `test`.
/// We keep this lossy and string-based; `#[tokio::test]` matches as well,
/// which is the right default for our taxonomy.
fn has_test_attribute(node: Node, src: &[u8]) -> bool {
    // Walk preceding siblings looking for attribute_item ending the line above.
    let mut sib = node.prev_sibling();
    while let Some(s) = sib {
        if s.kind() == "attribute_item" {
            let txt = node_text(s, src);
            if txt.contains("test") {
                return true;
            }
            sib = s.prev_sibling();
            continue;
        }
        if s.kind() == "line_comment" || s.kind() == "block_comment" {
            sib = s.prev_sibling();
            continue;
        }
        break;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(content: &str) -> CodeChunk {
        CodeChunk {
            id: "f.rs:1:10".into(),
            file: "f.rs".into(),
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
    fn rust_analyzer_extracts_function() {
        let a = RustAnalyzer::new();
        let c = make_chunk("fn hello() {}\n");
        let r = a.analyze_chunks(&[c]);
        assert_eq!(r.analyzed_chunks, 1);
        let funcs: Vec<&KgNode> = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Function))
            .collect();
        assert_eq!(funcs.len(), 1, "graph: {:?}", r.graph);
        assert_eq!(funcs[0].name, "hello");
        assert_eq!(funcs[0].language, "rust");
    }

    #[test]
    fn rust_analyzer_extracts_struct_and_trait() {
        let a = RustAnalyzer::new();
        let c = make_chunk(
            "pub struct Foo;\n\
             pub trait Bar {}\n",
        );
        let r = a.analyze_chunks(&[c]);
        let kinds: Vec<&KgNodeKind> = r.graph.nodes.iter().map(|n| &n.kind).collect();
        assert!(kinds.iter().any(|k| matches!(k, KgNodeKind::Class)));
        assert!(kinds.iter().any(|k| matches!(k, KgNodeKind::Interface)));
    }

    #[test]
    fn rust_analyzer_extracts_test_fn() {
        let a = RustAnalyzer::new();
        let c = make_chunk("#[test]\nfn it_works() {}\n");
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::TestCase) && n.name == "it_works"));
    }

    #[test]
    fn supports_dot_rs_files() {
        let a = RustAnalyzer::new();
        assert!(a.supports("src/main.rs"));
        assert!(!a.supports("foo.ts"));
    }
}
