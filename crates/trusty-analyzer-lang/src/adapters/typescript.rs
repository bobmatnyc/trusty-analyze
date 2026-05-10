//! TypeScript `LanguageAnalyzer` adapter backed by tree-sitter-typescript.
//!
//! Why: Extracts functions, classes, interfaces, imports/exports and call
//! expressions from TypeScript and TSX source.
//!
//! What: For each chunk, parses with `tree_sitter_typescript::LANGUAGE_TSX`
//! (which is a superset that also accepts plain `.ts`), walks the tree
//! once, and emits nodes/edges into a shared `KgGraph`.
//!
//! Test: `ts_analyzer_extracts_function` parses `function hello() {}` and
//! asserts the Function node is produced.

use tree_sitter::{Node, Parser};
use trusty_common::{CodeChunk, KgEdge, KgEdgeKind, KgGraph, KgNode, KgNodeKind};

use crate::lang::{LanguageAnalyzer, StaticAnalysisResult};

/// tree-sitter-typescript-backed analyzer (also handles TSX).
pub struct TypeScriptAnalyzer;

impl TypeScriptAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TypeScriptAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageAnalyzer for TypeScriptAnalyzer {
    fn language(&self) -> &str {
        "typescript"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".ts", ".tsx"]
    }

    fn analyze_chunks(&self, chunks: &[CodeChunk]) -> StaticAnalysisResult {
        analyze_with_grammar(chunks, "typescript", true)
    }
}

/// Shared implementation: parse with TS or JS grammar, walk, emit.
pub(crate) fn analyze_with_grammar(
    chunks: &[CodeChunk],
    language_tag: &str,
    is_typescript: bool,
) -> StaticAnalysisResult {
    let mut parser = Parser::new();
    let lang = if is_typescript {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    if parser.set_language(&lang).is_err() {
        return StaticAnalysisResult {
            errors: vec![format!("failed to load grammar for {language_tag}")],
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
            result
                .graph
                .nodes
                .push(file_node(&chunk.file, language_tag));
        }

        walk_ts_like(
            tree.root_node(),
            chunk.content.as_bytes(),
            chunk,
            language_tag,
            &mut result.graph,
        );
    }

    result
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

fn name_of(node: Node, src: &[u8]) -> Option<String> {
    node.child_by_field_name("name").map(|n| node_text(n, src))
}

fn make_node(kind: KgNodeKind, name: &str, chunk: &CodeChunk, ast: Node, language: &str) -> KgNode {
    let start = (chunk.start_line as u32).saturating_add(ast.start_position().row as u32);
    let end = (chunk.start_line as u32).saturating_add(ast.end_position().row as u32);
    let kind_str = format!("{kind:?}");
    KgNode {
        id: format!("{language}:{kind_str}:{}:{name}", chunk.file),
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        language: language.to_string(),
        file: chunk.file.clone(),
        start_line: start,
        end_line: end,
        doc_comment: None,
        is_public: false,
        extra: serde_json::Value::Null,
    }
}

fn walk_ts_like(node: Node, src: &[u8], chunk: &CodeChunk, language: &str, graph: &mut KgGraph) {
    let file_id = format!("{language}:File:{}", chunk.file);

    fn recurse(
        node: Node,
        src: &[u8],
        chunk: &CodeChunk,
        language: &str,
        graph: &mut KgGraph,
        file_id: &str,
    ) {
        match node.kind() {
            "function_declaration" | "function" => {
                if let Some(name) = name_of(node, src) {
                    let n = make_node(KgNodeKind::Function, &name, chunk, node, language);
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
            "method_definition" => {
                if let Some(name) = name_of(node, src) {
                    let n = make_node(KgNodeKind::Method, &name, chunk, node, language);
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
            "class_declaration" => {
                if let Some(name) = name_of(node, src) {
                    let n = make_node(KgNodeKind::Class, &name, chunk, node, language);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id.clone(),
                        kind: KgEdgeKind::Contains,
                        weight: 1.0,
                    });
                    // extends / implements
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.kind() != "class_heritage" {
                            continue;
                        }
                        let mut c2 = child.walk();
                        for h in child.children(&mut c2) {
                            match h.kind() {
                                "extends_clause" => {
                                    let mut inner_cursor = h.walk();
                                    for inner in h.children(&mut inner_cursor) {
                                        if inner.kind() == "identifier"
                                            || inner.kind() == "type_identifier"
                                        {
                                            let target = node_text(inner, src);
                                            let to_id =
                                                format!("{language}:Class:{}:{target}", chunk.file);
                                            graph.edges.push(KgEdge {
                                                from: id.clone(),
                                                to: to_id,
                                                kind: KgEdgeKind::Extends,
                                                weight: 1.0,
                                            });
                                        }
                                    }
                                }
                                "implements_clause" => {
                                    let mut inner_cursor = h.walk();
                                    for inner in h.children(&mut inner_cursor) {
                                        if inner.kind() == "type_identifier"
                                            || inner.kind() == "identifier"
                                        {
                                            let target = node_text(inner, src);
                                            let to_id = format!(
                                                "{language}:Interface:{}:{target}",
                                                chunk.file
                                            );
                                            graph.edges.push(KgEdge {
                                                from: id.clone(),
                                                to: to_id,
                                                kind: KgEdgeKind::Implements,
                                                weight: 1.0,
                                            });
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name) = name_of(node, src) {
                    let n = make_node(KgNodeKind::Interface, &name, chunk, node, language);
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
            "import_statement" => {
                let txt = node_text(node, src);
                let cleaned = txt.trim().trim_end_matches(';').to_string();
                if !cleaned.is_empty() {
                    let n = make_node(KgNodeKind::Import, &cleaned, chunk, node, language);
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
            "export_statement" => {
                let txt = node_text(node, src);
                let cleaned = txt.trim().trim_end_matches(';').to_string();
                if !cleaned.is_empty() {
                    let n = make_node(KgNodeKind::Export, &cleaned, chunk, node, language);
                    let id = n.id.clone();
                    graph.nodes.push(n);
                    graph.edges.push(KgEdge {
                        from: file_id.to_string(),
                        to: id,
                        kind: KgEdgeKind::Exports,
                        weight: 1.0,
                    });
                }
            }
            "call_expression" => {
                // Try to extract the callee identifier.
                if let Some(fun) = node.child_by_field_name("function") {
                    let name = node_text(fun, src);
                    if !name.is_empty() {
                        let n = make_node(KgNodeKind::CallExpression, &name, chunk, node, language);
                        let id = n.id.clone();
                        let to_id = format!("{language}:Function:{}:{name}", chunk.file);
                        graph.nodes.push(n);
                        graph.edges.push(KgEdge {
                            from: id,
                            to: to_id,
                            kind: KgEdgeKind::Calls,
                            weight: 1.0,
                        });
                    }
                }
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            recurse(child, src, chunk, language, graph, file_id);
        }
    }

    recurse(node, src, chunk, language, graph, &file_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(content: &str, file: &str) -> CodeChunk {
        CodeChunk {
            id: format!("{file}:1:10"),
            file: file.into(),
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
    fn ts_analyzer_extracts_function() {
        let a = TypeScriptAnalyzer::new();
        let c = make_chunk("function hello() { return 1; }\n", "f.ts");
        let r = a.analyze_chunks(&[c]);
        assert_eq!(r.analyzed_chunks, 1);
        let funcs: Vec<&KgNode> = r
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, KgNodeKind::Function))
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "hello");
        assert_eq!(funcs[0].language, "typescript");
    }

    #[test]
    fn ts_analyzer_extracts_class_and_interface() {
        let a = TypeScriptAnalyzer::new();
        let c = make_chunk(
            "interface Foo { x: number }\n\
             class Bar implements Foo { x = 1; }\n",
            "f.ts",
        );
        let r = a.analyze_chunks(&[c]);
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::Class) && n.name == "Bar"));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| matches!(n.kind, KgNodeKind::Interface) && n.name == "Foo"));
        assert!(r
            .graph
            .edges
            .iter()
            .any(|e| matches!(e.kind, KgEdgeKind::Implements)));
    }

    #[test]
    fn supports_dot_ts_and_tsx() {
        let a = TypeScriptAnalyzer::new();
        assert!(a.supports("App.tsx"));
        assert!(a.supports("foo.ts"));
        assert!(!a.supports("foo.js"));
    }
}
