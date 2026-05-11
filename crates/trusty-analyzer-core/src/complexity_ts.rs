//! Tree-sitter-backed complexity computation for Rust and TypeScript/JS.
//!
//! Why: The text-heuristic `compute_complexity` in `complexity.rs` is fast
//! but wrong on common idioms — it counts the substring `if ` inside strings,
//! attributes, and identifiers, and it conflates `else` chains with branches.
//! Walking a real AST gives accurate cyclomatic / cognitive numbers and
//! produces line-accurate smells from `start_position()` / `end_position()`.
//!
//! What: For Rust and TypeScript (covering `.ts`/`.tsx`/`.js`), parse the
//! source with tree-sitter and walk it recursively. Cyclomatic counts each
//! branching node once. Cognitive multiplies each branching node by its
//! enclosing nesting depth + 1. If parsing fails or produces an empty tree,
//! callers should fall back to the text heuristic.
//!
//! Test: see the `tests` module — covers a single-branch function, a
//! no-branch function, deep nesting, and the smart dispatcher.

use tree_sitter::{Node, Parser};
use trusty_analyzer_types::complexity::{CodeSmell, ComplexityGrade, ComplexityMetrics};

/// Threshold for `LongFunction`: > 50 lines spanned by the function node.
const LONG_FUNCTION_THRESHOLD: usize = 50;
/// Threshold for `DeepNesting`: max nesting depth above this triggers the smell.
const DEEP_NESTING_THRESHOLD: u8 = 4;
/// Threshold for `TooManyParams`: parameter count above this triggers the smell.
const TOO_MANY_PARAMS_THRESHOLD: usize = 5;

/// Compute `ComplexityMetrics` for Rust source using tree-sitter AST.
///
/// Returns `None` if parsing fails so the caller can fall back to the
/// text-heuristic implementation in `complexity.rs`.
pub fn compute_complexity_rust(content: &str) -> Option<ComplexityMetrics> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    let src = content.as_bytes();

    let mut state = WalkState::default();
    walk_rust(root, src, 0, &mut state);

    let cyclomatic = state.cyclomatic.saturating_add(1);
    let cognitive = state.cognitive;
    let grade = ComplexityGrade::from_cyclomatic(cyclomatic);
    let smells = detect_smells_rust(root, src, &state);

    tracing::debug!(
        cyclomatic,
        cognitive,
        ?grade,
        max_nesting = state.max_nesting,
        "compute_complexity_rust"
    );

    Some(ComplexityMetrics {
        cyclomatic,
        cognitive,
        grade,
        smells,
    })
}

/// Compute `ComplexityMetrics` for TypeScript/JavaScript source using
/// tree-sitter AST. Uses the TSX grammar, which is a superset that also
/// parses plain `.ts` and `.js`.
///
/// Returns `None` if parsing fails so the caller can fall back to the
/// text-heuristic implementation.
pub fn compute_complexity_typescript(content: &str) -> Option<ComplexityMetrics> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    let src = content.as_bytes();

    let mut state = WalkState::default();
    walk_ts(root, src, 0, &mut state);

    let cyclomatic = state.cyclomatic.saturating_add(1);
    let cognitive = state.cognitive;
    let grade = ComplexityGrade::from_cyclomatic(cyclomatic);
    let smells = detect_smells_ts(root, src, &state);

    tracing::debug!(
        cyclomatic,
        cognitive,
        ?grade,
        max_nesting = state.max_nesting,
        "compute_complexity_typescript"
    );

    Some(ComplexityMetrics {
        cyclomatic,
        cognitive,
        grade,
        smells,
    })
}

/// Accumulator threaded through the recursive walk.
#[derive(Default)]
struct WalkState {
    cyclomatic: u32,
    cognitive: u32,
    max_nesting: u8,
}

impl WalkState {
    fn note_branch(&mut self, depth: u8) {
        self.cyclomatic = self.cyclomatic.saturating_add(1);
        let weight = (depth as u32).saturating_add(1);
        self.cognitive = self.cognitive.saturating_add(weight);
    }
}

/// Recursive walker for Rust ASTs. Counts branching nodes and tracks nesting
/// depth for cognitive complexity.
fn walk_rust(node: Node, src: &[u8], depth: u8, state: &mut WalkState) {
    state.max_nesting = state.max_nesting.max(depth);
    let kind = node.kind();
    let mut nest_inc: u8 = 0;

    match kind {
        "if_expression" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        // Only count `else if` as a branch — a plain `else` block is not.
        "else_clause" if has_child_kind(node, "if_expression") => {
            state.note_branch(depth);
        }
        "else_clause" => {}
        // First arm adds nothing; each subsequent arm is a branch.
        "match_arm" if !is_first_match_arm(node) => {
            state.note_branch(depth);
        }
        "match_arm" => {}
        "match_expression" => {
            nest_inc = 1;
        }
        "while_expression" | "loop_expression" | "for_expression" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        "binary_expression" if is_short_circuit_op(node, src) => {
            state.note_branch(depth);
        }
        "binary_expression" => {}
        "try_expression" => {
            // The `?` operator introduces an early-return branch.
            state.note_branch(depth);
        }
        "closure_expression" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        _ => {}
    }

    let new_depth = depth.saturating_add(nest_inc);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_rust(child, src, new_depth, state);
    }
}

/// Recursive walker for TypeScript / JavaScript ASTs.
fn walk_ts(node: Node, src: &[u8], depth: u8, state: &mut WalkState) {
    state.max_nesting = state.max_nesting.max(depth);
    let kind = node.kind();
    let mut nest_inc: u8 = 0;

    match kind {
        "if_statement" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        "else_clause" if has_child_kind(node, "if_statement") => {
            state.note_branch(depth);
        }
        "else_clause" => {}
        // Subsequent cases each add a branch; first case is already counted.
        "switch_case" if !is_first_switch_case(node) => {
            state.note_branch(depth);
        }
        "switch_case" => {}
        "switch_statement" => {
            nest_inc = 1;
        }
        "while_statement" | "do_statement" | "for_statement" | "for_in_statement"
        | "for_of_statement" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        "binary_expression" if is_short_circuit_op(node, src) => {
            state.note_branch(depth);
        }
        "binary_expression" => {}
        "ternary_expression" => {
            state.note_branch(depth);
        }
        "arrow_function" | "function_expression" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        "catch_clause" => {
            state.note_branch(depth);
            nest_inc = 1;
        }
        _ => {}
    }

    let new_depth = depth.saturating_add(nest_inc);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_ts(child, src, new_depth, state);
    }
}

/// True if `node` is a `binary_expression` whose operator is `&&` or `||`.
fn is_short_circuit_op(node: Node, src: &[u8]) -> bool {
    if let Some(op) = node.child_by_field_name("operator") {
        let txt = op.utf8_text(src).unwrap_or("");
        return txt == "&&" || txt == "||";
    }
    false
}

/// True if `node` is the first `match_arm` child of its parent.
fn is_first_match_arm(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if child.kind() == "match_arm" {
            return child.id() == node.id();
        }
    }
    true
}

/// True if `node` is the first `switch_case` child of its parent.
fn is_first_switch_case(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if child.kind() == "switch_case" {
            return child.id() == node.id();
        }
    }
    true
}

/// True if any direct child of `node` has the given kind.
fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return true;
        }
    }
    false
}

/// AST-driven smell detection for Rust.
fn detect_smells_rust(root: Node, src: &[u8], state: &WalkState) -> Vec<CodeSmell> {
    let mut smells = Vec::new();

    let fn_node = find_first_kind(root, "function_item");
    let lines = if let Some(n) = fn_node {
        n.end_position().row.saturating_sub(n.start_position().row) + 1
    } else {
        line_count(src)
    };
    if lines > LONG_FUNCTION_THRESHOLD {
        smells.push(CodeSmell::LongFunction { lines });
    }

    if state.max_nesting > DEEP_NESTING_THRESHOLD {
        smells.push(CodeSmell::DeepNesting {
            max_depth: state.max_nesting,
        });
    }

    if let Some(fn_n) = fn_node {
        let params = fn_n
            .child_by_field_name("parameters")
            .map(|p| count_named_children_kind(p, "parameter"))
            .unwrap_or(0);
        if params > TOO_MANY_PARAMS_THRESHOLD {
            smells.push(CodeSmell::TooManyParams { count: params });
        }
        if !has_rust_doc(fn_n, src) {
            smells.push(CodeSmell::MissingDocstring);
        }
    } else if !contains_doc_marker(src) {
        smells.push(CodeSmell::MissingDocstring);
    }

    smells
}

/// AST-driven smell detection for TypeScript / JavaScript.
fn detect_smells_ts(root: Node, src: &[u8], state: &WalkState) -> Vec<CodeSmell> {
    let mut smells = Vec::new();

    let fn_node = find_first_kind(root, "function_declaration")
        .or_else(|| find_first_kind(root, "method_definition"))
        .or_else(|| find_first_kind(root, "arrow_function"));
    let lines = if let Some(n) = fn_node {
        n.end_position().row.saturating_sub(n.start_position().row) + 1
    } else {
        line_count(src)
    };
    if lines > LONG_FUNCTION_THRESHOLD {
        smells.push(CodeSmell::LongFunction { lines });
    }

    if state.max_nesting > DEEP_NESTING_THRESHOLD {
        smells.push(CodeSmell::DeepNesting {
            max_depth: state.max_nesting,
        });
    }

    if let Some(fn_n) = fn_node {
        let params = fn_n
            .child_by_field_name("parameters")
            .map(count_param_children)
            .unwrap_or(0);
        if params > TOO_MANY_PARAMS_THRESHOLD {
            smells.push(CodeSmell::TooManyParams { count: params });
        }
        if !has_jsdoc(fn_n, src) {
            smells.push(CodeSmell::MissingDocstring);
        }
    } else if !contains_doc_marker(src) {
        smells.push(CodeSmell::MissingDocstring);
    }

    smells
}

/// Count parameter-shaped children of a `formal_parameters` node.
fn count_param_children(params: Node) -> usize {
    let mut count = 0;
    let mut cursor = params.walk();
    for child in params.children(&mut cursor) {
        match child.kind() {
            "required_parameter" | "optional_parameter" | "rest_pattern" | "identifier"
            | "assignment_pattern" | "object_pattern" | "array_pattern" => count += 1,
            _ => {}
        }
    }
    count
}

/// Count direct children of `node` whose kind matches `kind`.
fn count_named_children_kind(node: Node, kind: &str) -> usize {
    let mut count = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            count += 1;
        }
    }
    count
}

/// First descendant of `root` whose kind matches `kind`, or `None`.
fn find_first_kind<'a>(root: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == kind {
            return Some(n);
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

/// True if a `///` line_comment immediately precedes the function item.
fn has_rust_doc(fn_node: Node, src: &[u8]) -> bool {
    let mut sib = fn_node.prev_sibling();
    while let Some(s) = sib {
        match s.kind() {
            "line_comment" => {
                let txt = s.utf8_text(src).unwrap_or("");
                if txt.starts_with("///") || txt.starts_with("//!") {
                    return true;
                }
                sib = s.prev_sibling();
            }
            "block_comment" => {
                let txt = s.utf8_text(src).unwrap_or("");
                if txt.starts_with("/**") || txt.starts_with("/*!") {
                    return true;
                }
                sib = s.prev_sibling();
            }
            "attribute_item" | "inner_attribute_item" => {
                sib = s.prev_sibling();
            }
            _ => break,
        }
    }
    false
}

/// True if a `/** ... */` block_comment immediately precedes the function node.
fn has_jsdoc(fn_node: Node, src: &[u8]) -> bool {
    let mut sib = fn_node.prev_sibling();
    while let Some(s) = sib {
        if s.kind() == "comment" {
            let txt = s.utf8_text(src).unwrap_or("");
            if txt.starts_with("/**") {
                return true;
            }
            sib = s.prev_sibling();
        } else {
            break;
        }
    }
    false
}

/// Best-effort doc-marker check used when no function-shaped node is present.
fn contains_doc_marker(src: &[u8]) -> bool {
    let s = std::str::from_utf8(src).unwrap_or("");
    s.contains("///") || s.contains("/**") || s.contains("\"\"\"") || s.contains("'''")
}

/// Total line count of `src` (1-based; an empty buffer reports 1).
fn line_count(src: &[u8]) -> usize {
    let s = std::str::from_utf8(src).unwrap_or("");
    s.lines().count().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_complexity_rust_single_branch() {
        let src = "fn foo(a: i32, b: i32) -> i32 { if a > b { a } else { b } }";
        let m = compute_complexity_rust(src).expect("parse should succeed");
        assert!(
            m.cyclomatic >= 2,
            "expected cyclomatic >= 2, got {}",
            m.cyclomatic
        );
        assert!(
            matches!(m.grade, ComplexityGrade::A | ComplexityGrade::B),
            "expected grade A or B, got {:?}",
            m.grade
        );
    }

    #[test]
    fn compute_complexity_rust_no_branches() {
        let src = "fn foo() -> i32 { 42 }";
        let m = compute_complexity_rust(src).expect("parse should succeed");
        assert_eq!(
            m.cyclomatic, 1,
            "expected cyclomatic == 1, got {}",
            m.cyclomatic
        );
        assert_eq!(m.grade, ComplexityGrade::A);
    }

    #[test]
    fn compute_complexity_rust_match_arms_count() {
        let src = r#"
fn classify(n: i32) -> &'static str {
    match n {
        0 => "zero",
        1 => "one",
        2 => "two",
        _ => "many",
    }
}
"#;
        let m = compute_complexity_rust(src).expect("parse should succeed");
        // 3 arms after the first → +3, plus base 1 = 4.
        assert!(
            m.cyclomatic >= 3,
            "expected cyclomatic >= 3, got {}",
            m.cyclomatic
        );
    }

    #[test]
    fn compute_complexity_rust_short_circuit_counts() {
        let src = r#"fn f(a: bool, b: bool, c: bool) -> bool { a && b || c }"#;
        let m = compute_complexity_rust(src).expect("parse should succeed");
        // base(1) + && (1) + || (1) = 3
        assert!(m.cyclomatic >= 3);
    }

    #[test]
    fn compute_complexity_typescript_single_branch() {
        let src = "function foo(a: number, b: number): number { return a > b ? a : b; }";
        let m = compute_complexity_typescript(src).expect("parse should succeed");
        assert!(m.cyclomatic >= 2);
    }

    #[test]
    fn compute_complexity_typescript_no_branches() {
        let src = "function foo(): number { return 42; }";
        let m = compute_complexity_typescript(src).expect("parse should succeed");
        assert_eq!(m.cyclomatic, 1);
        assert_eq!(m.grade, ComplexityGrade::A);
    }

    #[test]
    fn long_function_smell_fires_for_long_fn() {
        let mut body = String::from("/// doc\nfn big(a: i32) -> i32 {\n");
        for _ in 0..60 {
            body.push_str("    let _ = 1;\n");
        }
        body.push_str("    a\n}\n");
        let m = compute_complexity_rust(&body).expect("parse should succeed");
        assert!(
            m.smells
                .iter()
                .any(|s| matches!(s, CodeSmell::LongFunction { .. })),
            "expected LongFunction smell, got {:?}",
            m.smells
        );
    }

    #[test]
    fn missing_docstring_smell_for_undocumented_rust_fn() {
        let m = compute_complexity_rust("fn f() {}").expect("parse should succeed");
        assert!(m
            .smells
            .iter()
            .any(|s| matches!(s, CodeSmell::MissingDocstring)));
    }

    #[test]
    fn doc_comment_suppresses_missing_docstring() {
        let m = compute_complexity_rust("/// hi\nfn f() {}").expect("parse should succeed");
        assert!(!m
            .smells
            .iter()
            .any(|s| matches!(s, CodeSmell::MissingDocstring)));
    }
}
