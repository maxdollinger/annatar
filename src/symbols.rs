//! Java symbols parsed with tree-sitter.
//!
//! [`JavaParser`] is the reusable entry point: construct it once (grammar
//! loading is fallible and expensive) and call [`JavaParser::parse`] per file.
//! It returns a [`ParsedFile`] with a record per named type (class, interface,
//! enum, record and `@interface`) plus every method and constructor, in source
//! order. Each carries the declaration text the LLM will read — signature (with
//! the body removed), Javadoc and annotations — and each type carries a Spring
//! [`Role`] derived from those annotations.
//!
//! Files that do not parse cleanly are logged and marked `parse_error`, with no
//! symbols; a clean file with no symbols (`package-info.java`) is not an error.
//! The plan wants a broken file to drop out of a run, not fail it. Anonymous and method-local
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

impl SymbolKind {
    /// The stable lowercase text stored in `symbols.kind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Enum => "enum",
            SymbolKind::Record => "record",
            SymbolKind::Annotation => "annotation",
            SymbolKind::Method => "method",
            SymbolKind::Constructor => "constructor",
        }
    }

    /// Whether this kind is a type (as opposed to a method or constructor).
    pub fn is_type(self) -> bool {
        matches!(
            self,
            SymbolKind::Class
                | SymbolKind::Interface
                | SymbolKind::Enum
                | SymbolKind::Record
                | SymbolKind::Annotation
        )
    }
}

/// The Spring role of a type, derived from its annotations (and, for
/// repositories, from its superinterfaces). Members have no role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Controller,
    Service,
    Repository,
    Component,
    Configuration,
    Entity,
}

impl Role {
    /// The stable lowercase text stored in `symbols.role`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Controller => "controller",
            Role::Service => "service",
            Role::Repository => "repository",
            Role::Component => "component",
            Role::Configuration => "configuration",
            Role::Entity => "entity",
        }
    }
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
    /// The declaration without its body, annotations removed, whitespace
    /// collapsed, e.g. `public User findById(Long id)`.
    pub signature: String,
    /// The `/** ... */` doc comment immediately above the declaration, cleaned.
    pub javadoc: Option<String>,
    /// Annotation source text on the declaration, as written, in source order.
    pub annotations: Vec<String>,
    /// The Spring role, for types only.
    pub role: Option<Role>,
}

/// A reusable tree-sitter Java parser.
///
/// Construct one per run (or per thread) and reuse it across files: loading the
/// grammar is fallible and expensive, so it happens once in [`JavaParser::new`].
pub struct JavaParser {
    parser: Parser,
}

impl JavaParser {
    /// Build a Java parser, loading the tree-sitter-java grammar once.
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .context("loading the tree-sitter-java grammar")?;
        Ok(Self { parser })
    }

    /// Parse every named type and member in `source`.
    ///
    /// A file that tree-sitter cannot parse without errors is logged with `path`
    /// and yields no symbols with `parse_error: true`; this is a skip, not a
    /// failure. A clean file with no symbols (`package-info.java`) has
    /// `parse_error: false`.
    pub fn parse(&mut self, path: &Path, source: &str) -> Result<ParsedFile> {
        let Some(tree) = self.parser.parse(source, None) else {
            tracing::warn!(path = %path.display(), "tree-sitter produced no tree; skipping file");
            return Ok(ParsedFile {
                symbols: Vec::new(),
                parse_error: true,
            });
        };
        let root = tree.root_node();
        if root.has_error() {
            tracing::warn!(path = %path.display(), "parse errors; skipping file");
            return Ok(ParsedFile {
                symbols: Vec::new(),
                parse_error: true,
            });
        }

        let package = package_name(root, source);
        let mut symbols = Vec::new();
        collect_symbols(root, source, &package, None, &mut symbols);
        Ok(ParsedFile {
            symbols,
            parse_error: false,
        })
    }
}

/// The result of parsing one source file.
#[derive(Debug)]
pub struct ParsedFile {
    /// The symbols found, in source order. Empty when `parse_error` is `true`.
    pub symbols: Vec<Symbol>,
    /// Whether tree-sitter reported errors or produced no tree.
    pub parse_error: bool,
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
            out.push(build_symbol(
                package,
                name,
                fqn.clone(),
                kind,
                enclosing.map(str::to_string),
                child,
                source,
            ));
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
    let fqn = format!("{parent}#{name}({params})");
    Some(build_symbol(
        package,
        name,
        fqn,
        kind,
        Some(parent.to_string()),
        node,
        source,
    ))
}

/// Build a [`Symbol`] from a declaration node, whether a type or a member.
///
/// The declaration's annotation nodes are walked once; the annotation texts,
/// the signature and the role are all derived from that single list.
fn build_symbol(
    package: &str,
    name: String,
    fqn: String,
    kind: SymbolKind,
    parent: Option<String>,
    declaration: Node<'_>,
    source: &str,
) -> Symbol {
    let annotation_nodes = annotation_nodes(declaration);
    let annotations: Vec<String> = annotation_nodes
        .iter()
        .map(|annotation| text(*annotation, source).trim().to_string())
        .collect();
    let annotation_names: Vec<&str> = annotations
        .iter()
        .map(|annotation| annotation_simple_name(annotation))
        .collect();
    let start = declaration.start_position();
    let end = declaration.end_position();
    let role = role_of(kind, &annotation_names, declaration, source);
    Symbol {
        package: package.to_string(),
        name,
        fqn,
        kind,
        start_line: start.row + 1,
        end_line: end.row + 1,
        start_byte: declaration.start_byte(),
        end_byte: declaration.end_byte(),
        parent,
        signature: signature_of(declaration, source, &annotation_nodes),
        javadoc: javadoc_of(declaration, source),
        annotations,
        role,
    }
}

/// The declaration text with the body removed, leading annotations stripped,
/// whitespace collapsed and a trailing `;` dropped. `annotations` is the
/// declaration's precomputed annotation-node list.
fn signature_of(declaration: Node<'_>, source: &str, annotations: &[Node<'_>]) -> String {
    let start = declaration.start_byte();
    let end = body_node(declaration)
        .map(|body| body.start_byte())
        .unwrap_or_else(|| declaration.end_byte());

    let mut header = String::new();
    let mut cursor = start;
    for annotation in annotations {
        let annotation_start = annotation.start_byte();
        let annotation_end = annotation.end_byte().min(end);
        if annotation_start < cursor || annotation_start >= end {
            continue;
        }
        header.push_str(&source[cursor..annotation_start]);
        cursor = annotation_end;
    }
    if cursor < end {
        header.push_str(&source[cursor..end]);
    }

    let collapsed = header.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    trimmed
        .strip_suffix(';')
        .unwrap_or(trimmed)
        .trim_end()
        .to_string()
}

/// The body block of a declaration: `class_body`/`interface_body`/`enum_body`/
/// `annotation_type_body` for types (records use `class_body`), and
/// `body`/`constructor_body` for methods and constructors. A record's body is
/// resolved by `child_by_field_name("body")`, since its grammar field is a
/// `class_body`.
fn body_node(declaration: Node<'_>) -> Option<Node<'_>> {
    if let Some(body) = declaration.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = declaration.walk();
    declaration.children(&mut cursor).find(|child| {
        matches!(
            child.kind(),
            "class_body"
                | "interface_body"
                | "enum_body"
                | "annotation_type_body"
                | "constructor_body"
                | "block"
        )
    })
}

/// The `marker_annotation` and `annotation` nodes on a declaration, whether
/// wrapped in a `modifiers` node or direct children.
fn annotation_nodes(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut inner = child.walk();
            for modifier in child.children(&mut inner) {
                if is_annotation_kind(modifier.kind()) {
                    out.push(modifier);
                }
            }
        } else if is_annotation_kind(child.kind()) {
            out.push(child);
        }
    }
    out
}

fn is_annotation_kind(node_kind: &str) -> bool {
    matches!(node_kind, "marker_annotation" | "annotation")
}

/// The cleaned Javadoc immediately above a declaration, or `None` when the
/// preceding sibling is not a `/** ... */` block comment.
fn javadoc_of(declaration: Node<'_>, source: &str) -> Option<String> {
    let previous = declaration.prev_sibling()?;
    if previous.kind() != "block_comment" {
        return None;
    }
    let raw = text(previous, source);
    if !raw.starts_with("/**") {
        return None;
    }
    Some(clean_javadoc(&raw))
}

/// Strip the `/**`, `*/` and per-line `* ` decoration from a Javadoc comment.
fn clean_javadoc(raw: &str) -> String {
    let body = raw.strip_prefix("/**").unwrap_or(raw);
    let body = body.strip_suffix("*/").unwrap_or(body);
    body.lines()
        .map(|line| {
            let trimmed = line.trim();
            let without_star = trimmed.strip_prefix('*').unwrap_or(trimmed);
            without_star
                .strip_prefix(' ')
                .unwrap_or(without_star)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The Spring role of a type, or `None` for a member or an unannotated type.
/// Precedence: controller, service, repository, configuration, component,
/// entity.
fn role_of(
    kind: SymbolKind,
    annotation_names: &[&str],
    declaration: Node<'_>,
    source: &str,
) -> Option<Role> {
    if !kind.is_type() {
        return None;
    }
    let has = |name: &str| annotation_names.contains(&name);

    if has("Controller") || has("RestController") {
        Some(Role::Controller)
    } else if has("Service") {
        Some(Role::Service)
    } else if has("Repository")
        || (kind == SymbolKind::Interface && is_spring_data_interface(declaration, source))
    {
        Some(Role::Repository)
    } else if has("Configuration") {
        Some(Role::Configuration)
    } else if has("Component") {
        Some(Role::Component)
    } else if has("Entity") {
        Some(Role::Entity)
    } else {
        None
    }
}

/// The simple name of an annotation, e.g. `Service` for
/// `@org.springframework.stereotype.Service`. Arguments are ignored.
fn annotation_simple_name(annotation: &str) -> &str {
    let name = annotation.trim_start_matches('@');
    let name = name
        .split(['(', ' ', '\t', '\n', '\r'])
        .next()
        .unwrap_or(name);
    name.rsplit('.').next().unwrap_or(name)
}

/// Whether an interface extends a known Spring Data repository interface, or
/// any superinterface whose simple name ends with `Repository`.
fn is_spring_data_interface(declaration: Node<'_>, source: &str) -> bool {
    const KNOWN: &[&str] = &[
        "JpaRepository",
        "CrudRepository",
        "PagingAndSortingRepository",
        "ListCrudRepository",
        "ListPagingAndSortingRepository",
        "JpaSpecificationExecutor",
        "MongoRepository",
        "ReactiveMongoRepository",
        "ReactiveCrudRepository",
        "ReactiveSortingRepository",
        "R2dbcRepository",
        "ElasticsearchRepository",
        "Neo4jRepository",
        "CassandraRepository",
        "CouchbaseRepository",
        "JdbcRepository",
    ];
    superinterface_names(declaration, source)
        .into_iter()
        .any(|name| name.ends_with("Repository") || KNOWN.contains(&name.as_str()))
}

/// Simple names of the types in a declaration's `extends` list.
fn superinterface_names(declaration: Node<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        if matches!(child.kind(), "super_interfaces" | "extends_interfaces") {
            collect_super_types(child, source, &mut names);
        }
    }
    names
}

fn collect_super_types(node: Node<'_>, source: &str, out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "type_list" => collect_super_types(child, source, out),
            "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                out.push(simple_type_name(&text(child, source)));
            }
            _ => {}
        }
    }
}

/// The simple name of a rendered type, dropping a package prefix and generic
/// arguments, e.g. `JpaRepository` for `com.acme.JpaRepository<User, Long>`.
fn simple_type_name(rendered: &str) -> String {
    let base = rendered.split('<').next().unwrap_or(rendered).trim();
    base.rsplit('.').next().unwrap_or(base).to_string()
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

/// Node kinds that hold type and member declarations directly: the bodies of
/// each type kind, and the declarations section of an enum body. The root
/// `program` is passed to `collect_symbols` directly and is never discovered
/// through this predicate.
fn is_type_container(node_kind: &str) -> bool {
    matches!(
        node_kind,
        "class_body"
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
    match node.utf8_text(source.as_bytes()) {
        Ok(text) => text.to_string(),
        Err(_) => {
            tracing::warn!(
                kind = node.kind(),
                start_byte = node.start_byte(),
                end_byte = node.end_byte(),
                "source span is not valid UTF-8; using an empty string"
            );
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> ParsedFile {
        JavaParser::new()
            .unwrap()
            .parse(Path::new("Fixture.java"), source)
            .unwrap()
    }

    fn parse(source: &str) -> Vec<Symbol> {
        parsed(source).symbols
    }

    fn by_name<'a>(symbols: &'a [Symbol], name: &str) -> &'a Symbol {
        symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name} in {symbols:?}"))
    }

    fn by_fqn<'a>(symbols: &'a [Symbol], fqn: &str) -> &'a Symbol {
        symbols
            .iter()
            .find(|symbol| symbol.fqn == fqn)
            .unwrap_or_else(|| panic!("no symbol {fqn} in {symbols:?}"))
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
    fn broken_file_reports_a_parse_error_and_does_not_panic() {
        let source = "\
package com.acme.broken;

class Broken {
    void run() {
";
        let result = parsed(source);
        assert!(
            result.parse_error,
            "a missing closing brace is a parse error"
        );
        assert!(result.symbols.is_empty());
    }

    #[test]
    fn clean_file_without_symbols_is_not_a_parse_error() {
        let result = parsed("package com.acme;\n");
        assert!(!result.parse_error, "a valid package declaration parses");
        assert!(result.symbols.is_empty());
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
    fn javadoc_is_cleaned_when_present_and_none_when_absent() {
        let source = "\
package com.acme.doc;

/**
 * Handles users.
 * @deprecated use NewService
 */
class UserService {
    /**
     * Finds a user.
     */
    User find(Long id) { return null; }

    User other() { return null; }
}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.doc.UserService")
                .javadoc
                .as_deref(),
            Some("Handles users.\n@deprecated use NewService")
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.doc.UserService#find(Long)")
                .javadoc
                .as_deref(),
            Some("Finds a user.")
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.doc.UserService#other()").javadoc,
            None
        );
    }

    #[test]
    fn annotations_are_captured_as_written_including_multiline() {
        let source = "\
package com.acme.ann;

@RestController
@RequestMapping(
    value = \"/users\",
    method = RequestMethod.GET
)
class UserController {
    @GetMapping(\"/{id}\")
    @ResponseBody
    User get(@PathVariable Long id) { return null; }

    void plain() {}
}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.ann.UserController").annotations,
            vec![
                "@RestController".to_string(),
                "@RequestMapping(\n    value = \"/users\",\n    method = RequestMethod.GET\n)"
                    .to_string(),
            ]
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.ann.UserController#get(Long)").annotations,
            vec![
                "@GetMapping(\"/{id}\")".to_string(),
                "@ResponseBody".to_string(),
            ]
        );
        assert!(
            by_fqn(&symbols, "com.acme.ann.UserController#plain()")
                .annotations
                .is_empty()
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.ann.UserController").signature,
            "class UserController",
            "a multi-line annotation is removed from the signature"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.ann.UserController#get(Long)").signature,
            "User get(@PathVariable Long id)"
        );
    }

    #[test]
    fn signature_is_the_declaration_without_its_body() {
        let source = "\
package com.acme.sig;

public class Foo extends Bar implements Baz {
}

interface Api {
    Long count();
}

abstract class Base {
    public abstract void run();
}

class Impl {
    public <T> T first(List<T> items) { return null; }
    void io() throws IOException {}
    public Impl(String name) {}
}

record Point(int x, int y) {}

enum Color { RED }
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Foo").signature,
            "public class Foo extends Bar implements Baz"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Api#count()").signature,
            "Long count()"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Base#run()").signature,
            "public abstract void run()"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Impl#first(List<T>)").signature,
            "public <T> T first(List<T> items)"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Impl#io()").signature,
            "void io() throws IOException"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Impl#<init>(String)").signature,
            "public Impl(String name)"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Point").signature,
            "record Point(int x, int y)"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sig.Color").signature,
            "enum Color"
        );
    }

    #[test]
    fn record_with_a_body_signature_stops_at_the_header() {
        let source = "\
package com.acme.recbody;

record Point(int x, int y) {
    Point {
        if (x < 0) { throw new IllegalArgumentException(); }
    }

    int sum() { return x + y; }
}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.recbody.Point").signature,
            "record Point(int x, int y)",
            "a record's body is a class_body, resolved by the body field"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.recbody.Point#sum()").signature,
            "int sum()"
        );
    }

    #[test]
    fn signature_drops_annotations_from_the_declaration() {
        let source = "\
package com.acme.sigann;

@Deprecated
public class Annotated {
    @Override
    public String toString() { return \"\"; }
}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.sigann.Annotated").signature,
            "public class Annotated"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.sigann.Annotated#toString()").signature,
            "public String toString()"
        );
    }

    #[test]
    fn every_role_is_derived_from_annotations_or_superinterfaces() {
        let source = "\
package com.acme.roles;

@Controller
class AController {}

@RestController
class BController {}

@Service
class AService {}

@Repository
class ARepository {}

interface UserRepository extends JpaRepository<User, Long> {}

interface SortingRepository extends PagingAndSortingRepository<User, Long> {}

interface DerivedRepository extends BaseRepository {}

interface PlainRepository {}

@Component
class AComponent {}

@Configuration
class AConfig {}

@Entity
class AEntity {}

@Service
@Component
class Both {}

class None {}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.AController").role,
            Some(Role::Controller)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.BController").role,
            Some(Role::Controller)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.AService").role,
            Some(Role::Service)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.ARepository").role,
            Some(Role::Repository)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.UserRepository").role,
            Some(Role::Repository)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.SortingRepository").role,
            Some(Role::Repository)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.DerivedRepository").role,
            Some(Role::Repository),
            "a superinterface whose simple name ends with Repository"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.PlainRepository").role,
            None,
            "an interface with no repository superinterface is not a repository"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.AComponent").role,
            Some(Role::Component)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.AConfig").role,
            Some(Role::Configuration)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.AEntity").role,
            Some(Role::Entity)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.roles.None").role,
            None,
            "a type with no relevant annotation has no role"
        );
    }

    #[test]
    fn role_precedence_prefers_the_more_specific_stereotype() {
        let source = "\
package com.acme.precedence;

@Service
@Component
class ServiceAndComponent {}

@Component
@Configuration
class ComponentAndConfig {}

@Repository
@Component
class RepoAndComponent {}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.precedence.ServiceAndComponent").role,
            Some(Role::Service)
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.precedence.ComponentAndConfig").role,
            Some(Role::Configuration),
            "configuration is a more specific stereotype than component"
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.precedence.RepoAndComponent").role,
            Some(Role::Repository)
        );
    }

    #[test]
    fn members_have_no_role_even_when_annotated() {
        let source = "\
package com.acme.memberrole;

@RestController
class Controller {
    @GetMapping(\"/x\")
    String get() { return \"\"; }

    Controller() {}
}
";
        let symbols = parse(source);

        assert_eq!(
            by_fqn(&symbols, "com.acme.memberrole.Controller#get()").role,
            None
        );
        assert_eq!(
            by_fqn(&symbols, "com.acme.memberrole.Controller#<init>()").role,
            None
        );
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
