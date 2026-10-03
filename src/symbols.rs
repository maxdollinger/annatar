//! Java type declarations parsed with tree-sitter.
//!
//! [`parse_types`] is the entry point: it parses one source file and returns a
//! record per class, interface, enum, record and annotation type (`@interface`),
//! including nested types. Method and field members, signatures, Javadoc and
//! Spring roles are deliberately not here yet; they arrive in 1.3 and 1.4.
//!
//! Files that do not parse cleanly are logged and skipped: the plan wants a
//! broken file to drop out of a run, not fail it. Anonymous and method-local
//! classes are ignored too (1.3 makes the same rule for members), so only named
//! types that take part in the fqn hierarchy are returned.

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

/// The kind of a named Java type declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Interface,
    Enum,
    Record,
    /// A `@interface` annotation type.
    Annotation,
}

/// A named type declaration in one source file.
///
/// `name` is the simple name; `fqn` is dotted and includes enclosing types
/// (`package.Outer.Inner`); `parent` is the fqn of the immediately enclosing
/// type, or `None` for a top-level type. Line spans are 1-based, byte spans are
/// byte offsets into the source. A file in the default package has an empty
/// `package` and an fqn with no leading dot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDef {
    pub package: String,
    pub name: String,
    pub fqn: String,
    pub kind: TypeKind,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub parent: Option<String>,
}

/// Parse every named type in `source`.
///
/// A file that tree-sitter cannot parse without errors is logged with `path`
/// and yields an empty list; this is a skip, not a failure.
pub fn parse_types(path: &Path, source: &str) -> Result<Vec<TypeDef>> {
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
    let mut types = Vec::new();
    collect_types(root, source, &package, None, &mut types);
    Ok(types)
}

/// Walk a type container (the root `program` or a type body) and record every
/// named type declaration directly inside it, recursing into each type's own
/// body for nested types.
///
/// Only type containers are descended into. Executable scopes (method and
/// constructor bodies, field initializers, lambdas) are not, so method-local
/// classes — which the grammar also calls `class_declaration` — and anonymous
/// classes are never visited.
fn collect_types(
    container: Node<'_>,
    source: &str,
    package: &str,
    parent: Option<&str>,
    out: &mut Vec<TypeDef>,
) {
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if let Some(kind) = type_kind(child.kind()) {
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let name = text(name_node, source);
            let fqn = match parent {
                Some(parent) => format!("{parent}.{name}"),
                None => qualify(package, &name),
            };
            let start = child.start_position();
            let end = child.end_position();
            out.push(TypeDef {
                package: package.to_string(),
                name,
                fqn: fqn.clone(),
                kind,
                start_line: start.row + 1,
                end_line: end.row + 1,
                start_byte: child.start_byte(),
                end_byte: child.end_byte(),
                parent: parent.map(str::to_string),
            });
            if let Some(body) = type_body(child) {
                collect_types(body, source, package, Some(&fqn), out);
            }
        } else if is_type_container(child.kind()) {
            collect_types(child, source, package, parent, out);
        }
    }
}

fn type_kind(node_kind: &str) -> Option<TypeKind> {
    match node_kind {
        "class_declaration" => Some(TypeKind::Class),
        "interface_declaration" => Some(TypeKind::Interface),
        "enum_declaration" => Some(TypeKind::Enum),
        "record_declaration" => Some(TypeKind::Record),
        "annotation_type_declaration" => Some(TypeKind::Annotation),
        _ => None,
    }
}

/// The body node of a type declaration, where its nested types live.
fn type_body(declaration: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .find(|child| is_type_container(child.kind()))
}

/// Node kinds that hold type declarations directly: the root, the bodies of
/// each type kind, and the declarations section of an enum body.
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

    fn parse(source: &str) -> Vec<TypeDef> {
        parse_types(Path::new("Fixture.java"), source).unwrap()
    }

    fn by_name<'a>(types: &'a [TypeDef], name: &str) -> &'a TypeDef {
        types
            .iter()
            .find(|def| def.name == name)
            .unwrap_or_else(|| panic!("no type named {name} in {types:?}"))
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
        let types = parse(source);

        assert_eq!(
            types.iter().map(|def| def.kind).collect::<Vec<_>>(),
            vec![
                TypeKind::Class,
                TypeKind::Interface,
                TypeKind::Enum,
                TypeKind::Record,
                TypeKind::Annotation,
            ]
        );
        assert_eq!(
            types.iter().map(|def| def.fqn.as_str()).collect::<Vec<_>>(),
            vec![
                "com.acme.kinds.Outer",
                "com.acme.kinds.Service",
                "com.acme.kinds.Color",
                "com.acme.kinds.Point",
                "com.acme.kinds.Marker",
            ]
        );
        assert!(types.iter().all(|def| def.package == "com.acme.kinds"));
        assert!(types.iter().all(|def| def.parent.is_none()));
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
        let types = parse(source);

        assert_eq!(
            types.iter().map(|def| def.fqn.as_str()).collect::<Vec<_>>(),
            vec![
                "com.acme.nest.Outer",
                "com.acme.nest.Outer.Inner",
                "com.acme.nest.Outer.Inner.Deep",
            ],
            "only named types in the fqn hierarchy should be returned"
        );

        let outer = by_name(&types, "Outer");
        assert_eq!(outer.parent, None);

        let inner = by_name(&types, "Inner");
        assert_eq!(inner.parent.as_deref(), Some("com.acme.nest.Outer"));

        let deep = by_name(&types, "Deep");
        assert_eq!(deep.kind, TypeKind::Interface);
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
        let types = parse(source);
        let outer = by_name(&types, "Outer");
        assert_eq!((outer.start_line, outer.end_line), (3, 5));
        assert_eq!(
            &source[outer.start_byte..outer.end_byte],
            "class Outer {\n    class Inner {}\n}"
        );

        let inner = by_name(&types, "Inner");
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
        let types = parse(source);
        assert_eq!(types.len(), 2);
        assert_eq!(types[0].fqn, "com.acme.First");
        assert_eq!(types[1].fqn, "com.acme.Second");
    }

    #[test]
    fn default_package_has_no_leading_dot() {
        let source = "\
class NoPackage {
    class Nested {}
}
";
        let types = parse(source);
        let outer = by_name(&types, "NoPackage");
        assert_eq!(outer.package, "");
        assert_eq!(outer.fqn, "NoPackage");
        assert_eq!(by_name(&types, "Nested").fqn, "NoPackage.Nested");
        assert_eq!(
            by_name(&types, "Nested").parent.as_deref(),
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
}
