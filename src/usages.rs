//! Usages between indexed symbols, resolved from the parsed trees.
//!
//! [`resolve`] is a second pass over every file of a run: it needs the whole
//! run's symbol table, because a name is turned into an fqn by Java's own
//! lookup rules before it is looked up, never matched by its simple name
//! across the repository. A simple type name resolves, in `javac`'s order, to
//! a type nested in the super types of the anonymous and local classes
//! around it, in the current type or a type around it (nested types
//! inherited from an indexed super type included), then a single-type
//! import, a type in the same package, a wildcard import. A type's header
//! and annotations resolve around the type, its body inside it. A qualified name
//! resolves its first part that way, or as a package, and walks down. A name
//! that does not reach an indexed type makes no edge and is counted.
//!
//! Every mention becomes an [`Edge`] from the innermost method or
//! constructor around it: code in lambdas, anonymous and local classes
//! belongs to that member, while field declarations, initializer blocks,
//! enum constants, the class header and class annotations belong to the
//! type. Imports, Javadoc, type variables and a symbol naming itself are not
//! usages.
//!
//! This step resolves types only (`extends`, `implements`, `instantiate`,
//! `reference`). Names in expressions resolve as far as they decide whether a
//! qualifier is a type: a local variable, parameter or field (of the current
//! type, its indexed super types or an enclosing type) shadows a type of the
//! same name, as in Java. Variables are tracked per member (or per field
//! declaration, initializer and enum constant), not per block.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Tree};

use crate::symbols::{Import, Symbol, TypeFacts, is_type_container, member_kind, type_kind};

/// The kind of a usage, stored in `edges.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    /// A class's superclass, or an interface's super interface.
    Extends,
    /// An interface in a class's, enum's or record's `implements` list.
    Implements,
    /// `new T(..)`, `new T(..) { .. }` and `T::new`, to the type.
    Instantiate,
    /// Every other mention of a type.
    Reference,
}

impl EdgeKind {
    /// The stable lowercase text stored in `edges.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::Instantiate => "instantiate",
            EdgeKind::Reference => "reference",
        }
    }
}

/// One usage: the symbol row `src` names the symbol row `dst` on `line`
/// (1-based, in `src`'s file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub src: i64,
    pub dst: i64,
    pub kind: EdgeKind,
    pub line: usize,
}

/// One parsed file of the run, with the symbol rows it wrote.
pub struct SourceFile<'a> {
    pub source: &'a str,
    pub tree: &'a Tree,
    pub package: &'a str,
    pub imports: &'a [Import],
    pub types: &'a [TypeFacts],
    /// The file's symbols that have a row, with its id. A symbol left out
    /// (a duplicate fqn) is neither a source nor a target of usages.
    pub symbols: Vec<(&'a Symbol, i64)>,
}

/// The result of [`resolve`].
#[derive(Debug, Default)]
pub struct Usages {
    /// The edges in file and source order; the same edge may repeat (two
    /// mentions on one line).
    pub edges: Vec<Edge>,
    /// Mentions of type names that do not resolve to an indexed type (JDK,
    /// libraries, names the run does not know), by the name as written.
    pub unresolved: HashMap<String, usize>,
}

/// Resolve the type usages of every file against the symbols of all of them.
pub fn resolve<'a>(files: &'a [SourceFile<'a>]) -> Usages {
    let resolver = Resolver::new(files);
    let mut usages = Usages::default();
    for (index, file) in files.iter().enumerate() {
        let mut walk = FileWalk {
            resolver: &resolver,
            file: index,
            source: file.source,
            spans: file
                .symbols
                .iter()
                .map(|&(symbol, id)| (symbol.start_byte, (symbol, id)))
                .collect(),
            enclosing: Vec::new(),
            unnamed: Vec::new(),
            type_vars: Vec::new(),
            src: None,
            region: Region::default(),
            usages: &mut usages,
        };
        walk.children(file.tree.root_node(), true);
    }
    usages
}

/// An indexed type: its row, facts and file.
struct TypeEntry<'a> {
    id: i64,
    facts: &'a TypeFacts,
    file: usize,
}

/// Where a name is looked up: the super types of the anonymous and local
/// classes around it (innermost last), then the named type around it.
#[derive(Clone, Copy)]
struct Scope<'s> {
    unnamed: &'s [String],
    enclosing: Option<&'s str>,
}

impl<'s> Scope<'s> {
    /// The scope of a named type's header and annotations, or of a super
    /// type resolved for the lookup chain: the type around it.
    fn around(enclosing: Option<&'s str>) -> Self {
        Self {
            unnamed: &[],
            enclosing,
        }
    }
}

/// Which members [`Resolver::has_field`] looks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldFilter {
    /// Fields, enum constants and record components.
    Any,
    /// Static fields and enum constants.
    Statics,
    /// Static fields only.
    StaticFields,
}

/// The outcome of looking a type name up.
enum Lookup {
    Found(String),
    /// A single-type import names a type outside the index.
    External,
    /// Nothing in scope: `java.lang`, a library type or an unknown name.
    Unknown,
    /// More than one wildcard import holds an indexed type of that name.
    Ambiguous(Vec<String>),
}

/// The run's type table and the lookups on it.
struct Resolver<'a> {
    files: &'a [SourceFile<'a>],
    types: HashMap<&'a str, TypeEntry<'a>>,
    supertypes: RefCell<HashMap<String, Vec<String>>>,
    pending: RefCell<HashSet<String>>,
}

impl<'a> Resolver<'a> {
    /// The table of every type with a row; for a duplicate fqn, the facts of
    /// the definition that was written.
    fn new(files: &'a [SourceFile<'a>]) -> Self {
        let mut types = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            let ids: HashMap<&str, i64> = file
                .symbols
                .iter()
                .filter(|(symbol, _)| symbol.kind.is_type())
                .map(|(symbol, id)| (symbol.fqn.as_str(), *id))
                .collect();
            for facts in file.types {
                if let Some(&id) = ids.get(facts.fqn.as_str()) {
                    types.entry(facts.fqn.as_str()).or_insert(TypeEntry {
                        id,
                        facts,
                        file: index,
                    });
                }
            }
        }
        Self {
            files,
            types,
            supertypes: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashSet::new()),
        }
    }

    fn id(&self, fqn: &str) -> Option<i64> {
        self.types.get(fqn).map(|entry| entry.id)
    }

    /// A simple type name in `file`, in `scope`, in `javac`'s order.
    fn resolve_simple(&self, file: usize, scope: Scope<'_>, name: &str) -> Lookup {
        if let Some(found) = scope
            .unnamed
            .iter()
            .rev()
            .find_map(|supertype| self.member_type(supertype, name))
        {
            return Lookup::Found(found);
        }
        let mut current = scope.enclosing;
        while let Some(owner) = current {
            if let Some(found) = self.member_type(owner, name) {
                return Lookup::Found(found);
            }
            current = self
                .types
                .get(owner)
                .and_then(|entry| entry.facts.parent.as_deref());
        }
        let source = &self.files[file];
        for import in source.imports.iter().filter(|import| !import.wildcard) {
            if import.path.rsplit('.').next() != Some(name) {
                continue;
            }
            if self.types.contains_key(import.path.as_str()) {
                return Lookup::Found(import.path.clone());
            }
            if !import.is_static {
                return Lookup::External;
            }
        }
        let same_package = if source.package.is_empty() {
            name.to_string()
        } else {
            format!("{}.{name}", source.package)
        };
        if self.types.contains_key(same_package.as_str()) {
            return Lookup::Found(same_package);
        }
        let mut candidates: Vec<String> = source
            .imports
            .iter()
            .filter(|import| import.wildcard)
            .map(|import| format!("{}.{name}", import.path))
            .filter(|fqn| self.types.contains_key(fqn.as_str()))
            .collect();
        candidates.sort();
        candidates.dedup();
        match candidates.len() {
            0 => Lookup::Unknown,
            1 => Lookup::Found(candidates.remove(0)),
            _ => Lookup::Ambiguous(candidates),
        }
    }

    /// The longest prefix of a dotted name that is an indexed type, and how
    /// many segments it took: the first segment as a simple name, or else
    /// the shortest package-qualified prefix, then nested types down.
    fn resolve_prefix(&self, file: usize, scope: Scope<'_>, segments: &[&str]) -> (Lookup, usize) {
        let Some(first) = segments.first() else {
            return (Lookup::Unknown, 0);
        };
        let (mut fqn, mut used) = match self.resolve_simple(file, scope, first) {
            Lookup::Found(fqn) => (fqn, 1),
            Lookup::Unknown => {
                let qualified = (2..=segments.len()).find_map(|count| {
                    let fqn = segments[..count].join(".");
                    self.types
                        .contains_key(fqn.as_str())
                        .then_some((fqn, count))
                });
                match qualified {
                    Some(found) => found,
                    None => return (Lookup::Unknown, 0),
                }
            }
            other => return (other, 1),
        };
        for segment in &segments[used..] {
            match self.member_type(&fqn, segment) {
                Some(next) => {
                    fqn = next;
                    used += 1;
                }
                None => break,
            }
        }
        (Lookup::Found(fqn), used)
    }

    /// A dotted type name that must resolve whole.
    fn resolve_type(&self, file: usize, scope: Scope<'_>, segments: &[&str]) -> Lookup {
        match self.resolve_prefix(file, scope, segments) {
            (Lookup::Found(fqn), used) if used == segments.len() => Lookup::Found(fqn),
            (Lookup::Found(_), _) => Lookup::Unknown,
            (other, _) => other,
        }
    }

    /// The type `name` nested in `owner` or inherited from its indexed
    /// super types.
    fn member_type(&self, owner: &str, name: &str) -> Option<String> {
        self.member_type_in(owner, name, &mut HashSet::new())
    }

    fn member_type_in(
        &self,
        owner: &str,
        name: &str,
        seen: &mut HashSet<String>,
    ) -> Option<String> {
        if !seen.insert(owner.to_string()) {
            return None;
        }
        let fqn = format!("{owner}.{name}");
        if self.types.contains_key(fqn.as_str()) {
            return Some(fqn);
        }
        self.supertypes(owner)
            .iter()
            .find_map(|supertype| self.member_type_in(supertype, name, seen))
    }

    /// The indexed superclass and interfaces of `fqn`, resolved in its own
    /// file around its enclosing type; memoized, and empty on a cycle.
    fn supertypes(&self, fqn: &str) -> Vec<String> {
        if let Some(found) = self.supertypes.borrow().get(fqn) {
            return found.clone();
        }
        let Some(entry) = self.types.get(fqn) else {
            return Vec::new();
        };
        if !self.pending.borrow_mut().insert(fqn.to_string()) {
            return Vec::new();
        }
        let facts = entry.facts;
        let resolved: Vec<String> = facts
            .superclass
            .iter()
            .chain(&facts.interfaces)
            .filter(|ty| !facts.type_params.contains(&ty.name))
            .filter_map(|ty| {
                let segments: Vec<&str> = ty.name.split('.').collect();
                match self.resolve_type(
                    entry.file,
                    Scope::around(facts.parent.as_deref()),
                    &segments,
                ) {
                    Lookup::Found(fqn) => Some(fqn),
                    _ => None,
                }
            })
            .collect();
        self.pending.borrow_mut().remove(fqn);
        self.supertypes
            .borrow_mut()
            .insert(fqn.to_string(), resolved.clone());
        resolved
    }

    /// Whether `owner` or one of its indexed super types has the field (or
    /// enum constant, or record component) `name`, of the kind `fields`.
    fn has_field(&self, owner: &str, name: &str, fields: FieldFilter) -> bool {
        self.has_field_in(owner, name, fields, &mut HashSet::new())
    }

    fn has_field_in(
        &self,
        owner: &str,
        name: &str,
        fields: FieldFilter,
        seen: &mut HashSet<String>,
    ) -> bool {
        if !seen.insert(owner.to_string()) {
            return false;
        }
        let Some(entry) = self.types.get(owner) else {
            return false;
        };
        let facts = entry.facts;
        let own = facts
            .fields
            .iter()
            .any(|field| field.name == name && (field.is_static || fields == FieldFilter::Any))
            || (fields != FieldFilter::StaticFields
                && facts.enum_constants.iter().any(|constant| constant == name))
            || (fields == FieldFilter::Any
                && facts
                    .record_components
                    .iter()
                    .any(|component| component.name == name));
        own || self
            .supertypes(owner)
            .iter()
            .any(|supertype| self.has_field_in(supertype, name, fields, seen))
    }

    /// The indexed type whose static member `name` (of the kind `fields`) a
    /// static import of `file` brings in: a single static import first,
    /// else exactly one static wildcard import.
    fn static_import_owner(&self, file: usize, name: &str, fields: FieldFilter) -> Option<String> {
        let imports = self.files[file].imports;
        let mut named = false;
        for import in imports
            .iter()
            .filter(|import| import.is_static && !import.wildcard)
        {
            let Some((owner, member)) = import.path.rsplit_once('.') else {
                continue;
            };
            if member != name {
                continue;
            }
            named = true;
            if self.types.contains_key(owner) && self.has_field(owner, name, fields) {
                return Some(owner.to_string());
            }
        }
        if named {
            return None;
        }
        let mut owners = imports
            .iter()
            .filter(|import| import.is_static && import.wildcard)
            .filter(|import| {
                self.types.contains_key(import.path.as_str())
                    && self.has_field(&import.path, name, fields)
            });
        match (owners.next(), owners.next()) {
            (Some(owner), None) => Some(owner.path.clone()),
            _ => None,
        }
    }
}

/// The names declared in one member, field declaration, initializer or enum
/// constant, whatever their block: variables (shadowing a type in a
/// qualifier), local and anonymous-class-local types and type variables.
#[derive(Debug, Default)]
struct Region {
    vars: HashSet<String>,
    types: HashSet<String>,
    type_vars: HashSet<String>,
}

/// The walk over one file's tree that turns its mentions into edges.
struct FileWalk<'r, 'a> {
    resolver: &'r Resolver<'a>,
    file: usize,
    source: &'a str,
    /// The file's written symbols by their declaration's start byte.
    spans: HashMap<usize, (&'a Symbol, i64)>,
    /// The fqns of the named types around the current node, innermost last.
    enclosing: Vec<&'a str>,
    /// The indexed super types of the anonymous and local classes around the
    /// current node, innermost last: their nested types are in scope.
    unnamed: Vec<String>,
    /// The type variables of those types.
    type_vars: Vec<String>,
    /// The edge source: the innermost member or type with a row.
    src: Option<i64>,
    region: Region,
    usages: &'r mut Usages,
}

impl<'a> FileWalk<'_, 'a> {
    fn children(&mut self, node: Node<'a>, named: bool) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, named);
        }
    }

    /// Visit `node`. `named` is true directly in the file or a named type's
    /// body, where declarations are symbols.
    fn visit(&mut self, node: Node<'a>, named: bool) {
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
        match kind {
            "import_declaration" | "package_declaration" | "line_comment" | "block_comment" => {}
            "type_identifier" => self.type_identifier(node, EdgeKind::Reference),
            "scoped_type_identifier" => self.scoped_type(node, EdgeKind::Reference),
            "marker_annotation" | "annotation" => self.annotation(node),
            "object_creation_expression" => self.object_creation(node),
            kind if type_kind(kind).is_some() => self.local_type(node),
            "method_reference" => self.method_reference(node),
            "method_invocation" => self.method_invocation(node),
            "field_access" => self.field_access(node),
            "identifier" => self.identifier(node),
            _ => self.children(node, named && is_type_container(kind)),
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
    fn scope(&self) -> Scope<'_> {
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
        self.region = saved_region;
        self.src = saved_src;
    }

    /// A local class (or interface, enum, record): its header is a
    /// `reference` from the member around it, and the nested types of its
    /// indexed super types are in scope in its body.
    fn local_type(&mut self, node: Node<'a>) {
        let supers = self.super_types(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if is_type_container(child.kind()) {
                self.unnamed_body(child, supers.clone());
            } else {
                self.visit(child, false);
            }
        }
    }

    /// The indexed types a local class's header names.
    fn super_types(&self, node: Node<'a>) -> Vec<String> {
        let mut supers = Vec::new();
        let mut cursor = node.walk();
        for clause in node.children(&mut cursor).filter(|child| {
            matches!(
                child.kind(),
                "superclass" | "super_interfaces" | "extends_interfaces"
            )
        }) {
            let mut types = Vec::new();
            collect_header_types(clause, &mut types);
            supers.extend(types.into_iter().filter_map(|ty| self.type_fqn(ty)));
        }
        supers
    }

    /// The indexed type a type node names, without its generic arguments.
    fn type_fqn(&self, node: Node<'a>) -> Option<String> {
        let mut segments = Vec::new();
        type_segments(node, self.source, &mut segments).then_some(())?;
        if !self.is_type_name(segments.first()?) {
            return None;
        }
        match self
            .resolver
            .resolve_type(self.file, self.scope(), &segments)
        {
            Lookup::Found(fqn) => Some(fqn),
            _ => None,
        }
    }

    /// The body of an anonymous or local class, with `supers` in scope.
    fn unnamed_body(&mut self, node: Node<'a>, supers: Vec<String>) {
        let unnamed = self.unnamed.len();
        self.unnamed.extend(supers);
        self.children(node, false);
        self.unnamed.truncate(unnamed);
    }

    /// A method or constructor with a row: the source of everything in it.
    fn member(&mut self, node: Node<'a>) {
        let Some((_, id)) = self.symbol_at(node) else {
            return;
        };
        let saved_src = self.src.replace(id);
        let saved_region = std::mem::replace(&mut self.region, scan_region(node, self.source));
        self.children(node, false);
        self.region = saved_region;
        self.src = saved_src;
    }

    /// Type-level code (a field declaration, an initializer, an enum
    /// constant): the type is the source.
    fn type_region(&mut self, node: Node<'a>) {
        let saved_region = std::mem::replace(&mut self.region, scan_region(node, self.source));
        self.children(node, false);
        self.region = saved_region;
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
    fn emit_type(&mut self, node: Node<'a>, kind: EdgeKind) {
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

    /// `new T(..)`, with or without a body: `instantiate` to `T`; in the
    /// body, `T`'s nested types are in scope. In `outer.new Inner()` the
    /// type is a member of `outer`'s type, unknown here: no edge, counted.
    fn object_creation(&mut self, node: Node<'a>) {
        let ty = node.child_by_field_name("type");
        let qualified = node.child(0).is_some_and(|first| first.kind() != "new");
        let supers: Vec<String> = match ty {
            Some(ty) if !qualified => self.type_fqn(ty).into_iter().collect(),
            _ => Vec::new(),
        };
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == ty {
                if qualified {
                    self.qualified_creation(child);
                } else {
                    self.emit_type(child, EdgeKind::Instantiate);
                }
            } else if child.kind() == "class_body" {
                self.unnamed_body(child, supers.clone());
            } else {
                self.visit(child, false);
            }
        }
    }

    /// The type of `outer.new Inner<A>()`: counted as unresolved, its
    /// generic arguments visited.
    fn qualified_creation(&mut self, node: Node<'a>) {
        let mut segments = Vec::new();
        type_segments(node, self.source, &mut segments);
        *self
            .usages
            .unresolved
            .entry(segments.join("."))
            .or_default() += 1;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_arguments" {
                self.visit(child, false);
            }
        }
    }

    /// `T::m` (a reference to `T`) and `T::new` (`instantiate`); the
    /// method name is not a type.
    fn method_reference(&mut self, node: Node<'a>) {
        let mut cursor = node.walk();
        let children: Vec<Node<'a>> = node.children(&mut cursor).collect();
        let Some((&target, rest)) = children.split_first() else {
            return;
        };
        let constructor = rest.iter().any(|child| child.kind() == "new");
        let kind = if constructor {
            EdgeKind::Instantiate
        } else {
            EdgeKind::Reference
        };
        match name_chain(target, self.source) {
            Some(segments) => self.qualifier(&segments, kind, line(target)),
            None if constructor => self.emit_type(target, kind),
            None => self.visit(target, false),
        }
        for child in rest {
            if child.kind() == "type_arguments" {
                self.visit(*child, false);
            }
        }
    }

    /// `x.m(..)` or `T.m(..)`: a qualifier that is a type is a reference.
    fn method_invocation(&mut self, node: Node<'a>) {
        if let Some(object) = node.child_by_field_name("object") {
            match name_chain(object, self.source) {
                Some(segments) => self.qualifier(&segments, EdgeKind::Reference, line(object)),
                None => self.visit(object, false),
            }
        }
        for field in ["type_arguments", "arguments"] {
            if let Some(child) = node.child_by_field_name(field) {
                self.visit(child, false);
            }
        }
    }

    /// `T.X`, `Outer.Inner.X`, `Outer.this`: a qualifier that is a type is
    /// a reference.
    fn field_access(&mut self, node: Node<'a>) {
        let Some(object) = node.child_by_field_name("object") else {
            return;
        };
        let field = node.child_by_field_name("field");
        if field.is_some_and(|field| matches!(field.kind(), "this" | "super")) {
            match name_chain(object, self.source) {
                Some(segments) => self.qualifier(&segments, EdgeKind::Reference, line(object)),
                None => self.visit(object, false),
            }
            return;
        }
        match name_chain(node, self.source) {
            Some(segments) => self.qualifier(
                &segments[..segments.len() - 1],
                EdgeKind::Reference,
                line(node),
            ),
            None => self.visit(object, false),
        }
    }

    /// A dotted name in an expression: a variable first, as in Java (a
    /// static-imported constant is a reference to its type); else its
    /// longest prefix that is a type.
    fn qualifier(&mut self, segments: &[&str], kind: EdgeKind, line: usize) {
        let Some(&first) = segments.first() else {
            return;
        };
        if self.is_variable(first) {
            return;
        }
        if let Some(owner) =
            self.resolver
                .static_import_owner(self.file, first, FieldFilter::Statics)
        {
            self.emit(&owner, EdgeKind::Reference, line);
            return;
        }
        if !self.is_type_name(first) {
            return;
        }
        let (lookup, _) = self
            .resolver
            .resolve_prefix(self.file, self.scope(), segments);
        match lookup {
            Lookup::Unknown if !first.starts_with(char::is_uppercase) => {}
            lookup => self.record(lookup, first, kind, line),
        }
    }

    /// A bare name in an expression: a constant brought in by a static
    /// import is a reference to its type. In a `case` label only a static
    /// field counts: in an enum switch the name is the selector's constant,
    /// whatever is imported.
    fn identifier(&mut self, node: Node<'a>) {
        if !is_expression_name(node) {
            return;
        }
        let name = node_text(node, self.source);
        if self.is_variable(name) {
            return;
        }
        let fields = if node
            .parent()
            .is_some_and(|parent| parent.kind() == "switch_label")
        {
            FieldFilter::StaticFields
        } else {
            FieldFilter::Statics
        };
        if let Some(owner) = self.resolver.static_import_owner(self.file, name, fields) {
            self.emit(&owner, EdgeKind::Reference, line(node));
        }
    }

    /// Whether `name` can be a type here: not `var`, a type variable or a
    /// local class.
    fn is_type_name(&self, name: &str) -> bool {
        name != "var"
            && !self.type_vars.iter().any(|var| var == name)
            && !self.region.type_vars.contains(name)
            && !self.region.types.contains(name)
    }

    /// Whether `name` is a variable here: declared in the current region or
    /// a field of an enclosing type or of an anonymous or local class's
    /// super type (inherited from an indexed super type included).
    fn is_variable(&self, name: &str) -> bool {
        self.region.vars.contains(name)
            || self
                .enclosing
                .iter()
                .copied()
                .chain(self.unnamed.iter().map(String::as_str))
                .any(|owner| self.resolver.has_field(owner, name, FieldFilter::Any))
    }

    fn record(&mut self, lookup: Lookup, written: &str, kind: EdgeKind, line: usize) {
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
    fn emit(&mut self, fqn: &str, kind: EdgeKind, line: usize) {
        let (Some(src), Some(dst)) = (self.src, self.resolver.id(fqn)) else {
            return;
        };
        if src != dst {
            self.usages.edges.push(Edge {
                src,
                dst,
                kind,
                line,
            });
        }
    }
}

/// The names `root` declares anywhere inside it; see [`Region`].
fn scan_region(root: Node<'_>, source: &str) -> Region {
    let mut region = Region::default();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        match node.kind() {
            "variable_declarator"
            | "formal_parameter"
            | "catch_formal_parameter"
            | "enhanced_for_statement"
            | "instanceof_expression"
            | "resource" => {
                if let Some(name) = node.child_by_field_name("name")
                    && name.kind() == "identifier"
                {
                    region.vars.insert(node_text(name, source).to_string());
                }
            }
            "lambda_expression" => {
                if let Some(name) = node.child_by_field_name("parameters")
                    && name.kind() == "identifier"
                {
                    region.vars.insert(node_text(name, source).to_string());
                }
            }
            "inferred_parameters" | "type_pattern" | "record_pattern_component" => {
                for child in node.named_children(&mut cursor) {
                    if child.kind() == "identifier" {
                        region.vars.insert(node_text(child, source).to_string());
                    }
                }
            }
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

/// The identifiers of a name in an expression (`a`, `a.b.c`), or `None`
/// when it is anything else (a call, `this`, an array access).
fn name_chain<'a>(node: Node<'_>, source: &'a str) -> Option<Vec<&'a str>> {
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
fn type_segments<'a>(node: Node<'_>, source: &'a str, out: &mut Vec<&'a str>) -> bool {
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

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::symbols::{JavaParser, ParsedFile};

    /// The edges of `files` (path, source) as `src -kind-> dst :line`,
    /// sorted, and the unresolved names. Every symbol gets a row.
    fn resolve_files(files: &[(&str, &str)]) -> (Vec<String>, HashMap<String, usize>) {
        let mut parser = JavaParser::new().unwrap();
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(path, source)| parser.parse(Path::new(path), source).unwrap())
            .collect();
        let mut next_id = 0;
        let mut fqns = HashMap::new();
        let sources: Vec<SourceFile<'_>> = parsed
            .iter()
            .zip(files)
            .map(|(file, (_, source))| SourceFile {
                source,
                tree: file.tree.as_ref().unwrap(),
                package: &file.package,
                imports: &file.imports,
                types: &file.types,
                symbols: file
                    .symbols
                    .iter()
                    .map(|symbol| {
                        next_id += 1;
                        fqns.insert(next_id, symbol.fqn.clone());
                        (symbol, next_id)
                    })
                    .collect(),
            })
            .collect();
        let usages = resolve(&sources);
        let mut edges: Vec<String> = usages
            .edges
            .iter()
            .map(|edge| {
                format!(
                    "{} -{}-> {} :{}",
                    fqns[&edge.src],
                    edge.kind.as_str(),
                    fqns[&edge.dst],
                    edge.line
                )
            })
            .collect();
        edges.sort();
        edges.dedup();
        (edges, usages.unresolved)
    }

    fn edges(files: &[(&str, &str)]) -> Vec<String> {
        resolve_files(files).0
    }

    #[test]
    fn kinds_round_trip() {
        assert_eq!(
            [
                EdgeKind::Extends,
                EdgeKind::Implements,
                EdgeKind::Instantiate,
                EdgeKind::Reference,
            ]
            .map(EdgeKind::as_str),
            ["extends", "implements", "instantiate", "reference"]
        );
    }

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
    fn nested_types_resolve_inside_out_and_by_qualified_name() {
        let edges = edges(&[
            (
                "Outer.java",
                "\
package a;
class Outer {
    static class Inner {
        Inner self() { return null; }
        Sibling sibling() { return null; }
    }
    static class Sibling {}
    Inner inner() { return null; }
}
",
            ),
            (
                "Child.java",
                "\
package b;
import a.Outer;
class Child extends Outer {
    Inner inherited() { return null; }
    Outer.Sibling qualified() { return null; }
    a.Outer.Inner full() { return null; }
}
",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "a.Outer#inner() -reference-> a.Outer.Inner :8",
                "a.Outer.Inner#self() -reference-> a.Outer.Inner :4",
                "a.Outer.Inner#sibling() -reference-> a.Outer.Sibling :5",
                "b.Child -extends-> a.Outer :3",
                "b.Child#full() -reference-> a.Outer.Inner :6",
                "b.Child#inherited() -reference-> a.Outer.Inner :4",
                "b.Child#qualified() -reference-> a.Outer.Sibling :5",
            ]
        );
    }

    #[test]
    fn the_same_simple_name_resolves_by_nested_type_import_and_package() {
        let bearer = |package: &str| {
            (
                format!("{package}/MessageBearer.java"),
                format!("package {package};\npublic class MessageBearer {{}}\n"),
            )
        };
        let role = bearer("role");
        let person = bearer("person");
        let user = bearer("user");
        let edges = edges(&[
            (role.0.as_str(), role.1.as_str()),
            (person.0.as_str(), person.1.as_str()),
            (user.0.as_str(), user.1.as_str()),
            (
                "role/RoleDispatcher.java",
                "package role;\nclass RoleDispatcher { MessageBearer bearer; }\n",
            ),
            (
                "app/PersonDispatcher.java",
                "package app;\nimport person.MessageBearer;\nclass PersonDispatcher { MessageBearer bearer; }\n",
            ),
            (
                "user/UserDispatcher.java",
                "package user;\nimport person.*;\nclass UserDispatcher { MessageBearer bearer; }\n",
            ),
            (
                "app/Own.java",
                "package app;\nimport user.MessageBearer;\nclass Own {\n    static class MessageBearer {}\n    MessageBearer bearer;\n}\n",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "app.Own -reference-> app.Own.MessageBearer :5",
                "app.PersonDispatcher -reference-> person.MessageBearer :3",
                "role.RoleDispatcher -reference-> role.MessageBearer :2",
                "user.UserDispatcher -reference-> user.MessageBearer :3",
            ]
        );
    }

    #[test]
    fn a_wildcard_import_resolves_last_and_two_matches_make_no_edge() {
        let edges = edges(&[
            ("lib/Tool.java", "package lib;\npublic class Tool {}\n"),
            ("lib/Helper.java", "package lib;\npublic class Helper {}\n"),
            (
                "other/Helper.java",
                "package other;\npublic class Helper {}\n",
            ),
            ("app/Tool.java", "package app;\nclass Tool {}\n"),
            (
                "app/App.java",
                "\
package app;
import lib.*;
import other.*;
class App {
    Tool tool;
    Helper helper;
    Spare spare;
}
",
            ),
            (
                "other/Spare.java",
                "package other;\npublic class Spare {}\n",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "app.App -reference-> app.Tool :5",
                "app.App -reference-> other.Spare :7",
            ]
        );
    }

    #[test]
    fn unresolved_and_external_names_are_counted_without_an_edge() {
        let (edges, unresolved) = resolve_files(&[
            (
                "a/Local.java",
                "package a;\nclass Local {}\nclass Service {}\n",
            ),
            (
                "a/A.java",
                "\
package a;
import java.util.List;
import org.springframework.stereotype.Service;
@Service
class A {
    List<String> names;
    Service service;
    Local local;
    void run() { String.valueOf(Math.abs(1)); log.info(); }
}
",
            ),
        ]);
        assert_eq!(edges, ["a.A -reference-> a.Local :8"]);
        let mut counted: Vec<(&str, usize)> = unresolved
            .iter()
            .map(|(name, count)| (name.as_str(), *count))
            .collect();
        counted.sort();
        assert_eq!(
            counted,
            [("List", 1), ("Math", 1), ("Service", 2), ("String", 2)],
            "an imported external Service shadows the same-package one"
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
        assert_eq!(edges, ["a.Box -reference-> a.Helper :5"]);
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
    fn static_imports_of_constants_reference_their_type() {
        let edges = edges(&[
            (
                "a/Limits.java",
                "package a;\npublic class Limits { public static final int MAX = 1; public static int min() { return 0; } }\n",
            ),
            ("a/Color.java", "package a;\npublic enum Color { RED }\n"),
            (
                "b/B.java",
                "\
package b;
import static a.Limits.MAX;
import static a.Limits.min;
import static a.Color.*;
class B {
    int limit = MAX;
    Object color() { return RED; }
    int low() { return min(); }
    void shadowed(int MAX) { int x = MAX; }
}
",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "b.B -reference-> a.Limits :6",
                "b.B#color() -reference-> a.Color :7",
            ]
        );
    }

    #[test]
    fn inherited_fields_shadow_a_type_named_like_them() {
        let edges = edges(&[(
            "a/A.java",
            "\
package a;
class Config { static void load() {} }
class Base { Object Config; }
class A extends Base {
    void run() { Config.toString(); }
}
",
        )]);
        assert_eq!(edges, ["a.A -extends-> a.Base :4"]);
    }

    #[test]
    fn the_header_and_class_annotations_resolve_around_the_type() {
        let edges = edges(&[(
            "a/C.java",
            "\
package a;
class Base {}
@interface Ann {}
@Ann
class C extends Base {
    static class Base {}
    @interface Ann {}
    Base inner;
}
record R(Item item) { static class Item {} }
class Item {}
",
        )]);
        assert_eq!(
            edges,
            [
                "a.C -extends-> a.Base :5",
                "a.C -reference-> a.Ann :4",
                "a.C -reference-> a.C.Base :8",
                "a.R -reference-> a.R.Item :10",
            ]
        );
    }

    #[test]
    fn a_static_field_in_a_case_label_references_its_type_an_enum_constant_does_not() {
        let edges = edges(&[
            (
                "a/Events.java",
                "package a;\npublic class Events { public static final String ADDED = \"a\"; public static final String REMOVED = \"r\"; }\n",
            ),
            ("a/Color.java", "package a;\npublic enum Color { RED }\n"),
            (
                "b/B.java",
                "\
package b;
import static a.Events.ADDED;
import static a.Events.REMOVED;
import static a.Color.RED;
class B {
    void on(String event) {
        switch (event) {
            case ADDED, REMOVED -> {}
            default -> {}
        }
    }
    void paint(a.Color color) {
        switch (color) {
            case RED: break;
        }
    }
}
",
            ),
        ]);
        assert_eq!(
            edges,
            [
                "b.B#on(String) -reference-> a.Events :8",
                "b.B#paint(a.Color) -reference-> a.Color :12",
            ]
        );
    }

    #[test]
    fn nested_types_of_an_anonymous_or_local_class_super_type_are_in_scope() {
        let edges = edges(&[(
            "a/U.java",
            "\
package a;
class Base { static class Item {} Object Config; }
class Item {}
class Config { static void load() {} }
class U {
    void anonymous() {
        new Base() { Item item; void m() { Config.load(); } };
    }
    void local() {
        class L extends Base { Item item; }
    }
    void outside() { Item item; }
}
",
        )]);
        assert_eq!(
            edges,
            [
                "a.U#anonymous() -instantiate-> a.Base :7",
                "a.U#anonymous() -reference-> a.Base.Item :7",
                "a.U#local() -reference-> a.Base :10",
                "a.U#local() -reference-> a.Base.Item :10",
                "a.U#outside() -reference-> a.Item :12",
            ]
        );
    }

    #[test]
    fn a_qualified_instance_creation_makes_no_edge_to_the_inner_type() {
        let (edges, unresolved) = resolve_files(&[(
            "a/U.java",
            "\
package a;
class Inner {}
class Outer { class Inner {} }
class U { void m(Outer o) { o.new Inner(); new Inner(); } }
",
        )]);
        assert_eq!(
            edges,
            [
                "a.U#m(Outer) -instantiate-> a.Inner :4",
                "a.U#m(Outer) -reference-> a.Outer :4",
            ]
        );
        assert_eq!(unresolved.get("Inner"), Some(&1));
    }

    #[test]
    fn a_static_imported_constant_as_a_qualifier_references_its_type() {
        let (edges, unresolved) = resolve_files(&[
            (
                "a/L.java",
                "package a;\npublic class L { public static final String MAX = \"m\"; }\n",
            ),
            (
                "b/B.java",
                "package b;\nimport static a.L.MAX;\nclass B { int n() { return MAX.length(); } }\n",
            ),
        ]);
        assert_eq!(edges, ["b.B#n() -reference-> a.L :3"]);
        assert!(!unresolved.contains_key("MAX"), "{unresolved:?}");
    }
}
