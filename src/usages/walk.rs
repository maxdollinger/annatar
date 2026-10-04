//! The walk over one file's tree: scopes, regions, block-scoped locals and
//! the type mentions it turns into edges.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::symbols::{Symbol, is_type_container, member_kind, type_kind};

use super::resolver::{Lookup, Resolver, Scope, Ty, with_dims};
use super::{Edge, EdgeKind, Usages};

/// The type variables and local types declared in one member, field
/// declaration, initializer or enum constant, whatever their block: neither
/// names a type of the index.
#[derive(Debug, Default)]
pub(super) struct Region {
    pub(super) types: HashSet<String>,
    pub(super) type_vars: HashSet<String>,
}

/// One anonymous or local class around the current node.
pub(super) struct Unnamed<'a> {
    /// Where its super types start in [`FileWalk::unnamed`].
    pub(super) start: usize,
    /// Where its own fields and the variables declared in it start in
    /// [`FileWalk::locals`].
    pub(super) locals: usize,
    /// Whether a super type it names is outside the index, so its
    /// inherited members are unknown.
    pub(super) opaque: bool,
    /// The names of the methods it declares (not symbols).
    pub(super) methods: Vec<&'a str>,
}

/// The walk over one file's tree that turns its mentions into edges.
pub(super) struct FileWalk<'r, 'a> {
    pub(super) resolver: &'r Resolver<'a>,
    pub(super) file: usize,
    pub(super) source: &'a str,
    /// The file's written symbols by their declaration's start byte.
    pub(super) spans: HashMap<usize, (&'a Symbol, i64)>,
    /// The fqns of the named types around the current node, innermost last.
    pub(super) enclosing: Vec<&'a str>,
    /// The indexed super types of the anonymous and local classes around the
    /// current node, innermost last: their nested types are in scope.
    pub(super) unnamed: Vec<String>,
    /// Those classes, innermost last.
    pub(super) unnamed_classes: Vec<Unnamed<'a>>,
    /// The type variables of those types.
    pub(super) type_vars: Vec<String>,
    /// The local variables, parameters and anonymous-class fields in scope,
    /// with their types; a block's are dropped at its end.
    pub(super) locals: Vec<(&'a str, Ty)>,
    /// The edge source: the innermost member or type with a row.
    pub(super) src: Option<i64>,
    pub(super) region: Region,
    pub(super) usages: &'r mut Usages,
}

impl<'a> FileWalk<'_, 'a> {
    pub(super) fn children(&mut self, node: Node<'a>, named: bool) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, named);
        }
    }

    /// Visit `node`. `named` is true directly in the file or a named type's
    /// body, where declarations are symbols.
    pub(super) fn visit(&mut self, node: Node<'a>, named: bool) {
        let kind = node.kind();
        if named {
            if type_kind(kind).is_some() {
                return self.named_type(node);
            }
            if member_kind(kind).is_some() {
                return self.member(node);
            }
            if matches!(
                kind,
                "field_declaration"
                    | "constant_declaration"
                    | "static_initializer"
                    | "block"
                    | "enum_constant"
                    | "annotation_type_element_declaration"
            ) {
                return self.type_region(node);
            }
        }
        if is_typed_expression(kind) {
            self.expr(node);
            return;
        }
        match kind {
            "identifier" => {
                if is_expression_name(node) {
                    self.bare_name(node);
                }
            }
            _ => self.statement(node, named),
        }
    }

    /// Everything but a typed expression: declarations add their variables
    /// to the scope, blocks open one.
    pub(super) fn statement(&mut self, node: Node<'a>, named: bool) {
        match node.kind() {
            "import_declaration" | "package_declaration" | "line_comment" | "block_comment" => {}
            "type_identifier" => self.type_identifier(node, EdgeKind::Reference),
            "scoped_type_identifier" => self.scoped_type(node, EdgeKind::Reference),
            "marker_annotation" | "annotation" => self.annotation(node),
            kind if type_kind(kind).is_some() => self.local_type(node),
            "explicit_constructor_invocation" => self.constructor_invocation(node),
            "block"
            | "constructor_body"
            | "for_statement"
            | "catch_clause"
            | "try_with_resources_statement"
            | "switch_block"
            | "switch_rule" => {
                let locals = self.locals.len();
                self.children(node, false);
                self.locals.truncate(locals);
            }
            "local_variable_declaration" => self.local_variables(node),
            "formal_parameter" | "spread_parameter" | "catch_formal_parameter" => {
                self.children(node, false);
                if let Some((name, ty)) = self.parameter(node) {
                    self.locals.push((name, ty));
                }
            }
            "inferred_parameters" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.kind() == "identifier" {
                        self.locals
                            .push((node_text(child, self.source), Ty::Unknown));
                    }
                }
            }
            "enhanced_for_statement" => self.enhanced_for(node),
            "if_statement" | "while_statement" | "ternary_expression" => self.conditional(node),
            "instanceof_expression" => {
                self.children(node, false);
                if let (Some(name), Some(ty)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("right"),
                ) {
                    let ty = self.node_ty(ty);
                    self.locals.push((node_text(name, self.source), ty));
                }
            }
            "resource" => self.resource(node),
            "type_pattern" | "record_pattern_component" => {
                self.children(node, false);
                let mut cursor = node.walk();
                let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
                if let [ty, .., name] = children.as_slice()
                    && name.kind() == "identifier"
                {
                    let ty = self.node_ty(*ty);
                    self.locals.push((node_text(*name, self.source), ty));
                }
            }
            kind => self.children(node, named && is_type_container(kind)),
        }
    }

    /// The symbol whose declaration is exactly `node`.
    fn symbol_at(&self, node: Node<'_>) -> Option<(&'a Symbol, i64)> {
        self.spans
            .get(&node.start_byte())
            .filter(|(symbol, _)| symbol.end_byte == node.end_byte())
            .copied()
    }

    /// The scope of the current node.
    pub(super) fn scope(&self) -> Scope<'_> {
        Scope {
            unnamed: &self.unnamed,
            enclosing: self.enclosing.last().copied(),
        }
    }

    /// A named type: the source of its header, annotations and type-level
    /// code. A type without a row (a duplicate fqn) is skipped whole. Its
    /// header and annotations resolve around it (its type variables are in
    /// scope there, its nested types are not); its body and record
    /// components resolve inside it.
    fn named_type(&mut self, node: Node<'a>) {
        let Some((symbol, id)) = self.symbol_at(node) else {
            return;
        };
        let saved_src = self.src.replace(id);
        let saved_region = std::mem::take(&mut self.region);
        let saved_locals = std::mem::take(&mut self.locals);
        let type_vars = self.type_vars.len();
        if let Some(entry) = self.resolver.types.get(symbol.fqn.as_str()) {
            self.type_vars
                .extend(entry.facts.type_params.iter().cloned());
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "superclass" | "extends_interfaces" => self.header(child, EdgeKind::Extends),
                "super_interfaces" => self.header(child, EdgeKind::Implements),
                kind if is_type_container(kind) || kind == "formal_parameters" => {
                    self.enclosing.push(&symbol.fqn);
                    self.children(child, is_type_container(kind));
                    self.enclosing.pop();
                }
                _ => self.visit(child, false),
            }
        }
        self.type_vars.truncate(type_vars);
        self.locals = saved_locals;
        self.region = saved_region;
        self.src = saved_src;
    }

    /// A local class (or interface, enum, record): its header is a
    /// `reference` from the member around it, and the nested types of its
    /// indexed super types are in scope in its body.
    fn local_type(&mut self, node: Node<'a>) {
        let (supers, opaque) = self.super_types(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if is_type_container(child.kind()) {
                self.unnamed_body(child, supers.clone(), opaque);
            } else {
                self.visit(child, false);
            }
        }
    }

    /// The indexed types a local class's header names, and whether it
    /// names one outside the index.
    fn super_types(&self, node: Node<'a>) -> (Vec<String>, bool) {
        let mut supers = Vec::new();
        let mut opaque = false;
        let mut cursor = node.walk();
        for clause in node.children(&mut cursor).filter(|child| {
            matches!(
                child.kind(),
                "superclass" | "super_interfaces" | "extends_interfaces"
            )
        }) {
            let mut types = Vec::new();
            collect_header_types(clause, &mut types);
            for ty in types {
                match self.type_fqn(ty) {
                    Some(fqn) => supers.push(fqn),
                    None => opaque = true,
                }
            }
        }
        (supers, opaque)
    }

    /// The indexed type a type node names, without its generic arguments.
    fn type_fqn(&self, node: Node<'a>) -> Option<String> {
        match self.node_ty(node) {
            Ty::Indexed(fqn, 0) => Some(fqn),
            _ => None,
        }
    }

    /// The type a type node names in the current scope.
    pub(super) fn node_ty(&self, node: Node<'a>) -> Ty {
        match node.kind() {
            "integral_type" | "floating_point_type" | "boolean_type" | "void_type" => {
                Ty::External(node_text(node, self.source).to_string(), 0)
            }
            "array_type" => {
                let dims = dims(node, self.source);
                match node
                    .child_by_field_name("element")
                    .map(|element| self.node_ty(element))
                {
                    Some(Ty::Indexed(fqn, inner)) => Ty::Indexed(fqn, inner + dims),
                    Some(Ty::External(name, inner)) => Ty::External(name, inner + dims),
                    _ => Ty::Unknown,
                }
            }
            _ => {
                let mut segments = Vec::new();
                if !type_segments(node, self.source, &mut segments) {
                    return Ty::Unknown;
                }
                let Some(first) = segments.first() else {
                    return Ty::Unknown;
                };
                if !self.is_type_name(first) {
                    return Ty::Unknown;
                }
                match self
                    .resolver
                    .resolve_type(self.file, self.scope(), &segments)
                {
                    Lookup::Found(fqn) => Ty::Indexed(fqn, 0),
                    Lookup::Ambiguous(_) => Ty::Unknown,
                    Lookup::External | Lookup::Unknown => {
                        Ty::External(segments[segments.len() - 1].to_string(), 0)
                    }
                }
            }
        }
    }

    /// The body of an anonymous or local class, with `supers` in scope and
    /// its own fields as variables.
    pub(super) fn unnamed_body(&mut self, node: Node<'a>, supers: Vec<String>, opaque: bool) {
        let unnamed = self.unnamed.len();
        let locals = self.locals.len();
        let mut cursor = node.walk();
        let methods = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "method_declaration")
            .filter_map(|method| method.child_by_field_name("name"))
            .map(|name| node_text(name, self.source))
            .collect();
        self.unnamed_classes.push(Unnamed {
            start: unnamed,
            locals,
            opaque: opaque
                || supers
                    .iter()
                    .any(|fqn| self.resolver.has_external_superclass(fqn)),
            methods,
        });
        self.unnamed.extend(supers);
        let mut cursor = node.walk();
        for field in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "field_declaration")
        {
            let ty = field
                .child_by_field_name("type")
                .map_or(Ty::Unknown, |ty| self.node_ty(ty));
            let mut inner = field.walk();
            for declarator in field.children_by_field_name("declarator", &mut inner) {
                if let Some(name) = declarator.child_by_field_name("name") {
                    self.locals.push((node_text(name, self.source), ty.clone()));
                }
            }
        }
        self.children(node, false);
        self.locals.truncate(locals);
        self.unnamed_classes.pop();
        self.unnamed.truncate(unnamed);
    }

    /// A method or constructor with a row: the source of everything in it.
    fn member(&mut self, node: Node<'a>) {
        let Some((_, id)) = self.symbol_at(node) else {
            return;
        };
        let saved_src = self.src.replace(id);
        let saved_region = std::mem::replace(&mut self.region, scan_region(node, self.source));
        let locals = self.locals.len();
        self.children(node, false);
        self.locals.truncate(locals);
        self.region = saved_region;
        self.src = saved_src;
    }

    /// Type-level code (a field declaration, an initializer, an enum
    /// constant): the type is the source.
    fn type_region(&mut self, node: Node<'a>) {
        let saved_region = std::mem::replace(&mut self.region, scan_region(node, self.source));
        let locals = self.locals.len();
        self.children(node, false);
        self.locals.truncate(locals);
        self.region = saved_region;
    }

    /// `T a = x, b[] = y;`: each variable is in scope from its declarator
    /// on; `var` takes its initializer's type.
    fn local_variables(&mut self, node: Node<'a>) {
        let ty = node.child_by_field_name("type");
        let declared = ty
            .filter(|ty| node_text(*ty, self.source) != "var")
            .map(|ty| self.node_ty(ty));
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "variable_declarator" {
                self.visit(child, false);
                continue;
            }
            let value = child.child_by_field_name("value");
            let initial = match value {
                Some(value) if value.kind() != "array_initializer" => self.expr(value),
                Some(value) => {
                    self.visit(value, false);
                    Ty::Unknown
                }
                None => Ty::Unknown,
            };
            if let Some(name) = child.child_by_field_name("name") {
                let dims = dims(child, self.source);
                let ty = match &declared {
                    Some(ty) => with_dims(ty.clone(), dims),
                    None => initial,
                };
                self.locals.push((node_text(name, self.source), ty));
            }
        }
    }

    /// The name and type of a method, lambda or catch parameter.
    fn parameter(&self, node: Node<'a>) -> Option<(&'a str, Ty)> {
        match node.kind() {
            "spread_parameter" => {
                let mut cursor = node.walk();
                let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
                let ty = children
                    .iter()
                    .find(|child| !matches!(child.kind(), "modifiers" | "variable_declarator"))?;
                let declarator = children
                    .iter()
                    .find(|child| child.kind() == "variable_declarator")?;
                let name = declarator.child_by_field_name("name")?;
                Some((
                    node_text(name, self.source),
                    with_dims(self.node_ty(*ty), 1),
                ))
            }
            "catch_formal_parameter" => {
                let name = node.child_by_field_name("name")?;
                let mut cursor = node.walk();
                let catch_type = node
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "catch_type")?;
                let mut inner = catch_type.walk();
                let types: Vec<Node<'a>> = catch_type.named_children(&mut inner).collect();
                let ty = match types.as_slice() {
                    [ty] => self.node_ty(*ty),
                    _ => Ty::Unknown,
                };
                Some((node_text(name, self.source), ty))
            }
            _ => {
                let name = node.child_by_field_name("name")?;
                let dims = dims(node, self.source);
                let ty = node
                    .child_by_field_name("type")
                    .map_or(Ty::Unknown, |ty| with_dims(self.node_ty(ty), dims));
                Some((node_text(name, self.source), ty))
            }
        }
    }

    /// `if`, `while`, `c ? a : b`: a pattern variable of the condition is
    /// in scope up to the end of the then-branch (or the body); Java's
    /// flow scoping into the `else` branch or after an `if` that cannot
    /// complete is not followed.
    fn conditional(&mut self, node: Node<'a>) {
        let locals = self.locals.len();
        let alternative = node.child_by_field_name("alternative");
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == alternative {
                self.locals.truncate(locals);
            }
            self.visit(child, false);
        }
        self.locals.truncate(locals);
    }

    /// `for (T x : xs)`: `x` is in scope in the body only.
    fn enhanced_for(&mut self, node: Node<'a>) {
        let locals = self.locals.len();
        let mut declared = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == node.child_by_field_name("type") {
                self.visit(child, false);
                if node_text(child, self.source) != "var" {
                    declared = Some(self.node_ty(child));
                }
            } else if Some(child) == node.child_by_field_name("value") {
                self.expr(child);
                if let Some(name) = node.child_by_field_name("name") {
                    self.locals.push((
                        node_text(name, self.source),
                        declared.take().unwrap_or(Ty::Unknown),
                    ));
                }
            } else if Some(child) != node.child_by_field_name("name") {
                self.visit(child, false);
            }
        }
        self.locals.truncate(locals);
    }

    /// A try-with-resources resource: a declaration or an existing variable.
    fn resource(&mut self, node: Node<'a>) {
        let Some(name) = node.child_by_field_name("name") else {
            self.children(node, false);
            return;
        };
        let ty = node.child_by_field_name("type");
        if let Some(ty) = ty {
            self.visit(ty, false);
        }
        let initial = node
            .child_by_field_name("value")
            .map_or(Ty::Unknown, |value| self.expr(value));
        let ty = match ty {
            Some(ty) if node_text(ty, self.source) != "var" => self.node_ty(ty),
            _ => initial,
        };
        self.locals.push((node_text(name, self.source), ty));
    }

    /// The types of a `superclass`, `super_interfaces` or
    /// `extends_interfaces` clause, as `kind`; their generic arguments as
    /// references.
    fn header(&mut self, node: Node<'a>, kind: EdgeKind) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_list" {
                self.header(child, kind);
            } else {
                self.emit_type(child, kind);
            }
        }
    }

    /// A type node whose outer type is a usage of `kind`; generic
    /// arguments and annotations inside it are visited as usual.
    pub(super) fn emit_type(&mut self, node: Node<'a>, kind: EdgeKind) {
        match node.kind() {
            "type_identifier" => self.type_identifier(node, kind),
            "scoped_type_identifier" => self.scoped_type(node, kind),
            "generic_type" | "annotated_type" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if matches!(
                        child.kind(),
                        "type_identifier"
                            | "scoped_type_identifier"
                            | "generic_type"
                            | "annotated_type"
                    ) {
                        self.emit_type(child, kind);
                    } else {
                        self.visit(child, false);
                    }
                }
            }
            _ => self.visit(node, false),
        }
    }

    fn type_identifier(&mut self, node: Node<'a>, kind: EdgeKind) {
        if node
            .parent()
            .is_some_and(|parent| parent.kind() == "type_parameter")
        {
            return;
        }
        let name = node_text(node, self.source);
        if !self.is_type_name(name) {
            return;
        }
        let lookup = self.resolver.resolve_simple(self.file, self.scope(), name);
        self.record(lookup, name, kind, line(node));
    }

    /// A qualified type (`Outer.Inner`, `com.acme.Foo`); the generic
    /// arguments of a qualifying type are visited as references.
    fn scoped_type(&mut self, node: Node<'a>, kind: EdgeKind) {
        let mut segments = Vec::new();
        self.scoped_segments(node, &mut segments);
        let Some(first) = segments.first() else {
            return;
        };
        if !self.is_type_name(first) {
            return;
        }
        let lookup = self
            .resolver
            .resolve_type(self.file, self.scope(), &segments);
        self.record(lookup, &segments.join("."), kind, line(node));
    }

    fn scoped_segments(&mut self, node: Node<'a>, out: &mut Vec<&'a str>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "type_identifier" => out.push(node_text(child, self.source)),
                "scoped_type_identifier" | "generic_type" => self.scoped_segments(child, out),
                _ => self.visit(child, false),
            }
        }
    }

    /// An annotation's type and its arguments.
    fn annotation(&mut self, node: Node<'a>) {
        if let Some(name) = node.child_by_field_name("name") {
            let written = node_text(name, self.source);
            let segments: Vec<&str> = written.split('.').map(str::trim).collect();
            if self.is_type_name(segments[0]) {
                let lookup = self
                    .resolver
                    .resolve_type(self.file, self.scope(), &segments);
                self.record(lookup, &segments.join("."), EdgeKind::Reference, line(node));
            }
        }
        if let Some(arguments) = node.child_by_field_name("arguments") {
            self.visit(arguments, false);
        }
    }

    pub(super) fn record(&mut self, lookup: Lookup, written: &str, kind: EdgeKind, line: usize) {
        match lookup {
            Lookup::Found(fqn) => self.emit(&fqn, kind, line),
            Lookup::External | Lookup::Unknown => {
                *self
                    .usages
                    .unresolved
                    .entry(written.to_string())
                    .or_default() += 1;
            }
            Lookup::Ambiguous(candidates) => tracing::info!(
                name = written,
                candidates = candidates.join(", "),
                line,
                "type name matches more than one wildcard import; no edge"
            ),
        }
    }

    /// An edge from the current source to the type `fqn`; none outside a
    /// symbol or to the source itself.
    pub(super) fn emit(&mut self, fqn: &str, kind: EdgeKind, line: usize) {
        if let Some(dst) = self.resolver.id(fqn) {
            self.push(dst, kind, line, false);
        }
    }

    /// An edge from the current source to the row `dst`; none outside a
    /// symbol or to the source itself (recursion).
    pub(super) fn push(&mut self, dst: i64, kind: EdgeKind, line: usize, ambiguous: bool) {
        let Some(src) = self.src else {
            return;
        };
        if src != dst {
            self.usages.edges.push(Edge {
                src,
                dst,
                kind,
                line,
                ambiguous,
            });
        }
    }
}

/// The type variables and local types `root` declares anywhere inside it;
/// see [`Region`].
fn scan_region(root: Node<'_>, source: &str) -> Region {
    let mut region = Region::default();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        match node.kind() {
            "type_parameter" => {
                if let Some(name) = node
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "type_identifier")
                {
                    region.type_vars.insert(node_text(name, source).to_string());
                }
            }
            kind if type_kind(kind).is_some() && node != root => {
                if let Some(name) = node.child_by_field_name("name") {
                    region.types.insert(node_text(name, source).to_string());
                }
            }
            _ => {}
        }
        stack.extend(node.children(&mut cursor));
    }
    region
}

/// Whether a node is an expression [`FileWalk::expr`] types (a bare
/// `identifier` aside, which depends on where it stands).
fn is_typed_expression(kind: &str) -> bool {
    matches!(
        kind,
        "method_invocation"
            | "field_access"
            | "object_creation_expression"
            | "method_reference"
            | "lambda_expression"
            | "cast_expression"
            | "parenthesized_expression"
            | "array_access"
    )
}

/// The identifiers of a name in an expression (`a`, `a.b.c`), or `None`
/// when it is anything else (a call, `this`, an array access).
pub(super) fn name_chain<'a>(node: Node<'_>, source: &'a str) -> Option<Vec<&'a str>> {
    match node.kind() {
        "identifier" => Some(vec![node_text(node, source)]),
        "field_access" => {
            let field = node.child_by_field_name("field")?;
            if field.kind() != "identifier" {
                return None;
            }
            let mut chain = name_chain(node.child_by_field_name("object")?, source)?;
            chain.push(node_text(field, source));
            Some(chain)
        }
        _ => None,
    }
}

/// Whether an `identifier` stands for a value in an expression (a `case`
/// label included), rather than declaring something, naming a member, a
/// label or an annotation element.
fn is_expression_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let is_field = |field: &str| parent.child_by_field_name(field) == Some(node);
    match parent.kind() {
        "argument_list"
        | "binary_expression"
        | "unary_expression"
        | "update_expression"
        | "parenthesized_expression"
        | "return_statement"
        | "array_access"
        | "ternary_expression"
        | "array_initializer"
        | "element_value_array_initializer"
        | "annotation_argument_list"
        | "yield_statement"
        | "throw_statement"
        | "assert_statement"
        | "expression_statement"
        | "assignment_expression"
        | "dimensions_expr"
        | "switch_label"
        | "guard" => true,
        "variable_declarator" | "element_value_pair" | "cast_expression" => is_field("value"),
        "instanceof_expression" => is_field("left"),
        "enhanced_for_statement" => is_field("value"),
        "lambda_expression" => is_field("body"),
        _ => false,
    }
}

/// The type nodes of a `superclass`, `super_interfaces` or
/// `extends_interfaces` clause.
fn collect_header_types<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_list" {
            collect_header_types(child, out);
        } else {
            out.push(child);
        }
    }
}

/// The dotted name of a type node without generic arguments or
/// annotations; false when it is not a class or interface type.
pub(super) fn type_segments<'a>(node: Node<'_>, source: &'a str, out: &mut Vec<&'a str>) -> bool {
    match node.kind() {
        "type_identifier" => {
            out.push(node_text(node, source));
            true
        }
        "scoped_type_identifier" | "generic_type" | "annotated_type" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .filter(|child| {
                    matches!(
                        child.kind(),
                        "type_identifier"
                            | "scoped_type_identifier"
                            | "generic_type"
                            | "annotated_type"
                    )
                })
                .all(|child| type_segments(child, source, out))
        }
        _ => false,
    }
}

/// The array dimensions a node's `dimensions` field adds (`[]`, `[][]`).
fn dims(node: Node<'_>, source: &str) -> usize {
    node.child_by_field_name("dimensions")
        .map_or(0, |dims| node_text(dims, source).matches('[').count())
}

pub(super) fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

pub(super) fn line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

#[cfg(test)]
mod tests {
    use crate::usages::tests::{calls, edges};

    #[test]
    fn header_edges_are_extends_and_implements_from_the_type() {
        let edges = edges(&[
            (
                "Types.java",
                "\
package a;
class Base<T> {}
interface Api {}
interface Wide extends Api {}
class Msg {}
enum Kind implements Api { A }
record Point(int x) implements Api {}
class Impl extends Base<Msg> implements Api, Wide {}
",
            ),
            (
                "Other.java",
                "package a;\nclass Other extends a.Base<String> {}\n",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "a.Impl -extends-> a.Base :8",
                "a.Impl -implements-> a.Api :8",
                "a.Impl -implements-> a.Wide :8",
                "a.Impl -reference-> a.Msg :8",
                "a.Kind -implements-> a.Api :6",
                "a.Other -extends-> a.Base :2",
                "a.Point -implements-> a.Api :7",
                "a.Wide -extends-> a.Api :4",
            ]
        );
    }

    #[test]
    fn each_reference_construct_is_an_edge_from_the_innermost_member() {
        let edges = edges(&[
            (
                "Use.java",
                "\
package a;

import static a.Limits.MAX;

@Marker(type = Ann.class, mode = Mode.FAST)
class Use {
    private Field field = new Made();
    static { Init.run(); }

    @Marker
    Ret run(Param p, Var... rest) throws Thrown {
        Local local = (Cast) p;
        if (p instanceof Check c) {}
        try {} catch (Caught | Other e) {}
        java.util.List<Generic> list = null;
        Object[] array = new Arr[MAX];
        Class<?> type = Lit.class;
        Runnable r = Ref::go;
        java.util.function.Supplier<Object> s = Made::new;
        Const.VALUE.toString();
        Statics.call();
        new Anon() {};
        return null;
    }
}
",
            ),
            (
                "Types.java",
                "\
package a;
@interface Marker { Class<?> type(); Mode mode(); }
class Ann {} enum Mode { FAST }
class Field {} class Made {} class Init { static void run() {} }
class Ret {} class Param {} class Var {} class Thrown extends Exception {}
class Local {} class Cast {} class Check {} class Caught extends Exception {}
class Other extends Exception {} class Generic {} class Arr {} class Lit {}
class Ref { static void go() {} } class Const { static Object VALUE; }
class Statics { static void call() {} } class Anon {}
class Limits { static final int MAX = 1; }
",
            ),
        ]);
        let run = "a.Use#run(Param,Var...)";
        let from_run: Vec<&str> = edges
            .iter()
            .filter_map(|edge| edge.strip_prefix(run))
            .collect();
        assert_eq!(
            from_run,
            [
                " -call-> a.Ref#go() :18",
                " -call-> a.Statics#call() :21",
                " -instantiate-> a.Anon :22",
                " -instantiate-> a.Made :19",
                " -reference-> a.Arr :16",
                " -reference-> a.Cast :12",
                " -reference-> a.Caught :14",
                " -reference-> a.Check :13",
                " -reference-> a.Const :20",
                " -reference-> a.Generic :15",
                " -reference-> a.Limits :16",
                " -reference-> a.Lit :17",
                " -reference-> a.Local :12",
                " -reference-> a.Marker :10",
                " -reference-> a.Other :14",
                " -reference-> a.Param :11",
                " -reference-> a.Ref :18",
                " -reference-> a.Ret :11",
                " -reference-> a.Statics :21",
                " -reference-> a.Thrown :11",
                " -reference-> a.Var :11",
            ]
        );
        let from_type: Vec<&str> = edges
            .iter()
            .filter_map(|edge| edge.strip_prefix("a.Use "))
            .collect();
        assert_eq!(
            from_type,
            [
                "-call-> a.Init#run() :8",
                "-instantiate-> a.Made :7",
                "-reference-> a.Ann :5",
                "-reference-> a.Field :7",
                "-reference-> a.Init :8",
                "-reference-> a.Marker :5",
                "-reference-> a.Mode :5",
            ]
        );
    }

    #[test]
    fn lambdas_anonymous_and_local_classes_belong_to_the_enclosing_member() {
        let edges = edges(&[(
            "A.java",
            "\
package a;
class Dep {}
class Dep2 {}
class Dep3 {}
enum Op {
    PLUS { Dep apply() { return null; } };
}
class A {
    void run() {
        Runnable r = () -> { Dep d = null; };
        new Object() { Dep2 field; };
        class Local { Dep3 x; }
    }
}
",
        )]);
        assert_eq!(
            edges,
            [
                "a.A#run() -reference-> a.Dep :10",
                "a.A#run() -reference-> a.Dep2 :11",
                "a.A#run() -reference-> a.Dep3 :12",
                "a.Op -reference-> a.Dep :6",
            ]
        );
    }

    #[test]
    fn type_variables_local_classes_and_variables_are_not_types() {
        let edges = edges(&[
            (
                "a/T.java",
                "package a;\nclass T { static int X; }\nclass E {}\nclass Local {}\nclass Helper { static void go() {} }\n",
            ),
            (
                "a/Box.java",
                "\
package a;
class Box<T> {
    T value;
    <E> E first() { return null; }
    Helper Helper;
    void run() {
        class Local {}
        Local local = new Local();
        Helper.go();
        int T = 1;
    }
}
",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "a.Box -reference-> a.Helper :5",
                "a.Box#run() -call-> a.Helper#go() :9",
            ]
        );
    }

    #[test]
    fn imports_javadoc_and_self_references_are_not_usages() {
        let edges = edges(&[
            ("a/Dep.java", "package a;\npublic class Dep {}\n"),
            (
                "b/B.java",
                "\
package b;
import a.Dep;
/** Uses {@link Dep}. */
class B {
    B next;
    B copy() { return new B(); }
}
",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "b.B#copy() -instantiate-> b.B :6",
                "b.B#copy() -reference-> b.B :6",
            ]
        );
    }

    #[test]
    fn locals_are_block_scoped_and_shadow_fields() {
        let calls = calls(&[(
            "a/U.java",
            "\
package a;
class A { void a() {} }
class B { void b() {} }
class U {
    A value;
    void run() {
        {
            B value = null;
            value.b();
        }
        value.a();
        for (B each : new B[0]) { each.b(); }
        try (B res = null) { res.b(); } catch (RuntimeException e) {}
        Object o = null;
        if (o instanceof B typed) { typed.b(); }
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#run() -call-> a.A#a() :11",
                "a.U#run() -call-> a.B#b() :12",
                "a.U#run() -call-> a.B#b() :13",
                "a.U#run() -call-> a.B#b() :15",
                "a.U#run() -call-> a.B#b() :9",
            ]
        );
    }

    #[test]
    fn a_pattern_variable_is_scoped_to_its_condition_and_then_branch() {
        let calls = calls(&[(
            "a/P.java",
            "\
package a;
class Foo { void go() {} }
class Bar { void go() {} }
class P {
    Bar value;
    void f(Object o) {
        if (o instanceof Foo value) { value.go(); } else { value.go(); }
        value.go();
        while (o instanceof Foo value) { value.go(); }
        Object x = o instanceof Foo value ? value : value;
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.P#f(Object) -call-> a.Bar#go() :7",
                "a.P#f(Object) -call-> a.Bar#go() :8",
                "a.P#f(Object) -call-> a.Foo#go() :7",
                "a.P#f(Object) -call-> a.Foo#go() :9",
            ]
        );
    }

    #[test]
    fn an_anonymous_class_sees_its_inherited_fields_before_outer_locals() {
        let calls = calls(&[(
            "a/S.java",
            "\
package a;
class Svc { void go() {} }
class Other { void go() {} }
abstract class Base { protected Svc svc; abstract void run(); }
class U {
    void f(Other svc) {
        new Base() { void run() { svc.go(); } };
        svc.go();
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#f(Other) -call-> a.Other#go() :8",
                "a.U#f(Other) -call-> a.Svc#go() :7",
            ]
        );
    }
}
