//! What usage resolution needs from one file besides its symbols: imports,
//! super types, fields, Lombok annotations, return and parameter types.
//!
//! Collected on the same walk as the [`Symbol`](super::Symbol)s and kept in
//! memory on the [`ParsedFile`](super::ParsedFile); nothing here is stored in
//! the index. Types are recorded as written, never resolved: turning a name
//! into an fqn needs the whole run's symbol table and is the resolver's job.

use tree_sitter::Node;

use super::{SymbolKind, annotation_nodes, annotation_simple_name, is_type_node, normalize, text};

/// One `import` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The imported name without `import`, `static` and a trailing `.*`, e.g.
    /// `a.b.Foo` for `import a.b.Foo;`, `a.b` for `import a.b.*;` and
    /// `a.b.Foo.bar` for `import static a.b.Foo.bar;`.
    pub path: String,
    pub is_static: bool,
    /// Whether the import ends in `.*`.
    pub wildcard: bool,
}

/// A type as written in the source, generic arguments kept apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    /// The name as written without generic arguments and array brackets:
    /// `Foo`, `Outer.Inner`, `com.acme.Foo`, a primitive such as `int`,
    /// `void`, or `?` for a wildcard.
    pub name: String,
    /// The generic arguments, in order. A wildcard is a `?` whose single
    /// argument is its bound, if it has one (`? extends Foo`, `? super Foo`).
    pub args: Vec<TypeRef>,
    /// The number of array dimensions, including those written after a
    /// variable name (`int values[]`).
    pub dims: usize,
    /// The 1-based line of the type in the source.
    pub line: usize,
}

/// A field of a type: one per declarator, so `int a, b;` gives two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldFacts {
    pub name: String,
    pub ty: TypeRef,
    /// Declared `static`, or an interface constant (implicitly static).
    pub is_static: bool,
    /// Lombok annotations on the field, by simple name (`Getter`, `Setter`).
    pub lombok: Vec<String>,
    pub line: usize,
}

/// A parameter of a method or constructor, or a record component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// The declared type; for a varargs parameter the element type, so
    /// `String... values` is `String` with `varargs` set on the method.
    pub ty: TypeRef,
}

/// A method or constructor of a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodFacts {
    /// The member's fqn, equal to its [`Symbol::fqn`](super::Symbol::fqn).
    pub fqn: String,
    /// The simple name; `<init>` for a constructor.
    pub name: String,
    /// The declared return type as written; `None` for a constructor.
    pub return_type: Option<TypeRef>,
    /// The parameters in order. A compact constructor's are the record
    /// components (its symbol fqn stays `R#<init>()`): count arguments
    /// against these, never against the fqn text.
    pub params: Vec<Param>,
    /// Whether the last parameter is a varargs parameter.
    pub varargs: bool,
    pub is_static: bool,
    /// The method's own type variables (`<T> T first(List<T>)`).
    pub type_params: Vec<String>,
    pub line: usize,
}

/// The resolution facts of one named type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFacts {
    /// The type's fqn, equal to its [`Symbol::fqn`](super::Symbol::fqn).
    pub fqn: String,
    pub name: String,
    pub kind: SymbolKind,
    /// The fqn of the enclosing type, `None` for a top-level type. The nested
    /// types of a type are those whose `parent` is its fqn.
    pub parent: Option<String>,
    /// The type's own type variables (`class Box<T>`).
    pub type_params: Vec<String>,
    /// The `extends` type of a class.
    pub superclass: Option<TypeRef>,
    /// The `implements` list of a class, enum or record, or the `extends`
    /// list of an interface.
    pub interfaces: Vec<TypeRef>,
    /// Lombok annotations on the type, by simple name (`Getter`, `Data`,
    /// `Value`, `Builder`, `RequiredArgsConstructor`, ...).
    pub lombok: Vec<String>,
    pub fields: Vec<FieldFacts>,
    pub methods: Vec<MethodFacts>,
    /// The constant names of an enum, in order.
    pub enum_constants: Vec<String>,
    /// The components of a record, in order. A varargs component
    /// (`String... rest`) is an array (`String[]`), as its field and accessor
    /// are.
    pub record_components: Vec<Param>,
}

/// The imports of the file root `program`, in source order.
pub(super) fn imports(root: Node<'_>, source: &str) -> Vec<Import> {
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        let mut inner = child.walk();
        let parts: Vec<Node<'_>> = child.children(&mut inner).collect();
        let Some(path) = parts
            .iter()
            .find(|part| matches!(part.kind(), "scoped_identifier" | "identifier"))
        else {
            continue;
        };
        out.push(Import {
            path: normalize(&text(*path, source)),
            is_static: parts.iter().any(|part| part.kind() == "static"),
            wildcard: parts.iter().any(|part| part.kind() == "asterisk"),
        });
    }
    out
}

/// The facts of a type declaration, without its fields and members, which
/// the walk adds as it meets them.
pub(super) fn type_facts(
    declaration: Node<'_>,
    source: &str,
    imports: &[Import],
    fqn: &str,
    name: &str,
    kind: SymbolKind,
    parent: Option<&str>,
) -> TypeFacts {
    let superclass = declaration
        .child_by_field_name("superclass")
        .and_then(first_ref_child)
        .map(|ty| type_ref(ty, source));
    let mut interfaces = Vec::new();
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        if matches!(child.kind(), "super_interfaces" | "extends_interfaces") {
            let mut inner = child.walk();
            for list in child.named_children(&mut inner) {
                if list.kind() == "type_list" {
                    interfaces.extend(type_children(list, source));
                }
            }
        }
    }
    let mut enum_constants = Vec::new();
    if let Some(body) = declaration.child_by_field_name("body")
        && body.kind() == "enum_body"
    {
        let mut inner = body.walk();
        for constant in body.named_children(&mut inner) {
            if constant.kind() == "enum_constant"
                && let Some(name) = constant.child_by_field_name("name")
            {
                enum_constants.push(text(name, source));
            }
        }
    }
    let record_components = if kind == SymbolKind::Record {
        declaration
            .child_by_field_name("parameters")
            .map(|parameters| {
                let (mut components, varargs) = params(parameters, source);
                if varargs && let Some(last) = components.last_mut() {
                    last.ty.dims += 1;
                }
                components
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    TypeFacts {
        fqn: fqn.to_string(),
        name: name.to_string(),
        kind,
        parent: parent.map(str::to_string),
        type_params: type_params(declaration, source),
        superclass,
        interfaces,
        lombok: lombok_annotations(declaration, source, imports),
        fields: Vec::new(),
        methods: Vec::new(),
        enum_constants,
        record_components,
    }
}

/// The fields of a `field_declaration` or an interface's
/// `constant_declaration`, one per declarator.
pub(super) fn field_facts(
    declaration: Node<'_>,
    source: &str,
    imports: &[Import],
) -> Vec<FieldFacts> {
    let Some(ty) = declaration.child_by_field_name("type") else {
        return Vec::new();
    };
    let is_static =
        declaration.kind() == "constant_declaration" || has_modifier(declaration, "static");
    let lombok = lombok_annotations(declaration, source, imports);
    let base = type_ref(ty, source);
    let mut out = Vec::new();
    let mut cursor = declaration.walk();
    for declarator in declaration.children_by_field_name("declarator", &mut cursor) {
        let Some(name) = declarator.child_by_field_name("name") else {
            continue;
        };
        let mut ty = base.clone();
        ty.dims += dims_of(declarator.child_by_field_name("dimensions"), source);
        out.push(FieldFacts {
            name: text(name, source),
            ty,
            is_static,
            lombok: lombok.clone(),
            line: declarator.start_position().row + 1,
        });
    }
    out
}

/// The facts of a method or constructor declaration whose symbol has `fqn`
/// and simple `name`.
pub(super) fn method_facts(
    declaration: Node<'_>,
    source: &str,
    fqn: &str,
    name: &str,
) -> MethodFacts {
    let return_type = (declaration.kind() == "method_declaration")
        .then(|| declaration.child_by_field_name("type"))
        .flatten()
        .map(|ty| {
            let mut ty = type_ref(ty, source);
            ty.dims += dims_of(declaration.child_by_field_name("dimensions"), source);
            ty
        });
    let parameters = if declaration.kind() == "compact_constructor_declaration" {
        declaration
            .parent()
            .and_then(|body| body.parent())
            .filter(|record| record.kind() == "record_declaration")
            .and_then(|record| record.child_by_field_name("parameters"))
    } else {
        declaration.child_by_field_name("parameters")
    };
    let (params, varargs) = parameters
        .map(|parameters| params(parameters, source))
        .unwrap_or_default();
    MethodFacts {
        fqn: fqn.to_string(),
        name: name.to_string(),
        return_type,
        params,
        varargs,
        is_static: has_modifier(declaration, "static"),
        type_params: type_params(declaration, source),
        line: declaration.start_position().row + 1,
    }
}

/// The parameters of a `formal_parameters` node and whether the last one is
/// varargs. A receiver parameter (`Foo this`) is not a parameter.
fn params(parameters: Node<'_>, source: &str) -> (Vec<Param>, bool) {
    let mut out = Vec::new();
    let mut varargs = false;
    let mut cursor = parameters.walk();
    for param in parameters.named_children(&mut cursor) {
        match param.kind() {
            "formal_parameter" => {
                let (Some(ty), Some(name)) = (
                    param.child_by_field_name("type"),
                    param.child_by_field_name("name"),
                ) else {
                    continue;
                };
                let mut ty = type_ref(ty, source);
                ty.dims += dims_of(param.child_by_field_name("dimensions"), source);
                out.push(Param {
                    name: text(name, source),
                    ty,
                });
            }
            "spread_parameter" => {
                let Some(ty) = first_ref_child(param) else {
                    continue;
                };
                let mut inner = param.walk();
                let declarator = param
                    .named_children(&mut inner)
                    .find(|child| child.kind() == "variable_declarator");
                let Some(name) = declarator.and_then(|node| node.child_by_field_name("name"))
                else {
                    continue;
                };
                out.push(Param {
                    name: text(name, source),
                    ty: type_ref(ty, source),
                });
                varargs = true;
            }
            _ => {}
        }
    }
    (out, varargs)
}

/// The names of the `type_parameters` of a type or method declaration.
fn type_params(declaration: Node<'_>, source: &str) -> Vec<String> {
    let Some(parameters) = declaration.child_by_field_name("type_parameters") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() != "type_parameter" {
            continue;
        }
        let mut inner = parameter.walk();
        if let Some(name) = parameter
            .named_children(&mut inner)
            .find(|child| child.kind() == "type_identifier")
        {
            out.push(text(name, source));
        }
    }
    out
}

/// The type nodes directly in `node` (a `type_list` or `type_arguments`), as
/// [`TypeRef`]s; wildcards included.
fn type_children(node: Node<'_>, source: &str) -> Vec<TypeRef> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| is_ref_node(child.kind()))
        .map(|child| type_ref(child, source))
        .collect()
}

/// The first type node directly in `node`, annotated types included.
fn first_ref_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| is_ref_node(child.kind()))
}

fn is_ref_node(node_kind: &str) -> bool {
    is_type_node(node_kind) || matches!(node_kind, "void_type" | "annotated_type" | "wildcard")
}

/// A type node as a [`TypeRef`]: generic arguments split off, array
/// dimensions counted, type annotations dropped.
fn type_ref(node: Node<'_>, source: &str) -> TypeRef {
    let line = node.start_position().row + 1;
    let named = |kinds: &[&str]| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| kinds.contains(&child.kind()))
    };
    match node.kind() {
        "generic_type" => {
            let name = named(&["type_identifier", "scoped_type_identifier"])
                .map(|base| type_ref(base, source).name)
                .unwrap_or_default();
            let args = named(&["type_arguments"])
                .map(|arguments| type_children(arguments, source))
                .unwrap_or_default();
            TypeRef {
                name,
                args,
                dims: 0,
                line,
            }
        }
        "array_type" => {
            let mut element = node
                .child_by_field_name("element")
                .map(|element| type_ref(element, source))
                .unwrap_or_else(|| plain(String::new(), line));
            element.dims += dims_of(node.child_by_field_name("dimensions"), source);
            element.line = line;
            element
        }
        "annotated_type" => {
            let mut cursor = node.walk();
            let inner = node
                .named_children(&mut cursor)
                .find(|child| is_ref_node(child.kind()));
            match inner {
                Some(inner) => type_ref(inner, source),
                None => plain(String::new(), line),
            }
        }
        "wildcard" => {
            let mut cursor = node.walk();
            let args = node
                .named_children(&mut cursor)
                .filter(|child| is_ref_node(child.kind()))
                .map(|child| type_ref(child, source))
                .collect();
            TypeRef {
                name: "?".to_string(),
                args,
                dims: 0,
                line,
            }
        }
        "scoped_type_identifier" => {
            let mut cursor = node.walk();
            let parts: Vec<String> = node
                .named_children(&mut cursor)
                .filter(|child| is_ref_node(child.kind()))
                .map(|child| type_ref(child, source).name)
                .collect();
            plain(parts.join("."), line)
        }
        _ => plain(normalize(&text(node, source)), line),
    }
}

fn plain(name: String, line: usize) -> TypeRef {
    TypeRef {
        name,
        args: Vec::new(),
        dims: 0,
        line,
    }
}

/// The number of `[]` pairs in an optional `dimensions` node.
fn dims_of(dimensions: Option<Node<'_>>, source: &str) -> usize {
    dimensions.map_or(0, |node| text(node, source).matches('[').count())
}

/// Whether a declaration's `modifiers` include the keyword `modifier`.
fn has_modifier(declaration: Node<'_>, modifier: &str) -> bool {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .filter(|child| child.kind() == "modifiers")
        .any(|modifiers| {
            let mut inner = modifiers.walk();
            modifiers
                .children(&mut inner)
                .any(|keyword| keyword.kind() == modifier)
        })
}

/// The Lombok annotations a wildcard import (`import lombok.*;`) can bring in:
/// a wildcard does not say which names its package has, and `@Deprecated` or
/// `@Override` must not count.
const LOMBOK_ANNOTATIONS: &[&str] = &[
    "AllArgsConstructor",
    "Builder",
    "Data",
    "EqualsAndHashCode",
    "Getter",
    "NoArgsConstructor",
    "RequiredArgsConstructor",
    "Setter",
    "Singular",
    "Slf4j",
    "SuperBuilder",
    "ToString",
    "Value",
    "With",
];

/// The simple names of the Lombok annotations on a declaration: written as
/// `@lombok.X`, or as `@X` with `X` imported from a `lombok` package (a
/// single-type import, or a wildcard import for the names in
/// [`LOMBOK_ANNOTATIONS`]). Same-named annotations from elsewhere, such as
/// Spring's `@Value`, are left out.
fn lombok_annotations(declaration: Node<'_>, source: &str, imports: &[Import]) -> Vec<String> {
    annotation_nodes(declaration)
        .into_iter()
        .filter_map(|annotation| {
            let written = text(annotation, source);
            let written = written.trim().trim_start_matches('@');
            let qualified = written
                .split(['(', ' ', '\t', '\n', '\r'])
                .next()
                .unwrap_or(written);
            let simple = annotation_simple_name(qualified);
            let is_lombok_path = |path: &str| path == "lombok" || path.starts_with("lombok.");
            let is_lombok = if qualified.contains('.') {
                qualified.starts_with("lombok.")
            } else {
                let mut type_imports = imports.iter().filter(|import| !import.is_static);
                match type_imports.clone().find(|import| {
                    !import.wildcard && import.path.rsplit('.').next() == Some(simple)
                }) {
                    Some(single) => is_lombok_path(&single.path),
                    None => {
                        LOMBOK_ANNOTATIONS.contains(&simple)
                            && type_imports
                                .any(|import| import.wildcard && is_lombok_path(&import.path))
                    }
                }
            };
            is_lombok.then(|| simple.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{JavaParser, ParsedFile};
    use super::*;

    fn parsed(source: &str) -> ParsedFile {
        JavaParser::new()
            .unwrap()
            .parse(Path::new("Fixture.java"), source)
            .unwrap()
    }

    fn type_of<'a>(file: &'a ParsedFile, fqn: &str) -> &'a TypeFacts {
        file.types
            .iter()
            .find(|ty| ty.fqn == fqn)
            .unwrap_or_else(|| panic!("no type {fqn} in {:?}", file.types))
    }

    fn method_of<'a>(ty: &'a TypeFacts, fqn: &str) -> &'a MethodFacts {
        ty.methods
            .iter()
            .find(|method| method.fqn == fqn)
            .unwrap_or_else(|| panic!("no method {fqn} in {:?}", ty.methods))
    }

    fn field_of<'a>(ty: &'a TypeFacts, name: &str) -> &'a FieldFacts {
        ty.fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("no field {name} in {:?}", ty.fields))
    }

    fn named(name: &str, line: usize) -> TypeRef {
        TypeRef {
            name: name.to_string(),
            args: Vec::new(),
            dims: 0,
            line,
        }
    }

    fn generic(name: &str, args: Vec<TypeRef>, line: usize) -> TypeRef {
        TypeRef {
            args,
            ..named(name, line)
        }
    }

    #[test]
    fn records_the_package_and_each_import_form() {
        let file = parsed(
            "\
package com.acme.app;

import com.acme.model.User;
import com.acme.util.*;
import static com.acme.util.Strings.trim;
import static com.acme.util.Limits.*;
import java.util.Map.Entry;

class App {}
",
        );
        assert_eq!(file.package, "com.acme.app");
        let import = |path: &str, is_static: bool, wildcard: bool| Import {
            path: path.to_string(),
            is_static,
            wildcard,
        };
        assert_eq!(
            file.imports,
            vec![
                import("com.acme.model.User", false, false),
                import("com.acme.util", false, true),
                import("com.acme.util.Strings.trim", true, false),
                import("com.acme.util.Limits", true, true),
                import("java.util.Map.Entry", false, false),
            ]
        );
    }

    #[test]
    fn a_file_without_package_or_imports_has_none() {
        let file = parsed("class Bare {}\n");
        assert_eq!(file.package, "");
        assert!(file.imports.is_empty());
        assert_eq!(file.types.len(), 1);
    }

    #[test]
    fn super_types_keep_generic_arguments_apart() {
        let file = parsed(
            "\
package com.acme.sup;

class Consumer<T extends Message> extends Base<Map<String, List<T>>>
        implements MessageConsumer<SecurityMethodAddMessage>, com.acme.api.Closeable, Outer.Inner<?, ? extends Event> {}

interface Repo extends JpaRepository<User, Long>, Custom {}

enum Kind implements Labeled { A, B }

record Pair(String left, int[] right) implements Comparable<Pair> {}
",
        );
        let consumer = type_of(&file, "com.acme.sup.Consumer");
        assert_eq!(consumer.type_params, ["T"]);
        assert_eq!(
            consumer.superclass,
            Some(generic(
                "Base",
                vec![generic(
                    "Map",
                    vec![named("String", 3), generic("List", vec![named("T", 3)], 3)],
                    3
                )],
                3
            ))
        );
        assert_eq!(
            consumer.interfaces,
            vec![
                generic(
                    "MessageConsumer",
                    vec![named("SecurityMethodAddMessage", 4)],
                    4
                ),
                named("com.acme.api.Closeable", 4),
                generic(
                    "Outer.Inner",
                    vec![named("?", 4), generic("?", vec![named("Event", 4)], 4)],
                    4
                ),
            ]
        );

        let repo = type_of(&file, "com.acme.sup.Repo");
        assert_eq!(repo.kind, SymbolKind::Interface);
        assert_eq!(repo.superclass, None);
        assert_eq!(
            repo.interfaces,
            vec![
                generic("JpaRepository", vec![named("User", 6), named("Long", 6)], 6),
                named("Custom", 6),
            ]
        );

        let kind = type_of(&file, "com.acme.sup.Kind");
        assert_eq!(kind.interfaces, vec![named("Labeled", 8)]);
        assert_eq!(kind.enum_constants, ["A", "B"]);

        let pair = type_of(&file, "com.acme.sup.Pair");
        assert_eq!(
            pair.interfaces,
            vec![generic("Comparable", vec![named("Pair", 10)], 10)]
        );
        assert_eq!(
            pair.record_components,
            vec![
                Param {
                    name: "left".to_string(),
                    ty: named("String", 10),
                },
                Param {
                    name: "right".to_string(),
                    ty: TypeRef {
                        dims: 1,
                        ..named("int", 10)
                    },
                },
            ]
        );
    }

    #[test]
    fn an_annotated_superclass_is_kept() {
        let file = parsed("class B extends @Deprecated Base implements @Ann I {}\n");
        let b = type_of(&file, "B");
        assert_eq!(b.superclass, Some(named("Base", 1)));
        assert_eq!(b.interfaces, vec![named("I", 1)]);
    }

    #[test]
    fn records_type_varargs_components_and_the_compact_constructor() {
        let file = parsed(
            "\
package com.acme.rec;

record Batch(int size, String... rest) {
    Batch {
        if (size < 0) throw new IllegalArgumentException();
    }
    Batch(int size) { this(size, new String[0]); }
}
",
        );
        let batch = type_of(&file, "com.acme.rec.Batch");
        assert_eq!(
            batch.record_components,
            vec![
                Param {
                    name: "size".to_string(),
                    ty: named("int", 3),
                },
                Param {
                    name: "rest".to_string(),
                    ty: TypeRef {
                        dims: 1,
                        ..named("String", 3)
                    },
                },
            ],
            "a varargs component is an array, like its field and accessor"
        );

        let compact = method_of(batch, "com.acme.rec.Batch#<init>()");
        assert!(compact.varargs);
        assert_eq!(
            compact.params,
            vec![
                Param {
                    name: "size".to_string(),
                    ty: named("int", 3),
                },
                Param {
                    name: "rest".to_string(),
                    ty: named("String", 3),
                },
            ],
            "a compact constructor takes the record components; its fqn is unchanged"
        );

        let other = method_of(batch, "com.acme.rec.Batch#<init>(int)");
        assert!(!other.varargs);
        assert_eq!(other.params.len(), 1);
    }

    #[test]
    fn a_receiver_parameter_is_in_the_fqn_but_not_a_parameter() {
        let file = parsed("class A { void m(A this, int x) {} }\n");
        let a = type_of(&file, "A");
        let m = method_of(a, "A#m(A,int)");
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "x");
    }

    #[test]
    fn fields_record_type_static_and_lombok_annotations() {
        let file = parsed(
            "\
package com.acme.fields;

import lombok.Getter;
import lombok.experimental.*;
import org.springframework.beans.factory.annotation.Value;

@Getter
@lombok.RequiredArgsConstructor
@SuperBuilder
@Deprecated
class Config {
    private final SecurityDataAddEventConsumer addConsumer;
    @lombok.Setter @Value(\"${queue}\") private String queue;
    private static final Logger LOGGER = LoggerFactory.getLogger(Config.class);
    int first, second[];
    private java.util.List<@NonNull Handler> handlers;
}

interface Limits {
    int MAX = 10;
}

class Plain {
    private Repo repo;
}
",
        );
        let config = type_of(&file, "com.acme.fields.Config");
        assert_eq!(
            config.lombok,
            ["Getter", "RequiredArgsConstructor", "SuperBuilder"]
        );

        let add = field_of(config, "addConsumer");
        assert_eq!(add.ty, named("SecurityDataAddEventConsumer", 12));
        assert!(!add.is_static);
        assert!(add.lombok.is_empty());
        assert_eq!(add.line, 12);

        let queue = field_of(config, "queue");
        assert_eq!(queue.ty, named("String", 13));
        assert_eq!(
            queue.lombok,
            ["Setter"],
            "Spring's @Value is imported from Spring, not Lombok"
        );

        let logger = field_of(config, "LOGGER");
        assert!(logger.is_static);
        assert_eq!(logger.ty, named("Logger", 14));

        assert_eq!(field_of(config, "first").ty, named("int", 15));
        assert_eq!(field_of(config, "second").ty.dims, 1);
        assert_eq!(
            field_of(config, "handlers").ty,
            generic("java.util.List", vec![named("Handler", 16)], 16)
        );

        let max = field_of(type_of(&file, "com.acme.fields.Limits"), "MAX");
        assert!(max.is_static, "an interface constant is implicitly static");

        let plain = type_of(&file, "com.acme.fields.Plain");
        assert!(plain.lombok.is_empty());
        assert!(field_of(plain, "repo").lombok.is_empty());
    }

    #[test]
    fn lombok_names_need_a_lombok_import_or_qualification() {
        let file = parsed(
            "\
import lombok.*;
import org.springframework.beans.factory.annotation.Value;

@Value
@Data
@Builder
class Dto {
    @Getter private String name;
}

@com.acme.Getter
class Other {}
",
        );
        assert_eq!(
            type_of(&file, "Dto").lombok,
            ["Data", "Builder"],
            "a single-type import of Spring's Value shadows lombok.*"
        );
        assert_eq!(field_of(type_of(&file, "Dto"), "name").lombok, ["Getter"]);
        assert!(type_of(&file, "Other").lombok.is_empty());
    }

    #[test]
    fn methods_record_return_and_parameter_types() {
        let file = parsed(
            "\
package com.acme.methods;

class Service {
    Service(Repo repo, @Qualifier(\"x\") Clock clock) {}
    public SecurityDataAddEventConsumer getAddConsumer() { return null; }
    void dispatch(SecurityDataMessageBearer<?> message) {}
    static <T> List<T> wrap(T value) { return null; }
    void log(String format, Object... args) {}
    void log(String[] lines) {}
    int legacy()[] { return null; }
}
",
        );
        let service = type_of(&file, "com.acme.methods.Service");
        assert_eq!(
            service
                .methods
                .iter()
                .map(|method| method.fqn.as_str())
                .collect::<Vec<_>>(),
            [
                "com.acme.methods.Service#<init>(Repo,Clock)",
                "com.acme.methods.Service#getAddConsumer()",
                "com.acme.methods.Service#dispatch(SecurityDataMessageBearer<?>)",
                "com.acme.methods.Service#wrap(T)",
                "com.acme.methods.Service#log(String,Object...)",
                "com.acme.methods.Service#log(String[])",
                "com.acme.methods.Service#legacy()",
            ]
        );

        let init = method_of(service, "com.acme.methods.Service#<init>(Repo,Clock)");
        assert_eq!(init.name, "<init>");
        assert_eq!(init.return_type, None);
        assert_eq!(
            init.params
                .iter()
                .map(|param| (param.name.as_str(), param.ty.name.as_str()))
                .collect::<Vec<_>>(),
            [("repo", "Repo"), ("clock", "Clock")]
        );

        let getter = method_of(service, "com.acme.methods.Service#getAddConsumer()");
        assert_eq!(
            getter.return_type,
            Some(named("SecurityDataAddEventConsumer", 5))
        );
        assert!(getter.params.is_empty());
        assert!(!getter.is_static);

        let dispatch = method_of(
            service,
            "com.acme.methods.Service#dispatch(SecurityDataMessageBearer<?>)",
        );
        assert_eq!(dispatch.return_type, Some(named("void", 6)));
        assert_eq!(
            dispatch.params[0].ty,
            generic("SecurityDataMessageBearer", vec![named("?", 6)], 6)
        );

        let wrap = method_of(service, "com.acme.methods.Service#wrap(T)");
        assert!(wrap.is_static);
        assert_eq!(wrap.type_params, ["T"]);
        assert_eq!(
            wrap.return_type,
            Some(generic("List", vec![named("T", 7)], 7))
        );

        let varargs = method_of(service, "com.acme.methods.Service#log(String,Object...)");
        assert!(varargs.varargs);
        assert_eq!(
            varargs.params,
            vec![
                Param {
                    name: "format".to_string(),
                    ty: named("String", 8),
                },
                Param {
                    name: "args".to_string(),
                    ty: named("Object", 8),
                },
            ]
        );

        let array = method_of(service, "com.acme.methods.Service#log(String[])");
        assert!(!array.varargs);
        assert_eq!(array.params[0].ty.dims, 1);

        let legacy = method_of(service, "com.acme.methods.Service#legacy()");
        assert_eq!(legacy.return_type.as_ref().map(|ty| ty.dims), Some(1));
    }

    #[test]
    fn nested_types_link_to_their_parent_and_own_their_members() {
        let file = parsed(
            "\
package com.acme.nest;

class Outer {
    private Inner inner;
    void run() {
        class Local { int hidden; }
        Runnable r = new Runnable() { public void run() {} };
    }
    static class Inner {
        Inner.Deep deep;
        interface Deep { void go(); }
    }
    enum Mode { ON; private final int code = 0; int code() { return code; } }
}
",
        );
        assert_eq!(
            file.types
                .iter()
                .map(|ty| (ty.fqn.as_str(), ty.parent.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("com.acme.nest.Outer", None),
                ("com.acme.nest.Outer.Inner", Some("com.acme.nest.Outer")),
                (
                    "com.acme.nest.Outer.Inner.Deep",
                    Some("com.acme.nest.Outer.Inner")
                ),
                ("com.acme.nest.Outer.Mode", Some("com.acme.nest.Outer")),
            ]
        );
        let outer = type_of(&file, "com.acme.nest.Outer");
        assert_eq!(
            outer
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["inner"],
            "fields of local classes are not the outer type's"
        );
        assert_eq!(outer.methods.len(), 1);
        assert_eq!(
            field_of(type_of(&file, "com.acme.nest.Outer.Inner"), "deep").ty,
            named("Inner.Deep", 10)
        );
        let deep = type_of(&file, "com.acme.nest.Outer.Inner.Deep");
        assert_eq!(deep.methods[0].fqn, "com.acme.nest.Outer.Inner.Deep#go()");
        let mode = type_of(&file, "com.acme.nest.Outer.Mode");
        assert_eq!(mode.enum_constants, ["ON"]);
        assert_eq!(field_of(mode, "code").ty, named("int", 13));
        assert_eq!(mode.methods[0].fqn, "com.acme.nest.Outer.Mode#code()");
    }

    #[test]
    fn facts_match_the_symbols_one_to_one() {
        let source = "\
package com.acme.same;

import java.util.List;

@Service
class Orders extends Base<Order> implements Api {
    private final Repo repo;
    Orders(Repo repo) { this.repo = repo; }
    public List<Order> all(String... ids) { return repo.findAll(); }
    class Line { Line() {} void add(int[] qty, Order... orders) {} }
}

record Point(int x, int y) { Point { } int sum() { return x + y; } }
enum Color { RED { void paint() {} }; void mix() {} }
@interface Marker { String value(); }
";
        let file = parsed(source);
        let type_fqns: Vec<&str> = file
            .symbols
            .iter()
            .filter(|symbol| symbol.kind.is_type())
            .map(|symbol| symbol.fqn.as_str())
            .collect();
        assert_eq!(
            file.types
                .iter()
                .map(|ty| ty.fqn.as_str())
                .collect::<Vec<_>>(),
            type_fqns
        );
        for symbol in file.symbols.iter().filter(|symbol| !symbol.kind.is_type()) {
            let owner = type_of(&file, symbol.parent.as_deref().unwrap());
            let method = method_of(owner, &symbol.fqn);
            assert_eq!(method.name, symbol.name);
            assert_eq!(method.line, symbol.start_line);
        }
        let members = file.types.iter().map(|ty| ty.methods.len()).sum::<usize>();
        assert_eq!(members, file.symbols.len() - type_fqns.len());
    }

    #[test]
    fn a_broken_file_has_no_facts() {
        let file = parsed("package com.acme;\nimport a.B;\nclass Broken {\n");
        assert!(file.parse_error);
        assert!(file.package.is_empty());
        assert!(file.imports.is_empty());
        assert!(file.types.is_empty());
    }
}
