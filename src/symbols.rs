//! Java symbols parsed with tree-sitter.
//!
//! [`parse_file`] is the entry point: it parses one source file and returns a
//! record per named type (class, interface, enum, record and `@interface`) plus
//! every method and constructor, in source order. Signatures, Javadoc,
//! annotations and Spring roles are deliberately not here yet; they arrive in
//! 1.4.
//!
//! Files that do not parse cleanly are logged and skipped: the plan wants a
//! broken file to drop out of a run, not fail it. Anonymous and method-local
//! classes are ignored too: recursion only descends into type containers, so
//! executable scopes (method bodies, field initializers, lambdas) contribute
//! nothing.

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

/// The kind of a named Java symbol: a type or one of its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Interface,
    Enum,
    Record,
    /// A `@interface` annotation type.
    Annotation,
    Method,
    Constructor,
}

/// A named symbol in one source file: a type or a method/constructor.
///
/// `name` is the simple name (a constructor uses `<init>`); `fqn` is dotted for
/// types and `parent#name(params)` for members. `parent` is the fqn of the
/// immediately enclosing type, or `None` for a top-level type. Line spans are
/// 1-based, byte spans are offsets into the source. A file in the default
/// package has an empty `package` and an fqn with no leading dot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub package: String,
    pub name: String,
    pub fqn: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub parent: Option<String>,
}

/// Parse every named type and member in `source`.
///
/// A file that tree-sitter cannot parse without errors is logged with `path`
/// and yields an empty list; this is a skip, not a failure.
pub fn parse_file(path: &Path, source: &str) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .context("loading the tree-sitter-java grammar")?;

    let Some(tree) = parser.parse(source, None) else {
        tracing::warn!(path = %path.display(), "tree-sitter produced no tree; skipping file");
        return Ok(Vec::new());
    };
    let root = tree.root_node();
    if root.has_error() {
        tracing::warn!(path = %path.display(), "parse errors; skipping file");
        return Ok(Vec::new());
    }

    let package = package_name(root, source);
    let mut symbols = Vec::new();
    collect_symbols(root, source, &package, None, &mut symbols);
    Ok(symbols)
}

/// Walk a type container (the root `program` or a type body) and record every
/// named type and member declaration directly inside it, recursing into each
/// type's own body for nested types and members.
///
/// Only type containers are descended into. Executable scopes (method and
/// constructor bodies, field initializers, lambdas) are not, so method-local
/// classes — which the grammar also calls `class_declaration` — and anonymous
/// classes are never visited. `enclosing` is the fqn of the type whose body is
/// being walked; it is the parent of a nested type and of every member.
fn collect_symbols(
    container: Node<'_>,
    source: &str,
    package: &str,
    enclosing: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if let Some(kind) = type_kind(child.kind()) {
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let name = text(name_node, source);
            let fqn = match enclosing {
                Some(parent) => format!("{parent}.{name}"),
                None => qualify(package, &name),
            };
            let start = child.start_position();
            let end = child.end_position();
            out.push(Symbol {
                package: package.to_string(),
                name,
                fqn: fqn.clone(),
                kind,
                start_line: start.row + 1,
                end_line: end.row + 1,
                start_byte: child.start_byte(),
                end_byte: child.end_byte(),
                parent: enclosing.map(str::to_string),
            });
            if let Some(body) = type_body(child) {
                collect_symbols(body, source, package, Some(&fqn), out);
            }
        } else if let Some(symbol) = member_symbol(child, source, package, enclosing) {
            out.push(symbol);
        } else if is_type_container(child.kind()) {
            collect_symbols(child, source, package, enclosing, out);
        }
    }
}

fn type_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        "record_declaration" => Some(SymbolKind::Record),
        "annotation_type_declaration" => Some(SymbolKind::Annotation),
        _ => None,
    }
}

fn member_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "method_declaration" => Some(SymbolKind::Method),
        "constructor_declaration" | "compact_constructor_declaration" => {
            Some(SymbolKind::Constructor)
        }
        _ => None,
    }
}

/// Build the symbol for a method or constructor. Returns `None` for any node
/// that is not a member declaration, or when there is no enclosing type (a
/// member can only live in a type body, so this should not happen).
fn member_symbol(
    node: Node<'_>,
    source: &str,
    package: &str,
    enclosing: Option<&str>,
) -> Option<Symbol> {
    let kind = member_kind(node.kind())?;
    let parent = enclosing?;
    let name = match kind {
        SymbolKind::Constructor => "<init>".to_string(),
        _ => text(node.child_by_field_name("name")?, source),
    };
    let params = node
        .child_by_field_name("parameters")
        .map(|parameters| render_params(parameters, source))
        .unwrap_or_default();
    let start = node.start_position();
    let end = node.end_position();
    Some(Symbol {
        package: package.to_string(),
        name: name.clone(),
        fqn: format!("{parent}#{name}({params})"),
        kind,
        start_line: start.row + 1,
        end_line: end.row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        parent: Some(parent.to_string()),
    })
}

/// Render a `formal_parameters` node as comma-joined parameter types without
/// names, e.g. `Long,java.util.List<String>`.
fn render_params(parameters: Node<'_>, source: &str) -> String {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter(|param| {
            matches!(
                param.kind(),
                "formal_parameter" | "spread_parameter" | "receiver_parameter"
            )
        })
        .map(|param| render_param(param, source))
        .collect::<Vec<_>>()
        .join(",")
}

/// Render one parameter's type as written, whitespace-normalized. Arrays keep
/// their brackets and a vararg keeps its `...`.
fn render_param(param: Node<'_>, source: &str) -> String {
    if param.kind() == "spread_parameter" {
        return match first_type_child(param) {
            Some(ty) => format!("{}...", normalize(&text(ty, source))),
            None => fallback_param_type(param, source),
        };
    }
    let Some(ty) = param
        .child_by_field_name("type")
        .or_else(|| first_type_child(param))
    else {
        return fallback_param_type(param, source);
    };
    let mut rendered = normalize(&text(ty, source));
    if let Some(dimensions) = param.child_by_field_name("dimensions") {
        rendered.push_str(&normalize(&text(dimensions, source)));
    }
    rendered
}

/// Best-effort type when the grammar gives no type field: normalize the whole
/// parameter and strip its name, so overloads still differ.
fn fallback_param_type(param: Node<'_>, source: &str) -> String {
    let full = normalize(&text(param, source));
    let name = param
        .child_by_field_name("name")
        .map(|node| text(node, source))
        .or_else(|| {
            let mut cursor = param.walk();
            param
                .named_children(&mut cursor)
                .find(|node| node.kind() == "variable_declarator")
                .and_then(|node| node.child_by_field_name("name"))
                .map(|node| text(node, source))
        });
    match name {
        Some(name) if full.ends_with(&name) => full[..full.len() - name.len()].to_string(),
        _ => full,
    }
}

/// The first named child that is a type, used where the grammar exposes no
/// `type` field (varargs and receiver parameters).
fn first_type_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| is_type_node(child.kind()))
}

fn is_type_node(node_kind: &str) -> bool {
    matches!(
        node_kind,
        "type_identifier"
            | "scoped_type_identifier"
            | "generic_type"
            | "array_type"
            | "integral_type"
            | "floating_point_type"
            | "boolean_type"
    )
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The body node of a type declaration, where its nested types and members live.
fn type_body(declaration: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .find(|child| is_type_container(child.kind()))
}

/// Node kinds that hold type and member declarations directly: the root, the
/// bodies of each type kind, and the declarations section of an enum body.
fn is_type_container(node_kind: &str) -> bool {
    matches!(
        node_kind,
        "program"
            | "class_body"
            | "interface_body"
            | "enum_body"
            | "enum_body_declarations"
            | "annotation_type_body"
    )
}

/// The declared package, or an empty string for the default package.
fn package_name(root: Node<'_>, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "package_declaration" {
            continue;
        }
        let mut inner = child.walk();
        for part in child.named_children(&mut inner) {
            if matches!(part.kind(), "scoped_identifier" | "identifier") {
                return text(part, source);
            }
        }
    }
    String::new()
}

fn qualify(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_string()
    } else {
        format!("{package}.{name}")
    }
}

fn text(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Vec<Symbol> {
        parse_file(Path::new("Fixture.java"), source).unwrap()
    }

    fn by_name<'a>(symbols: &'a [Symbol], name: &str) -> &'a Symbol {
        symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name} in {symbols:?}"))
    }

    #[test]
    fn parses_each_type_kind() {
        let source = "\
package com.acme.kinds;

class Outer {}
interface Service {}
enum Color { RED }
record Point(int x, int y) {}
@interface Marker {}
";
        let symbols = parse(source);

        assert_eq!(
            symbols.iter().map(|symbol| symbol.kind).collect::<Vec<_>>(),
            vec![
                SymbolKind::Class,
                SymbolKind::Interface,
                SymbolKind::Enum,
                SymbolKind::Record,
                SymbolKind::Annotation,
            ]
        );
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.fqn.as_str())
                .collect::<Vec<_>>(),
            vec![
                "com.acme.kinds.Outer",
                "com.acme.kinds.Service",
                "com.acme.kinds.Color",
                "com.acme.kinds.Point",
                "com.acme.kinds.Marker",
            ]
        );
        assert!(
            symbols
                .iter()
                .all(|symbol| symbol.package == "com.acme.kinds")
        );
        assert!(symbols.iter().all(|symbol| symbol.parent.is_none()));
    }

    #[test]
    fn nests_types_and_ignores_local_and_anonymous_classes() {
        let source = "\
package com.acme.nest;

class Outer {
    class Inner {
        interface Deep {}
    }
    void run() {
        class Local {}
        Runnable r = new Runnable() {
            public void run() {}
        };
    }
}
";
        let symbols = parse(source);

        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.fqn.as_str())
                .collect::<Vec<_>>(),
            vec![
                "com.acme.nest.Outer",
                "com.acme.nest.Outer.Inner",
                "com.acme.nest.Outer.Inner.Deep",
                "com.acme.nest.Outer#run()",
            ],
            "only named types in the fqn hierarchy and members of named types should be returned"
        );

        let outer = by_name(&symbols, "Outer");
        assert_eq!(outer.parent, None);

        let inner = by_name(&symbols, "Inner");
        assert_eq!(inner.parent.as_deref(), Some("com.acme.nest.Outer"));

        let deep = by_name(&symbols, "Deep");
        assert_eq!(deep.kind, SymbolKind::Interface);
        assert_eq!(deep.parent.as_deref(), Some("com.acme.nest.Outer.Inner"));
    }

    #[test]
    fn spans_are_one_based_lines_and_byte_offsets_into_the_source() {
        let source = "\
package com.acme.spans;

class Outer {
    class Inner {}
}
";
        let symbols = parse(source);
        let outer = by_name(&symbols, "Outer");
        assert_eq!((outer.start_line, outer.end_line), (3, 5));
        assert_eq!(
            &source[outer.start_byte..outer.end_byte],
            "class Outer {\n    class Inner {}\n}"
        );

        let inner = by_name(&symbols, "Inner");
        assert_eq!((inner.start_line, inner.end_line), (4, 4));
        assert_eq!(&source[inner.start_byte..inner.end_byte], "class Inner {}");
    }

    #[test]
    fn handles_multiple_top_level_types() {
        let source = "\
package com.acme;

class First {}
class Second {}
";
        let symbols = parse(source);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].fqn, "com.acme.First");
        assert_eq!(symbols[1].fqn, "com.acme.Second");
    }

    #[test]
    fn default_package_has_no_leading_dot() {
        let source = "\
class NoPackage {
    class Nested {}
}
";
        let symbols = parse(source);
        let outer = by_name(&symbols, "NoPackage");
        assert_eq!(outer.package, "");
        assert_eq!(outer.fqn, "NoPackage");
        assert_eq!(by_name(&symbols, "Nested").fqn, "NoPackage.Nested");
        assert_eq!(
            by_name(&symbols, "Nested").parent.as_deref(),
            Some("NoPackage")
        );
    }

    #[test]
    fn broken_file_returns_empty_and_does_not_panic() {
        let source = "\
package com.acme.broken;

class Broken {
    void run() {
";
        assert!(parse(source).is_empty());
    }

    #[test]
    fn overloads_get_distinct_fqns() {
        let source = "\
package com.acme.over;

class Repo {
    void find(Long id) {}
    void find(String name) {}
    void find() {}
    void array(String[] values) {}
    void varargs(String... values) {}
    void legacy(int values[]) {}
    <T> T generic(java.util.List<T> values) { return null; }
    Object raw(java.util.List values) { return null; }
}
";
        let symbols = parse(source);
        let fqns: Vec<&str> = symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Method)
            .map(|symbol| symbol.fqn.as_str())
            .collect();

        assert_eq!(
            fqns,
            vec![
                "com.acme.over.Repo#find(Long)",
                "com.acme.over.Repo#find(String)",
                "com.acme.over.Repo#find()",
                "com.acme.over.Repo#array(String[])",
                "com.acme.over.Repo#varargs(String...)",
                "com.acme.over.Repo#legacy(int[])",
                "com.acme.over.Repo#generic(java.util.List<T>)",
                "com.acme.over.Repo#raw(java.util.List)",
            ]
        );
        assert_ne!(fqns[0], fqns[1], "differing parameter types");
        assert_ne!(fqns[6], fqns[7], "generic vs raw");
    }

    #[test]
    fn constructors_use_init_and_are_included() {
        let source = "\
package com.acme.ctor;

class User {
    User() {}
    User(String name, int age) {}
}

record Point(int x, int y) {
    Point {
        if (x < 0) { throw new IllegalArgumentException(); }
    }
}
";
        let symbols = parse(source);

        let constructors: Vec<&Symbol> = symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Constructor)
            .collect();
        assert_eq!(
            constructors.len(),
            3,
            "two User constructors and one Point compact constructor"
        );
        assert!(constructors.iter().all(|symbol| symbol.name == "<init>"));

        let user = by_name(&symbols, "User");
        let no_args = symbols
            .iter()
            .find(|symbol| symbol.fqn == "com.acme.ctor.User#<init>()")
            .expect("User() constructor");
        assert_eq!(no_args.kind, SymbolKind::Constructor);
        assert_eq!(no_args.parent.as_deref(), Some(user.fqn.as_str()));

        let two_args = symbols
            .iter()
            .find(|symbol| symbol.fqn == "com.acme.ctor.User#<init>(String,int)")
            .expect("User(String,int) constructor");
        assert_eq!(two_args.parent.as_deref(), Some("com.acme.ctor.User"));

        let point = symbols
            .iter()
            .find(|symbol| symbol.fqn == "com.acme.ctor.Point#<init>()")
            .expect("Point compact constructor");
        assert_eq!(point.kind, SymbolKind::Constructor);
        assert_eq!(point.parent.as_deref(), Some("com.acme.ctor.Point"));
    }

    #[test]
    fn nested_type_members_carry_the_nested_parent_fqn() {
        let source = "\
package com.acme.nest2;

class Outer {
    class Inner {
        void ping() {}
    }
    void pong() {}
}
";
        let symbols = parse(source);

        let ping = by_name(&symbols, "ping");
        assert_eq!(ping.fqn, "com.acme.nest2.Outer.Inner#ping()");
        assert_eq!(ping.parent.as_deref(), Some("com.acme.nest2.Outer.Inner"));

        let pong = by_name(&symbols, "pong");
        assert_eq!(pong.fqn, "com.acme.nest2.Outer#pong()");
        assert_eq!(pong.parent.as_deref(), Some("com.acme.nest2.Outer"));
    }

    #[test]
    fn type_without_members_yields_only_its_symbol() {
        let source = "\
package com.acme.empty;

class Empty {}
";
        let symbols = parse(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].fqn, "com.acme.empty.Empty");
        assert_eq!(symbols[0].kind, SymbolKind::Class);
    }
}
