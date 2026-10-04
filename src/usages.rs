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
//! Members resolve through the static type of their receiver: a local
//! variable or parameter (block-scoped, `var` from its initializer), a field
//! of a class around the call (inherited from an indexed super type
//! included), a type (`T.m()`), `this`, `super`, or the declared return type
//! of the call before it (`a.b().c()`), resolved in the declaring type's
//! file. The method is looked up in that type, its indexed superclasses,
//! then its interfaces; an unqualified `m()` in the classes around the
//! call, then in static imports. Overloads are chosen as Java does on the
//! argument types that are known; several left each get an ambiguous edge,
//! none that takes the arguments makes no edge.
//! Members the compiler or Lombok generates (accessors, builders, record
//! accessors, enum `values()`) are a `reference` to their type and carry
//! the chain on. A receiver outside the index (a library type, an untyped
//! lambda parameter, a type variable) stops it: no edge, counted by
//! `Receiver#name`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Tree};

use crate::symbols::{
    FieldFacts, Import, MethodFacts, Symbol, SymbolKind, TypeFacts, TypeRef, is_type_container,
    member_kind, type_kind,
};

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
    /// A method or constructor called: `x.m(..)`, `m(..)`, `super(..)`,
    /// `this(..)`, `x::m`.
    Call,
}

impl EdgeKind {
    /// The stable lowercase text stored in `edges.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::Instantiate => "instantiate",
            EdgeKind::Reference => "reference",
            EdgeKind::Call => "call",
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
    /// One of several overloads the call could mean.
    pub ambiguous: bool,
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
    /// Call sites (method calls, constructor calls, method references) by
    /// outcome.
    pub calls: CallSites,
    /// Call sites that resolve to no indexed member, by `Receiver#name`
    /// (the receiver's simple type name, `?` when unknown, `this` or
    /// `super`).
    pub unresolved_calls: HashMap<String, usize>,
}

/// Call sites by outcome.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CallSites {
    /// To one indexed method or constructor: a `call` (or `instantiate`)
    /// edge, none for a member calling itself.
    pub resolved: usize,
    /// To a member the compiler or Lombok generates (an accessor, a
    /// builder method, an enum or record member, an annotation element, a
    /// constructor that is not declared): only a `reference` (or the
    /// `instantiate` edge) to its type.
    pub implicit: usize,
    /// To more than one overload, each edge marked ambiguous.
    pub ambiguous: usize,
    /// To nothing in the index: a library type, an unknown receiver.
    pub unresolved: usize,
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
            unnamed_classes: Vec::new(),
            type_vars: Vec::new(),
            locals: Vec::new(),
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

/// The static type of an expression, as far as the run knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    /// Not known: an untyped lambda parameter, a type variable, a chain
    /// that left the index.
    Unknown,
    /// The `null` literal.
    Null,
    /// An indexed type (its fqn) with its array dimensions.
    Indexed(String, usize),
    /// A primitive, `void` or a type outside the index, by its simple name.
    External(String, usize),
    /// The Lombok builder of an indexed type.
    Builder(String),
}

impl Ty {
    /// Whether the type is known well enough to rank overloads by it.
    fn is_known(&self) -> bool {
        !matches!(self, Ty::Unknown | Ty::Builder(_))
    }
}

/// A method or constructor with a row.
#[derive(Clone, Copy)]
struct Method<'a> {
    id: i64,
    facts: &'a MethodFacts,
    /// The fqn of the type declaring it.
    owner: &'a str,
}

/// How an argument fits a parameter type.
enum Fit {
    Exact,
    Compatible,
    No,
}

/// The run's type table and the lookups on it.
struct Resolver<'a> {
    files: &'a [SourceFile<'a>],
    types: HashMap<&'a str, TypeEntry<'a>>,
    /// The methods and constructors with a row, by fqn.
    members: HashMap<&'a str, i64>,
    /// Per type: whether its superclass resolved (then it is the first
    /// super type), and its indexed super types.
    supertypes: RefCell<HashMap<String, (bool, Vec<String>)>>,
    pending: RefCell<HashSet<String>>,
}

impl<'a> Resolver<'a> {
    /// The table of every type with a row; for a duplicate fqn, the facts of
    /// the definition that was written.
    fn new(files: &'a [SourceFile<'a>]) -> Self {
        let mut types = HashMap::new();
        let mut members = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            for (symbol, id) in file
                .symbols
                .iter()
                .filter(|(symbol, _)| !symbol.kind.is_type())
            {
                members.entry(symbol.fqn.as_str()).or_insert(*id);
            }
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
            members,
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
        self.lineage(&[owner.to_string()])
            .iter()
            .map(|entry| format!("{}.{name}", entry.facts.fqn))
            .find(|fqn| self.types.contains_key(fqn.as_str()))
    }

    /// `owners` and their indexed super types in the order Java looks a
    /// member up: the indexed superclass chain of each owner first, then
    /// the interfaces breadth-first; each type once.
    fn lineage(&self, owners: &[String]) -> Vec<&TypeEntry<'a>> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut order: Vec<&TypeEntry<'a>> = Vec::new();
        for owner in owners {
            let mut current = Some(owner.clone());
            while let Some(fqn) = current {
                let Some(entry) = self.types.get(fqn.as_str()) else {
                    break;
                };
                if !seen.insert(entry.facts.fqn.as_str()) {
                    break;
                }
                order.push(entry);
                current = self.superclass(&fqn);
            }
        }
        let mut next = 0;
        while let Some(entry) = order.get(next) {
            next += 1;
            for supertype in self.supertypes(&entry.facts.fqn) {
                if let Some(found) = self.types.get(supertype.as_str())
                    && seen.insert(found.facts.fqn.as_str())
                {
                    order.push(found);
                }
            }
        }
        order
    }

    /// The indexed superclass and interfaces of `fqn`, resolved in its own
    /// file around its enclosing type; memoized, and empty on a cycle.
    fn supertypes(&self, fqn: &str) -> Vec<String> {
        self.supers(fqn).1
    }

    /// Whether the superclass of `fqn` resolved (then it is the first of
    /// them), and its indexed super types.
    fn supers(&self, fqn: &str) -> (bool, Vec<String>) {
        if let Some(found) = self.supertypes.borrow().get(fqn) {
            return found.clone();
        }
        let Some(entry) = self.types.get(fqn) else {
            return (false, Vec::new());
        };
        if !self.pending.borrow_mut().insert(fqn.to_string()) {
            return (false, Vec::new());
        }
        let facts = entry.facts;
        let resolve = |ty: &TypeRef| {
            if facts.type_params.contains(&ty.name) {
                return None;
            }
            let segments: Vec<&str> = ty.name.split('.').collect();
            match self.resolve_type(
                entry.file,
                Scope::around(facts.parent.as_deref()),
                &segments,
            ) {
                Lookup::Found(fqn) => Some(fqn),
                _ => None,
            }
        };
        let superclass = facts.superclass.as_ref().and_then(resolve);
        let found = (
            superclass.is_some(),
            superclass
                .into_iter()
                .chain(facts.interfaces.iter().filter_map(resolve))
                .collect(),
        );
        self.pending.borrow_mut().remove(fqn);
        self.supertypes
            .borrow_mut()
            .insert(fqn.to_string(), found.clone());
        found
    }

    /// Whether `owner` or one of its indexed super types has the field (or
    /// enum constant, or record component) `name`, of the kind `fields`.
    fn has_field(&self, owner: &str, name: &str, fields: FieldFilter) -> bool {
        self.lineage(&[owner.to_string()]).iter().any(|entry| {
            let facts = entry.facts;
            facts
                .fields
                .iter()
                .any(|field| field.name == name && (field.is_static || fields == FieldFilter::Any))
                || (fields != FieldFilter::StaticFields
                    && facts.enum_constants.iter().any(|constant| constant == name))
                || (fields == FieldFilter::Any
                    && facts
                        .record_components
                        .iter()
                        .any(|component| component.name == name))
        })
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

    /// The indexed superclass of `fqn`, resolved in its own file.
    fn superclass(&self, fqn: &str) -> Option<String> {
        match self.supers(fqn) {
            (true, supertypes) => supertypes.into_iter().next(),
            _ => None,
        }
    }

    /// Whether `fqn` or a superclass of it in the index extends a type
    /// outside the index, whose members are unknown.
    fn has_external_superclass(&self, fqn: &str) -> bool {
        let mut seen = HashSet::new();
        let mut current = fqn.to_string();
        while seen.insert(current.clone()) {
            let Some(entry) = self.types.get(current.as_str()) else {
                return true;
            };
            if entry.facts.superclass.is_none() {
                return false;
            }
            match self.superclass(&current) {
                Some(next) => current = next,
                None => return true,
            }
        }
        false
    }

    /// Whether `sub` is `sup` or has it among its indexed super types.
    fn is_subtype(&self, sub: &str, sup: &str) -> bool {
        self.lineage(&[sub.to_string()])
            .iter()
            .any(|entry| entry.facts.fqn == sup)
    }

    /// The type a declared `ty` of a member or field of `owner` stands for,
    /// resolved in `owner`'s file inside it; a type variable (of the member,
    /// `owner` or a type around it) or a wildcard is unknown.
    fn declared(&self, owner: &str, ty: &TypeRef, member_vars: &[String]) -> Ty {
        if is_primitive(&ty.name) || ty.name == "void" {
            return Ty::External(ty.name.clone(), ty.dims);
        }
        if ty.name == "?" || member_vars.contains(&ty.name) {
            return Ty::Unknown;
        }
        let Some(entry) = self.types.get(owner) else {
            return Ty::Unknown;
        };
        let mut current = Some(entry.facts);
        while let Some(facts) = current {
            if facts.type_params.contains(&ty.name) {
                return Ty::Unknown;
            }
            current = facts
                .parent
                .as_deref()
                .and_then(|parent| self.types.get(parent))
                .map(|entry| entry.facts);
        }
        let segments: Vec<&str> = ty.name.split('.').collect();
        match self.resolve_type(entry.file, Scope::around(Some(owner)), &segments) {
            Lookup::Found(fqn) => Ty::Indexed(fqn, ty.dims),
            Lookup::Ambiguous(_) => Ty::Unknown,
            Lookup::External | Lookup::Unknown => {
                Ty::External(segments[segments.len() - 1].to_string(), ty.dims)
            }
        }
    }

    /// The field (or enum constant, or record component) `name` of `owner`
    /// or its indexed super types: the type declaring it and its type.
    fn field(&self, owner: &str, name: &str) -> Option<(&'a str, Ty)> {
        self.lineage(&[owner.to_string()]).iter().find_map(|entry| {
            let facts = entry.facts;
            let declarer = facts.fqn.as_str();
            if let Some(field) = facts.fields.iter().find(|field| field.name == name) {
                return Some((declarer, self.declared(declarer, &field.ty, &[])));
            }
            if facts.enum_constants.iter().any(|constant| constant == name) {
                return Some((declarer, Ty::Indexed(declarer.to_string(), 0)));
            }
            facts
                .record_components
                .iter()
                .find(|component| component.name == name)
                .map(|component| (declarer, self.declared(declarer, &component.ty, &[])))
        })
    }

    /// The methods called `name` that `owners` have or inherit from their
    /// indexed super types; a method overridden nearer is left out (same
    /// parameter count and simple type names, a type variable of the
    /// farther one matching any type), the overloads of one type never.
    fn methods(&self, owners: &[String], name: &str) -> Vec<Method<'a>> {
        let mut found: Vec<Method<'a>> = Vec::new();
        for entry in self.lineage(owners) {
            let owner = entry.facts.fqn.as_str();
            for facts in entry.facts.methods.iter().filter(|m| m.name == name) {
                let Some(&id) = self.members.get(facts.fqn.as_str()) else {
                    continue;
                };
                let overridden = found.iter().any(|nearer| {
                    nearer.owner != owner
                        && overrides(nearer.facts, facts, &entry.facts.type_params)
                });
                if !overridden {
                    found.push(Method { id, facts, owner });
                }
            }
        }
        found
    }

    /// The declared constructors of `owner`.
    fn constructors(&self, owner: &str) -> Vec<Method<'a>> {
        let Some(entry) = self.types.get(owner) else {
            return Vec::new();
        };
        entry
            .facts
            .methods
            .iter()
            .filter(|m| m.name == "<init>")
            .filter_map(|facts| {
                Some(Method {
                    id: *self.members.get(facts.fqn.as_str())?,
                    facts,
                    owner: entry.facts.fqn.as_str(),
                })
            })
            .collect()
    }

    /// The static methods called `name` that the static imports of `file`
    /// bring in: those of a single static import of that name, else those
    /// of the static wildcard imports.
    fn static_methods(&self, file: usize, name: &str) -> Vec<Method<'a>> {
        let imports = self.files[file].imports;
        let single: Vec<String> = imports
            .iter()
            .filter(|import| import.is_static && !import.wildcard)
            .filter_map(|import| {
                let (owner, member) = import.path.rsplit_once('.')?;
                (member == name).then(|| owner.to_string())
            })
            .collect();
        let owners = if single.is_empty() {
            imports
                .iter()
                .filter(|import| import.is_static && import.wildcard)
                .map(|import| import.path.clone())
                .collect()
        } else {
            single
        };
        owners
            .iter()
            .filter(|owner| self.types.contains_key(owner.as_str()))
            .flat_map(|owner| self.methods(std::slice::from_ref(owner), name))
            .filter(|method| method.facts.is_static)
            .collect()
    }

    /// A member the compiler or Lombok generates, called `name` with
    /// `argc` arguments, on `owners` or their indexed super types: the type
    /// it belongs to and its result.
    fn implicit(&self, owners: &[String], name: &str, argc: usize) -> Option<(&'a str, Ty)> {
        self.lineage(owners).iter().find_map(|entry| {
            let found = self.implicit_in(entry.facts, name, argc)?;
            Some((entry.facts.fqn.as_str(), found))
        })
    }

    fn implicit_in(&self, facts: &'a TypeFacts, name: &str, argc: usize) -> Option<Ty> {
        let owner = facts.fqn.as_str();
        let on_type = |names: &[&str]| facts.lombok.iter().any(|ann| names.contains(&ann.as_str()));
        let getters = on_type(&["Getter", "Data", "Value"]);
        let setters = on_type(&["Setter", "Data"]);
        for field in facts.fields.iter().filter(|field| !field.is_static) {
            let on_field = |ann: &str| field.lombok.iter().any(|own| own == ann);
            if argc == 0 && (getters || on_field("Getter")) && getter_name(field) == name {
                return Some(self.declared(owner, &field.ty, &[]));
            }
            if argc == 1 && (setters || on_field("Setter")) && setter_name(field) == name {
                return Some(Ty::External("void".to_string(), 0));
            }
        }
        if argc == 0
            && matches!(name, "builder" | "toBuilder")
            && on_type(&["Builder", "SuperBuilder"])
        {
            return Some(Ty::Builder(owner.to_string()));
        }
        if facts.kind == SymbolKind::Record
            && argc == 0
            && let Some(component) = facts
                .record_components
                .iter()
                .find(|component| component.name == name)
        {
            return Some(self.declared(owner, &component.ty, &[]));
        }
        if facts.kind == SymbolKind::Annotation && argc == 0 && !is_object_method(name) {
            return Some(Ty::Unknown);
        }
        if facts.kind == SymbolKind::Enum {
            match (name, argc) {
                ("values", 0) => return Some(Ty::Indexed(owner.to_string(), 1)),
                ("valueOf", 1) => return Some(Ty::Indexed(owner.to_string(), 0)),
                ("name", 0) => return Some(Ty::External("String".to_string(), 0)),
                ("ordinal", 0) => return Some(Ty::External("int".to_string(), 0)),
                _ => {}
            }
        }
        None
    }

    /// Whether `owner` gets constructors it does not declare: a record's
    /// canonical one, Lombok's `@…ArgsConstructor`.
    fn generates_constructors(&self, owner: &str) -> bool {
        self.types.get(owner).is_some_and(|entry| {
            entry.facts.kind == SymbolKind::Record
                || entry.facts.lombok.iter().any(|ann| {
                    matches!(
                        ann.as_str(),
                        "AllArgsConstructor" | "RequiredArgsConstructor" | "NoArgsConstructor"
                    )
                })
        })
    }

    /// The candidates a call with arguments of the types `args` can mean,
    /// as far as Java's rule decides it on the types the run knows: those
    /// every argument can be passed to, by fixed arity (a varargs method
    /// taking an array) first, by variable arity (at least n − 1
    /// arguments) only when no fixed-arity one fits; of several, the most
    /// exact matches, and only when every argument's type is known. More
    /// than one result is an ambiguous call; none is a call to a method the
    /// index does not have (inherited from a library class or `Object`).
    fn choose(&self, candidates: Vec<Method<'a>>, args: &[Ty]) -> Vec<Method<'a>> {
        let applicable = |spread: bool| -> Vec<(usize, Method<'a>)> {
            candidates
                .iter()
                .filter(|method| !spread || method.facts.varargs)
                .filter_map(|method| Some((self.score(method, args, spread)?, *method)))
                .collect()
        };
        let mut chosen = applicable(false);
        if chosen.is_empty() {
            chosen = applicable(true);
        }
        if chosen.len() > 1 && args.iter().all(Ty::is_known) {
            let best = chosen.iter().map(|(score, _)| *score).max().unwrap_or(0);
            chosen.retain(|(score, _)| *score == best);
        }
        chosen.into_iter().map(|(_, method)| method).collect()
    }

    /// How many arguments match `method`'s parameter types exactly, or
    /// `None` when the count differs or one cannot be passed. `spread`
    /// passes the trailing arguments as elements of the varargs parameter,
    /// else it takes one array.
    fn score(&self, method: &Method<'a>, args: &[Ty], spread: bool) -> Option<usize> {
        let params = &method.facts.params;
        let count_fits = if spread {
            args.len() + 1 >= params.len()
        } else {
            args.len() == params.len()
        };
        if !count_fits {
            return None;
        }
        let mut exact = 0;
        for (index, arg) in args.iter().enumerate() {
            let last = index + 1 >= params.len();
            let param = params.get(index.min(params.len().saturating_sub(1)))?;
            let mut param = self.declared(method.owner, &param.ty, &method.facts.type_params);
            if method.facts.varargs && last && !spread {
                param = with_dims(param, 1);
            }
            match fits(arg, &param, |sub, sup| self.is_subtype(sub, sup)) {
                Fit::Exact => exact += 1,
                Fit::Compatible => {}
                Fit::No => return None,
            }
        }
        Some(exact)
    }
}

/// The type variables and local types declared in one member, field
/// declaration, initializer or enum constant, whatever their block: neither
/// names a type of the index.
#[derive(Debug, Default)]
struct Region {
    types: HashSet<String>,
    type_vars: HashSet<String>,
}

/// One anonymous or local class around the current node.
struct Unnamed<'a> {
    /// Where its super types start in [`FileWalk::unnamed`].
    start: usize,
    /// Where its own fields and the variables declared in it start in
    /// [`FileWalk::locals`].
    locals: usize,
    /// Whether a super type it names is outside the index, so its
    /// inherited members are unknown.
    opaque: bool,
    /// The names of the methods it declares (not symbols).
    methods: Vec<&'a str>,
}

/// Where a call's method is looked up.
enum Receiver {
    /// `m()`: the classes around the call, innermost first, then static
    /// imports.
    Unqualified,
    /// `x.m()`, `T.m()`, `a.b().m()`: the static type of the receiver.
    Value(Ty),
    /// `super.m()`: these types and their super types.
    Super(Vec<String>),
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
    /// Those classes, innermost last.
    unnamed_classes: Vec<Unnamed<'a>>,
    /// The type variables of those types.
    type_vars: Vec<String>,
    /// The local variables, parameters and anonymous-class fields in scope,
    /// with their types; a block's are dropped at its end.
    locals: Vec<(&'a str, Ty)>,
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
    fn statement(&mut self, node: Node<'a>, named: bool) {
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
    fn node_ty(&self, node: Node<'a>) -> Ty {
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
    fn unnamed_body(&mut self, node: Node<'a>, supers: Vec<String>, opaque: bool) {
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

    /// An expression: its usages, and its static type.
    fn expr(&mut self, node: Node<'a>) -> Ty {
        match node.kind() {
            "identifier" => self.bare_name(node),
            "method_invocation" => self.method_invocation(node),
            "field_access" => self.field_access(node),
            "object_creation_expression" => self.object_creation(node),
            "method_reference" => {
                self.method_reference(node);
                Ty::Unknown
            }
            "lambda_expression" => {
                self.lambda(node);
                Ty::Unknown
            }
            "cast_expression" => {
                self.children(node, false);
                node.child_by_field_name("type")
                    .map_or(Ty::Unknown, |ty| self.node_ty(ty))
            }
            "parenthesized_expression" => {
                let mut cursor = node.walk();
                let inner = node.named_children(&mut cursor).next();
                match inner {
                    Some(inner) => self.expr(inner),
                    None => Ty::Unknown,
                }
            }
            "array_access" => {
                let array = node
                    .child_by_field_name("array")
                    .map_or(Ty::Unknown, |array| self.expr(array));
                if let Some(index) = node.child_by_field_name("index") {
                    self.visit(index, false);
                }
                match array {
                    Ty::Indexed(fqn, dims) if dims > 0 => Ty::Indexed(fqn, dims - 1),
                    Ty::External(name, dims) if dims > 0 => Ty::External(name, dims - 1),
                    _ => Ty::Unknown,
                }
            }
            "this" => match self.this_types() {
                Some(types) if types.len() == 1 => Ty::Indexed(types[0].clone(), 0),
                _ => Ty::Unknown,
            },
            "string_literal" | "text_block" => Ty::External("String".to_string(), 0),
            "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal" => {
                let long = node_text(node, self.source).ends_with(['l', 'L']);
                Ty::External(if long { "long" } else { "int" }.to_string(), 0)
            }
            "decimal_floating_point_literal" | "hex_floating_point_literal" => {
                let float = node_text(node, self.source).ends_with(['f', 'F']);
                Ty::External(if float { "float" } else { "double" }.to_string(), 0)
            }
            "true" | "false" => Ty::External("boolean".to_string(), 0),
            "character_literal" => Ty::External("char".to_string(), 0),
            "null_literal" => Ty::Null,
            _ => {
                self.statement(node, false);
                Ty::Unknown
            }
        }
    }

    /// A lambda: its parameters are in scope in its body only.
    fn lambda(&mut self, node: Node<'a>) {
        let locals = self.locals.len();
        if let Some(parameters) = node.child_by_field_name("parameters") {
            if parameters.kind() == "identifier" {
                self.locals
                    .push((node_text(parameters, self.source), Ty::Unknown));
            } else {
                self.visit(parameters, false);
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.visit(body, false);
        }
        self.locals.truncate(locals);
    }

    /// The types arguments of a call are passed as, in order.
    fn arguments(&mut self, node: Option<Node<'a>>) -> Vec<Ty> {
        let Some(node) = node else {
            return Vec::new();
        };
        let mut types = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() && !child.is_extra() {
                types.push(self.expr(child));
            } else {
                self.visit(child, false);
            }
        }
        types
    }

    /// `new T(..)`, with or without a body: `instantiate` to `T` and to the
    /// constructor the arguments choose; in the body, `T`'s nested types are
    /// in scope. In `outer.new Inner()` the type is a member of `outer`'s
    /// type.
    fn object_creation(&mut self, node: Node<'a>) -> Ty {
        let ty = node.child_by_field_name("type");
        let outer = node.child(0).filter(|first| first.kind() != "new");
        let mut created = Ty::Unknown;
        let mut written = String::new();
        let mut args = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == ty {
                let mut segments = Vec::new();
                type_segments(child, self.source, &mut segments);
                written = segments.join(".");
                created = match outer {
                    Some(_) => self.qualified_creation(child, &created, &written),
                    None => {
                        self.emit_type(child, EdgeKind::Instantiate);
                        self.node_ty(child)
                    }
                };
            } else if Some(child) == outer {
                created = self.expr(child);
            } else if Some(child) == node.child_by_field_name("arguments") {
                args = self.arguments(Some(child));
            } else if child.kind() == "class_body" {
                let (supers, opaque) = match &created {
                    Ty::Indexed(fqn, 0) => (vec![fqn.clone()], false),
                    _ => (Vec::new(), true),
                };
                self.unnamed_body(child, supers, opaque);
            } else {
                self.visit(child, false);
            }
        }
        let line = ty.map_or(line(node), line);
        match &created {
            Ty::Indexed(fqn, 0) => {
                if !self.constructor_call(fqn, &args, EdgeKind::Instantiate, line) {
                    self.usages.calls.implicit += 1;
                }
            }
            _ => {
                let simple = written.rsplit('.').next().unwrap_or_default();
                self.unresolved_call(&format!("{simple}#<init>"));
            }
        }
        created
    }

    /// The type of `outer.new Inner<A>()`, `outer` being of the type
    /// `outer_ty`: `instantiate` to `Inner` when it is a member type of
    /// `outer`'s type, else counted as unresolved; its generic arguments
    /// visited.
    fn qualified_creation(&mut self, node: Node<'a>, outer_ty: &Ty, written: &str) -> Ty {
        let inner = match outer_ty {
            Ty::Indexed(outer, 0) => self.resolver.member_type(outer, written),
            _ => None,
        };
        match &inner {
            Some(fqn) => self.emit(fqn, EdgeKind::Instantiate, line(node)),
            None => {
                *self
                    .usages
                    .unresolved
                    .entry(written.to_string())
                    .or_default() += 1;
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_arguments" {
                self.visit(child, false);
            }
        }
        inner.map_or(Ty::Unknown, |fqn| Ty::Indexed(fqn, 0))
    }

    /// `super(..)` or `this(..)` in a constructor: a `call` to the
    /// constructor the arguments choose; an implicit superclass
    /// constructor is a `reference` to the superclass.
    fn constructor_invocation(&mut self, node: Node<'a>) {
        let mut cursor = node.walk();
        let mut args = Vec::new();
        for child in node.children(&mut cursor) {
            if Some(child) == node.child_by_field_name("arguments") {
                args = self.arguments(Some(child));
            } else {
                self.visit(child, false);
            }
        }
        let is_super = node
            .child_by_field_name("constructor")
            .is_some_and(|constructor| constructor.kind() == "super");
        let owner = if !self.unnamed_classes.is_empty() {
            None
        } else if is_super {
            self.enclosing
                .last()
                .and_then(|current| self.resolver.superclass(current))
        } else {
            self.enclosing.last().map(|current| current.to_string())
        };
        let line = line(node);
        let Some(owner) = owner else {
            let written = self
                .enclosing
                .last()
                .filter(|_| is_super && self.unnamed_classes.is_empty())
                .and_then(|current| self.resolver.types.get(current))
                .and_then(|entry| entry.facts.superclass.as_ref())
                .map(|ty| ty.name.rsplit('.').next().unwrap_or_default());
            let label = written.unwrap_or(if is_super { "super" } else { "this" });
            self.unresolved_call(&format!("{label}#<init>"));
            return;
        };
        if !self.constructor_call(&owner, &args, EdgeKind::Call, line) {
            self.implicit_call(&owner, line);
        }
    }

    /// An edge of `kind` to the constructor of `owner` the arguments
    /// choose. False when the constructor is one the compiler or Lombok
    /// generates (none declared, a record, `@…ArgsConstructor`); a
    /// declared one that takes no such arguments is counted unresolved.
    fn constructor_call(&mut self, owner: &str, args: &[Ty], kind: EdgeKind, line: usize) -> bool {
        let constructors = self.resolver.constructors(owner);
        let declared = !constructors.is_empty();
        let chosen = self.resolver.choose(constructors, args);
        if !chosen.is_empty() {
            self.calls_to(&chosen, kind, line);
        } else if declared && !self.resolver.generates_constructors(owner) {
            let simple = owner.rsplit('.').next().unwrap_or_default();
            self.unresolved_call(&format!("{simple}#<init>"));
        } else {
            return false;
        }
        true
    }

    /// `x::m`, `T::m`, `this::m`, `super::m` (a `call` to every method `m`
    /// of the type: the arity is unknown) and `T::new` (`instantiate` to the
    /// type and its constructors); `T` itself is a reference.
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
        let receiver = match target.kind() {
            "this" => Receiver::Value(self.expr(target)),
            "super" => Receiver::Super(self.super_types_here()),
            _ => match name_chain(target, self.source) {
                Some(segments) => Receiver::Value(self.chain(&segments, true, kind, line(target))),
                None if constructor => {
                    self.emit_type(target, kind);
                    Receiver::Value(self.node_ty(target))
                }
                None => Receiver::Value(self.expr(target)),
            },
        };
        for child in rest {
            if child.kind() == "type_arguments" {
                self.visit(*child, false);
            }
        }
        let line = line(node);
        if constructor {
            match receiver {
                Receiver::Value(Ty::Indexed(fqn, 0)) => {
                    let constructors = self.resolver.constructors(&fqn);
                    if constructors.is_empty() {
                        self.usages.calls.implicit += 1;
                    } else {
                        self.calls_to(&constructors, EdgeKind::Instantiate, line);
                    }
                }
                Receiver::Value(Ty::Indexed(..) | Ty::External(_, 1..)) => {}
                Receiver::Value(ty) => {
                    self.unresolved_call(&format!("{}#<init>", receiver_label(&ty)));
                }
                _ => {}
            }
            return;
        }
        let Some(name) = rest
            .iter()
            .rev()
            .find(|child| child.kind() == "identifier")
            .map(|name| node_text(*name, self.source))
        else {
            return;
        };
        let (owners, label) = match receiver {
            Receiver::Value(Ty::Indexed(fqn, 0)) => {
                let label = receiver_label(&Ty::Indexed(fqn.clone(), 0));
                (vec![fqn], label)
            }
            Receiver::Super(owners) => (owners, "super".to_string()),
            Receiver::Value(ty) => {
                self.unresolved_call(&format!("{}#{name}", receiver_label(&ty)));
                return;
            }
            Receiver::Unqualified => return,
        };
        let methods = self.resolver.methods(&owners, name);
        if !methods.is_empty() {
            self.calls_to(&methods, EdgeKind::Call, line);
            return;
        }
        let implicit = self
            .resolver
            .implicit(&owners, name, 0)
            .or_else(|| self.resolver.implicit(&owners, name, 1));
        match implicit {
            Some((owner, _)) => self.implicit_call(owner, line),
            None => self.unresolved_call(&format!("{label}#{name}")),
        }
    }

    /// `m(..)`, `x.m(..)`, `T.m(..)`, `super.m(..)`, `a.b().m(..)`: a
    /// `call` to the method the receiver's type and the arguments choose;
    /// the result is its declared return type.
    fn method_invocation(&mut self, node: Node<'a>) -> Ty {
        let object = node.child_by_field_name("object");
        let interface_super = object.is_some()
            && (0..node.child_count())
                .filter_map(|index| node.child(index))
                .any(|child| child.kind() == "super" && Some(child) != object);
        let receiver = match object {
            None => Receiver::Unqualified,
            Some(object) if object.kind() == "super" => Receiver::Super(self.super_types_here()),
            Some(object) => {
                let ty = match name_chain(object, self.source) {
                    Some(segments) => {
                        self.chain(&segments, true, EdgeKind::Reference, line(object))
                    }
                    None => self.expr(object),
                };
                if interface_super {
                    match ty {
                        Ty::Indexed(fqn, 0) => Receiver::Super(self.super_of(&fqn)),
                        _ => Receiver::Value(Ty::Unknown),
                    }
                } else {
                    Receiver::Value(ty)
                }
            }
        };
        if let Some(type_arguments) = node.child_by_field_name("type_arguments") {
            self.visit(type_arguments, false);
        }
        let args = self.arguments(node.child_by_field_name("arguments"));
        let Some(name_node) = node.child_by_field_name("name") else {
            return Ty::Unknown;
        };
        let name = node_text(name_node, self.source);
        let line = line(name_node);
        match receiver {
            Receiver::Unqualified => self.unqualified_call(name, &args, line),
            Receiver::Super(owners) => self.call_on(&owners, name, &args, line, "super"),
            Receiver::Value(Ty::Indexed(fqn, 0)) => {
                let label = receiver_label(&Ty::Indexed(fqn.clone(), 0));
                self.call_on(&[fqn], name, &args, line, &label)
            }
            Receiver::Value(Ty::Builder(owner)) => self.builder_call(&owner, name, &args, line),
            Receiver::Value(ty) => {
                self.unresolved_call(&format!("{}#{name}", receiver_label(&ty)));
                jdk_result(&ty, name, args.len())
            }
        }
    }

    /// `m(..)` without a receiver: the innermost class around the call that
    /// has a method `m` (inherited from an indexed super type included),
    /// else a static import. A class extending a type outside the index may
    /// inherit `m` from it, and an anonymous or local class's own methods
    /// are not symbols: the search stops there.
    fn unqualified_call(&mut self, name: &str, args: &[Ty], line: usize) -> Ty {
        let mut frames: Vec<(Vec<String>, bool)> = Vec::new();
        for (index, class) in self.unnamed_classes.iter().enumerate().rev() {
            let opaque = class.opaque || class.methods.contains(&name);
            frames.push((self.unnamed_supers(index).to_vec(), opaque));
        }
        for owner in self.enclosing.iter().rev() {
            let is_enum = self
                .resolver
                .types
                .get(owner)
                .is_some_and(|entry| entry.facts.kind == SymbolKind::Enum);
            frames.push((
                vec![owner.to_string()],
                self.resolver.has_external_superclass(owner) || (is_enum && is_enum_method(name)),
            ));
        }
        for (owners, opaque) in frames {
            if !self.resolver.methods(&owners, name).is_empty()
                || self.resolver.implicit(&owners, name, args.len()).is_some()
            {
                return self.call_on(&owners, name, args, line, "this");
            }
            if opaque || is_object_method(name) {
                self.unresolved_call(&format!("this#{name}"));
                return Ty::Unknown;
            }
        }
        let imported = self.resolver.static_methods(self.file, name);
        if imported.is_empty() {
            self.unresolved_call(&format!("this#{name}"));
            return Ty::Unknown;
        }
        let chosen = self.resolver.choose(imported, args);
        if chosen.is_empty() {
            self.unresolved_call(&format!("this#{name}"));
            return Ty::Unknown;
        }
        self.calls_to(&chosen, EdgeKind::Call, line);
        self.return_type(&chosen)
    }

    /// A call of `name` on a receiver of the types `owners`: the methods
    /// the arguments choose, else a member the compiler or Lombok
    /// generates (a `reference` to its owner).
    fn call_on(
        &mut self,
        owners: &[String],
        name: &str,
        args: &[Ty],
        line: usize,
        label: &str,
    ) -> Ty {
        let chosen = self
            .resolver
            .choose(self.resolver.methods(owners, name), args);
        if !chosen.is_empty() {
            self.calls_to(&chosen, EdgeKind::Call, line);
            return self.return_type(&chosen);
        }
        match self.resolver.implicit(owners, name, args.len()) {
            Some((owner, ty)) => {
                self.implicit_call(owner, line);
                ty
            }
            None => {
                self.unresolved_call(&format!("{label}#{name}"));
                jdk_result(&Ty::Unknown, name, args.len())
            }
        }
    }

    /// A call on a Lombok builder of `owner`, a reference to `owner`:
    /// `build()` returns the type, every other method (a field's setter,
    /// whatever its `setterPrefix`, a `@Singular` adder) the builder.
    /// A builder class written in the source (`{T}Builder` nested in `T`)
    /// is looked up first; `Object`'s methods return their JDK type.
    fn builder_call(&mut self, owner: &str, name: &str, args: &[Ty], line: usize) -> Ty {
        let simple = owner.rsplit('.').next().unwrap_or_default();
        if let Some(written) = self
            .resolver
            .member_type(owner, &format!("{simple}Builder"))
            .filter(|builder| {
                !self
                    .resolver
                    .methods(std::slice::from_ref(builder), name)
                    .is_empty()
            })
        {
            let label = receiver_label(&Ty::Indexed(written.clone(), 0));
            return match self.call_on(std::slice::from_ref(&written), name, args, line, &label) {
                Ty::Indexed(fqn, 0) if fqn == written => Ty::Builder(owner.to_string()),
                other => other,
            };
        }
        self.implicit_call(owner, line);
        if is_object_method(name) {
            jdk_result(&Ty::Unknown, name, args.len())
        } else if name == "build" && args.is_empty() {
            Ty::Indexed(owner.to_string(), 0)
        } else {
            Ty::Builder(owner.to_string())
        }
    }

    /// The return type the chosen methods agree on.
    fn return_type(&self, chosen: &[Method<'a>]) -> Ty {
        let mut types = chosen.iter().map(|method| {
            method.facts.return_type.as_ref().map_or(Ty::Unknown, |ty| {
                self.resolver
                    .declared(method.owner, ty, &method.facts.type_params)
            })
        });
        let Some(first) = types.next() else {
            return Ty::Unknown;
        };
        if types.all(|ty| ty == first) {
            first
        } else {
            Ty::Unknown
        }
    }

    /// The indexed super types of the `index`th anonymous or local class
    /// around the node.
    fn unnamed_supers(&self, index: usize) -> &[String] {
        let end = self
            .unnamed_classes
            .get(index + 1)
            .map_or(self.unnamed.len(), |next| next.start);
        &self.unnamed[self.unnamed_classes[index].start..end]
    }

    /// The types a `this` receiver stands for: the innermost class around
    /// the node (an anonymous or local class by its indexed super types).
    fn this_types(&self) -> Option<Vec<String>> {
        match self.unnamed_classes.last() {
            Some(class) => Some(self.unnamed[class.start..].to_vec()),
            None => self.enclosing.last().map(|owner| vec![owner.to_string()]),
        }
    }

    /// The types a `super` receiver stands for here.
    fn super_types_here(&self) -> Vec<String> {
        match self.unnamed_classes.last() {
            Some(class) => self.unnamed[class.start..].to_vec(),
            None => self
                .enclosing
                .last()
                .map(|owner| self.super_of(owner))
                .unwrap_or_default(),
        }
    }

    /// `X.super`: an interface `X` itself (its default method), else the
    /// superclass of `X`.
    fn super_of(&self, fqn: &str) -> Vec<String> {
        let is_interface = self
            .resolver
            .types
            .get(fqn)
            .is_some_and(|entry| entry.facts.kind == SymbolKind::Interface);
        if is_interface && self.enclosing.last() != Some(&fqn) {
            return vec![fqn.to_string()];
        }
        self.resolver.superclass(fqn).into_iter().collect()
    }

    /// An edge of `kind` to each of `targets` (at least one), marked
    /// ambiguous when there is more than one, counted as one call site; a
    /// member calling itself is resolved without an edge.
    fn calls_to(&mut self, targets: &[Method<'a>], kind: EdgeKind, line: usize) {
        let ambiguous = targets.len() > 1;
        for target in targets {
            self.push(target.id, kind, line, ambiguous);
        }
        if ambiguous {
            self.usages.calls.ambiguous += 1;
        } else {
            self.usages.calls.resolved += 1;
        }
    }

    /// A call to a member the compiler or Lombok generates: a `reference`
    /// to the type it belongs to.
    fn implicit_call(&mut self, owner: &str, line: usize) {
        self.emit(owner, EdgeKind::Reference, line);
        self.usages.calls.implicit += 1;
    }

    fn unresolved_call(&mut self, key: &str) {
        self.usages.calls.unresolved += 1;
        *self
            .usages
            .unresolved_calls
            .entry(key.to_string())
            .or_default() += 1;
    }

    /// `T.X`, `x.f`, `a().f`, `Outer.this`: a type qualifier is a
    /// reference, a field of another type a reference to the type declaring
    /// it; the result is the field's type.
    fn field_access(&mut self, node: Node<'a>) -> Ty {
        let Some(object) = node.child_by_field_name("object") else {
            return Ty::Unknown;
        };
        let Some(field) = node.child_by_field_name("field") else {
            return Ty::Unknown;
        };
        if field.kind() == "this" {
            return match name_chain(object, self.source) {
                Some(segments) => self.chain(&segments, true, EdgeKind::Reference, line(object)),
                None => {
                    self.visit(object, false);
                    Ty::Unknown
                }
            };
        }
        if let Some(segments) = name_chain(node, self.source) {
            return self.chain(&segments, false, EdgeKind::Reference, line(node));
        }
        let ty = match object.kind() {
            "super" => match self.super_types_here().as_slice() {
                [only] => Ty::Indexed(only.clone(), 0),
                _ => Ty::Unknown,
            },
            _ => self.expr(object),
        };
        self.field_of(ty, node_text(field, self.source), line(node))
    }

    /// The field `name` of a value of the type `ty`; an access to a field
    /// of another type is a reference to the type declaring it.
    fn field_of(&mut self, ty: Ty, name: &str, line: usize) -> Ty {
        match ty {
            Ty::Indexed(owner, 0) => match self.resolver.field(&owner, name) {
                Some((declarer, ty)) => {
                    self.field_owner(declarer, line);
                    ty
                }
                None => Ty::Unknown,
            },
            Ty::Indexed(_, 1..) | Ty::External(_, 1..) if name == "length" => {
                Ty::External("int".to_string(), 0)
            }
            _ => Ty::Unknown,
        }
    }

    /// A field declared by `declarer` was named: a reference unless
    /// `declarer` is a type around the node.
    fn field_owner(&mut self, declarer: &str, line: usize) {
        if !self.enclosing.contains(&declarer) {
            self.emit(declarer, EdgeKind::Reference, line);
        }
    }

    /// A dotted name in an expression, as in Java: a variable first (a
    /// local, a field, a static-imported constant — a reference to its
    /// type), then the longest prefix that is a type (a reference of
    /// `kind`), the remaining names fields. `whole` says whether the name
    /// may end on a type (a receiver or `T::m`), not on a field.
    fn chain(&mut self, segments: &[&'a str], whole: bool, kind: EdgeKind, line: usize) -> Ty {
        let Some(&first) = segments.first() else {
            return Ty::Unknown;
        };
        let (mut ty, used) = if let Some(ty) = self.variable(first, line, false) {
            (ty, 1)
        } else if !self.is_type_name(first) {
            return Ty::Unknown;
        } else {
            let typed = if whole {
                segments
            } else {
                &segments[..segments.len() - 1]
            };
            let (lookup, used) = self.resolver.resolve_prefix(self.file, self.scope(), typed);
            match lookup {
                Lookup::Found(fqn) => {
                    self.emit(&fqn, kind, line);
                    (Ty::Indexed(fqn, 0), used)
                }
                Lookup::Unknown if !first.starts_with(char::is_uppercase) => {
                    return Ty::Unknown;
                }
                lookup => {
                    self.record(lookup, first, kind, line);
                    if segments.len() == 1 {
                        return Ty::External(first.to_string(), 0);
                    }
                    return Ty::Unknown;
                }
            }
        };
        for segment in &segments[used..] {
            ty = self.field_of(ty, segment, line);
        }
        ty
    }

    /// A variable named `name` here: a local (or parameter), a field of a
    /// class around the node (a reference when it is inherited from
    /// another type), Lombok's `log`, or a static-imported constant (a
    /// reference to its type). `label` is a `case` label, where only a
    /// static field counts as a constant and an inherited field makes no
    /// edge.
    /// An anonymous or local class's own fields and inherited fields come
    /// before the variables of the code around it.
    fn variable(&mut self, name: &str, line: usize, label: bool) -> Option<Ty> {
        let mut end = self.locals.len();
        let mut fields: Option<(&'a str, Ty)> = None;
        for index in (0..self.unnamed_classes.len()).rev() {
            let class = &self.unnamed_classes[index];
            if let Some((_, ty)) = self.locals[class.locals..end]
                .iter()
                .rev()
                .find(|(local, _)| *local == name)
            {
                return Some(ty.clone());
            }
            end = class.locals;
            fields = self
                .unnamed_supers(index)
                .iter()
                .rev()
                .find_map(|owner| self.resolver.field(owner, name));
            if fields.is_some() {
                break;
            }
        }
        if fields.is_none() {
            if let Some((_, ty)) = self.locals[..end]
                .iter()
                .rev()
                .find(|(local, _)| *local == name)
            {
                return Some(ty.clone());
            }
            fields = self
                .enclosing
                .iter()
                .rev()
                .find_map(|owner| self.resolver.field(owner, name));
        }
        if let Some((declarer, ty)) = fields {
            if !label {
                self.field_owner(declarer, line);
            }
            return Some(ty);
        }
        if name == "log"
            && self.enclosing.iter().any(|owner| {
                self.resolver
                    .types
                    .get(owner)
                    .is_some_and(|entry| entry.facts.lombok.iter().any(|ann| ann == "Slf4j"))
            })
        {
            return Some(Ty::External("Logger".to_string(), 0));
        }
        let fields = if label {
            FieldFilter::StaticFields
        } else {
            FieldFilter::Statics
        };
        let owner = self.resolver.static_import_owner(self.file, name, fields)?;
        self.emit(&owner, EdgeKind::Reference, line);
        Some(
            self.resolver
                .field(&owner, name)
                .map_or(Ty::Unknown, |(_, ty)| ty),
        )
    }

    /// A bare name in an expression: a variable, its type. In a `case`
    /// label only a static field counts: in an enum switch the name is the
    /// selector's constant, whatever is imported.
    fn bare_name(&mut self, node: Node<'a>) -> Ty {
        let label = node
            .parent()
            .is_some_and(|parent| parent.kind() == "switch_label");
        self.variable(node_text(node, self.source), line(node), label)
            .unwrap_or(Ty::Unknown)
    }

    /// Whether `name` can be a type here: not `var`, a type variable or a
    /// local class.
    fn is_type_name(&self, name: &str) -> bool {
        name != "var"
            && !self.type_vars.iter().any(|var| var == name)
            && !self.region.type_vars.contains(name)
            && !self.region.types.contains(name)
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
        if let Some(dst) = self.resolver.id(fqn) {
            self.push(dst, kind, line, false);
        }
    }

    /// An edge from the current source to the row `dst`; none outside a
    /// symbol or to the source itself (recursion).
    fn push(&mut self, dst: i64, kind: EdgeKind, line: usize, ambiguous: bool) {
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

/// `ty` with `dims` more array dimensions.
fn with_dims(ty: Ty, dims: usize) -> Ty {
    match ty {
        Ty::Indexed(fqn, inner) => Ty::Indexed(fqn, inner + dims),
        Ty::External(name, inner) => Ty::External(name, inner + dims),
        other => other,
    }
}

/// The result type of a few JDK methods whose arguments often decide an
/// overload: `toString()`, the `String` factories and the `List`, `Set` and
/// `Map` factories.
fn jdk_result(receiver: &Ty, name: &str, argc: usize) -> Ty {
    let result = match (receiver, name, argc) {
        (_, "toString", 0) => "String",
        (_, "hashCode", 0) => "int",
        (_, "equals", 1) => "boolean",
        (Ty::External(owner, 0), name, _) => match (owner.as_str(), name) {
            ("String", "valueOf" | "format" | "join" | "trim" | "strip")
            | ("String", "toLowerCase" | "toUpperCase" | "substring" | "replace") => "String",
            ("Collections", "singletonList" | "emptyList" | "unmodifiableList")
            | ("List", "of" | "copyOf")
            | ("Arrays", "asList") => "List",
            ("Collections", "singleton" | "emptySet" | "unmodifiableSet")
            | ("Set", "of" | "copyOf") => "Set",
            ("Collections", "singletonMap" | "emptyMap" | "unmodifiableMap")
            | ("Map", "of" | "copyOf") => "Map",
            _ => return Ty::Unknown,
        },
        _ => return Ty::Unknown,
    };
    Ty::External(result.to_string(), 0)
}

/// A receiver type as an unresolved call's key shows it: `Optional`,
/// `?` when unknown.
fn receiver_label(ty: &Ty) -> String {
    match ty {
        Ty::Indexed(fqn, dims) => format!(
            "{}{}",
            fqn.rsplit('.').next().unwrap_or_default(),
            "[]".repeat(*dims)
        ),
        Ty::External(name, dims) => format!("{name}{}", "[]".repeat(*dims)),
        Ty::Builder(fqn) => format!("{}Builder", fqn.rsplit('.').next().unwrap_or_default()),
        Ty::Unknown | Ty::Null => "?".to_string(),
    }
}

/// Whether every class inherits a method `name` from `Object`.
fn is_object_method(name: &str) -> bool {
    matches!(
        name,
        "toString"
            | "equals"
            | "hashCode"
            | "getClass"
            | "clone"
            | "finalize"
            | "notify"
            | "notifyAll"
            | "wait"
    )
}

/// Whether every enum inherits a method `name` from `Enum`, beyond
/// `Object`'s and the members [`Resolver::implicit`] knows.
fn is_enum_method(name: &str) -> bool {
    matches!(
        name,
        "compareTo" | "getDeclaringClass" | "describeConstable"
    )
}

fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "byte" | "short" | "int" | "long" | "float" | "double" | "char" | "boolean"
    )
}

/// Whether `name` is a primitive number type or its box.
fn is_numeric(name: &str) -> bool {
    matches!(
        name,
        "byte"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "char"
            | "Byte"
            | "Short"
            | "Integer"
            | "Long"
            | "Float"
            | "Double"
            | "Character"
    )
}

/// How an argument of the type `arg` fits a parameter of the type
/// `param`; an unknown side fits. `is_subtype` answers for two indexed
/// types.
fn fits(arg: &Ty, param: &Ty, is_subtype: impl Fn(&str, &str) -> bool) -> Fit {
    match (arg, param) {
        (Ty::Null, Ty::External(name, 0)) if is_primitive(name) => Fit::No,
        (Ty::Indexed(a, da), Ty::Indexed(p, dp)) => {
            if da != dp {
                Fit::No
            } else if a == p {
                Fit::Exact
            } else if is_subtype(a, p) {
                Fit::Compatible
            } else {
                Fit::No
            }
        }
        (Ty::External(_, _), Ty::Indexed(_, _)) => Fit::No,
        (Ty::External(a, da), Ty::External(p, dp)) => {
            if p == "Object" && *dp == 0 {
                Fit::Compatible
            } else if da != dp {
                Fit::No
            } else if a == p {
                Fit::Exact
            } else if (is_numeric(a) && is_numeric(p))
                || matches!(
                    (a.as_str(), p.as_str()),
                    ("boolean", "Boolean") | ("Boolean", "boolean")
                )
                || (is_primitive(a)
                    && (matches!(p.as_str(), "Comparable" | "Serializable")
                        || (p == "Number" && is_numeric(a))))
            {
                Fit::Compatible
            } else if is_primitive(a) || is_primitive(p) || is_final_jdk(p) {
                Fit::No
            } else {
                Fit::Compatible
            }
        }
        (Ty::Indexed(_, da), Ty::External(p, dp)) => {
            if (p == "Object" && *dp == 0) || (da == dp && !is_primitive(p) && !is_final_jdk(p)) {
                Fit::Compatible
            } else {
                Fit::No
            }
        }
        _ => Fit::Compatible,
    }
}

/// Whether `nearer` overrides `farther` (declared in a type with the type
/// variables `type_vars`): the same parameter count and simple type names,
/// a type variable of `farther` matching any type.
fn overrides(nearer: &MethodFacts, farther: &MethodFacts, type_vars: &[String]) -> bool {
    let simple = |ty: &TypeRef| ty.name.rsplit('.').next().unwrap_or_default().to_string();
    nearer.params.len() == farther.params.len()
        && nearer
            .params
            .iter()
            .zip(&farther.params)
            .all(|(near, far)| {
                type_vars.contains(&far.ty.name)
                    || farther.type_params.contains(&far.ty.name)
                    || (simple(&near.ty) == simple(&far.ty) && near.ty.dims == far.ty.dims)
            })
}

/// Final JDK types: no other type is passed as one of them.
fn is_final_jdk(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "UUID"
            | "Boolean"
            | "Byte"
            | "Short"
            | "Integer"
            | "Long"
            | "Float"
            | "Double"
            | "Character"
    )
}

/// The name of the getter Lombok generates for `field`.
fn getter_name(field: &FieldFacts) -> String {
    let boolean = field.ty.name == "boolean" && field.ty.dims == 0;
    if boolean && is_prefixed(&field.name, "is") {
        return field.name.clone();
    }
    let prefix = if boolean { "is" } else { "get" };
    format!("{prefix}{}", capitalize(&field.name))
}

/// The name of the setter Lombok generates for `field`.
fn setter_name(field: &FieldFacts) -> String {
    let boolean = field.ty.name == "boolean" && field.ty.dims == 0;
    if boolean && is_prefixed(&field.name, "is") {
        return format!("set{}", &field.name[2..]);
    }
    format!("set{}", capitalize(&field.name))
}

/// Whether `name` is `prefix` followed by an uppercase letter.
fn is_prefixed(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_uppercase)
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
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

/// The array dimensions a node's `dimensions` field adds (`[]`, `[][]`).
fn dims(node: Node<'_>, source: &str) -> usize {
    node.child_by_field_name("dimensions")
        .map_or(0, |dims| node_text(dims, source).matches('[').count())
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

    /// The edges of `files` (path, source) as `src -kind-> dst :line`
    /// (`ambiguous` appended when so), sorted, and the unresolved names.
    /// Every symbol gets a row.
    fn resolve_files(files: &[(&str, &str)]) -> (Vec<String>, HashMap<String, usize>) {
        let (edges, usages) = resolve_all(files);
        (edges, usages.unresolved)
    }

    /// As [`resolve_files`], with the whole [`Usages`].
    fn resolve_all(files: &[(&str, &str)]) -> (Vec<String>, Usages) {
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
                    "{} -{}-> {} :{}{}",
                    fqns[&edge.src],
                    edge.kind.as_str(),
                    fqns[&edge.dst],
                    edge.line,
                    if edge.ambiguous { " ambiguous" } else { "" }
                )
            })
            .collect();
        edges.sort();
        edges.dedup();
        (edges, usages)
    }

    /// The `call` edges of `files`, and the `instantiate` edges to
    /// constructors.
    fn calls(files: &[(&str, &str)]) -> Vec<String> {
        edges(files)
            .into_iter()
            .filter(|edge| {
                edge.split_once(" -").is_some_and(|(_, rest)| {
                    rest.starts_with("call->")
                        || (rest.starts_with("instantiate->") && rest.contains("#<init>"))
                })
            })
            .collect()
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
                EdgeKind::Call,
            ]
            .map(EdgeKind::as_str),
            ["extends", "implements", "instantiate", "reference", "call"]
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
                "b.B#low() -call-> a.Limits#min() :8",
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
        assert_eq!(
            edges,
            [
                "a.A -extends-> a.Base :4",
                "a.A#run() -reference-> a.Base :5",
            ],
            "the inherited field is read: a reference to Base, no type edge to Config"
        );
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
                "a.U#anonymous() -reference-> a.Base :7",
                "a.U#anonymous() -reference-> a.Base.Item :7",
                "a.U#local() -reference-> a.Base :10",
                "a.U#local() -reference-> a.Base.Item :10",
                "a.U#outside() -reference-> a.Item :12",
            ]
        );
    }

    #[test]
    fn a_qualified_instance_creation_instantiates_the_member_type_of_the_receiver() {
        let (edges, unresolved) = resolve_files(&[(
            "a/U.java",
            "\
package a;
class Inner {}
class Outer { class Inner {} }
class U {
    void m(Outer o, Object x) { o.new Inner(); new Inner(); x.new Inner(); }
}
",
        )]);
        assert_eq!(
            edges,
            [
                "a.U#m(Outer,Object) -instantiate-> a.Inner :5",
                "a.U#m(Outer,Object) -instantiate-> a.Outer.Inner :5",
                "a.U#m(Outer,Object) -reference-> a.Outer :5",
            ]
        );
        assert_eq!(
            unresolved.get("Inner"),
            Some(&1),
            "x is not an indexed type"
        );
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

    #[test]
    fn the_dispatcher_reaches_the_consumer_through_its_config_getter() {
        let edges = edges(&[
            (
                "app/consumer/AddEventConsumer.java",
                "\
package app.consumer;
import app.message.AddMessage;
public class AddEventConsumer {
    public void consume(AddMessage message) {}
}
",
            ),
            (
                "app/message/AddMessage.java",
                "package app.message;\npublic class AddMessage extends app.Bearer<String> {}\n",
            ),
            (
                "app/Bearer.java",
                "package app;\npublic class Bearer<T> { public String getMessageType() { return null; } }\n",
            ),
            (
                "app/Config.java",
                "\
package app;
import app.consumer.AddEventConsumer;
public class Config {
    private final AddEventConsumer addEventConsumer;
    public Config(AddEventConsumer addEventConsumer) { this.addEventConsumer = addEventConsumer; }
    public AddEventConsumer getAddEventConsumer() { return addEventConsumer; }
}
",
            ),
            (
                "app/Dispatcher.java",
                "\
package app;
import app.message.AddMessage;
class Dispatcher {
    private final Config config;
    Dispatcher(Config config) { this.config = config; }
    void receive(java.util.List<Bearer<?>> messages) {
        messages.forEach(this::dispatch);
    }
    private void dispatch(Bearer<?> message) {
        switch (message.getMessageType()) {
            case \"ADDED\" -> config.getAddEventConsumer()
                .consume((AddMessage) message);
            default -> {}
        }
    }
}
",
            ),
        ]);
        let from_dispatcher: Vec<&str> = edges
            .iter()
            .filter(|edge| edge.starts_with("app.Dispatcher#"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            from_dispatcher,
            [
                "app.Dispatcher#<init>(Config) -reference-> app.Config :5",
                "app.Dispatcher#dispatch(Bearer<?>) -call-> app.Bearer#getMessageType() :10",
                "app.Dispatcher#dispatch(Bearer<?>) -call-> app.Config#getAddEventConsumer() :11",
                "app.Dispatcher#dispatch(Bearer<?>) -call-> app.consumer.AddEventConsumer#consume(AddMessage) :12",
                "app.Dispatcher#dispatch(Bearer<?>) -reference-> app.Bearer :9",
                "app.Dispatcher#dispatch(Bearer<?>) -reference-> app.message.AddMessage :12",
                "app.Dispatcher#receive(java.util.List<Bearer<?>>) -call-> app.Dispatcher#dispatch(Bearer<?>) :7",
                "app.Dispatcher#receive(java.util.List<Bearer<?>>) -reference-> app.Bearer :6",
            ]
        );
    }

    #[test]
    fn receivers_are_typed_by_locals_parameters_fields_new_casts_and_static_types() {
        let calls = calls(&[
            (
                "a/T.java",
                "\
package a;
class T {
    static T make() { return new T(); }
    void run() {}
    Next next() { return null; }
}
class Next { void go() {} }
class Base { void inherited() {} }
",
            ),
            (
                "a/U.java",
                "\
package a;
class U extends Base {
    T field;
    void use(T param, Object other) {
        T local = null;
        local.run();
        param.run();
        field.run();
        this.field.run();
        new T().run();
        ((T) other).run();
        var made = new T();
        made.run();
        var cast = (T) other;
        cast.run();
        T.make().next().go();
        inherited();
        { Next local2 = null; local2.go(); }
    }
}
",
            ),
        ]);
        assert_eq!(
            calls,
            [
                "a.U#use(T,Object) -call-> a.Base#inherited() :17",
                "a.U#use(T,Object) -call-> a.Next#go() :16",
                "a.U#use(T,Object) -call-> a.Next#go() :18",
                "a.U#use(T,Object) -call-> a.T#make() :16",
                "a.U#use(T,Object) -call-> a.T#next() :16",
                "a.U#use(T,Object) -call-> a.T#run() :10",
                "a.U#use(T,Object) -call-> a.T#run() :11",
                "a.U#use(T,Object) -call-> a.T#run() :13",
                "a.U#use(T,Object) -call-> a.T#run() :15",
                "a.U#use(T,Object) -call-> a.T#run() :6",
                "a.U#use(T,Object) -call-> a.T#run() :7",
                "a.U#use(T,Object) -call-> a.T#run() :8",
                "a.U#use(T,Object) -call-> a.T#run() :9",
            ]
        );
    }

    #[test]
    fn unqualified_calls_look_in_the_current_then_the_enclosing_types() {
        let calls = calls(&[(
            "a/Outer.java",
            "\
package a;
class Outer {
    void shared() {}
    void outerOnly() {}
    class Inner {
        void shared() {}
        void run() {
            shared();
            outerOnly();
            Outer.this.shared();
            run();
        }
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.Outer.Inner#run() -call-> a.Outer#outerOnly() :9",
                "a.Outer.Inner#run() -call-> a.Outer#shared() :10",
                "a.Outer.Inner#run() -call-> a.Outer.Inner#shared() :8",
            ],
            "the recursive run() makes no edge"
        );
    }

    #[test]
    fn super_calls_and_explicit_constructor_calls() {
        let calls = calls(&[(
            "a/Child.java",
            "\
package a;
class Base {
    Base() {}
    Base(String name) {}
    void m() {}
}
class Child extends Base {
    Child() { super(\"x\"); }
    Child(int n) { this(); }
    @Override void m() { super.m(); }
    Runnable ref() { return super::m; }
}
class Plain { }
class Sub extends Plain { Sub() { super(); } }
",
        )]);
        assert_eq!(
            calls,
            [
                "a.Child#<init>() -call-> a.Base#<init>(String) :8",
                "a.Child#<init>(int) -call-> a.Child#<init>() :9",
                "a.Child#m() -call-> a.Base#m() :10",
                "a.Child#ref() -call-> a.Base#m() :11",
            ]
        );
        let edges = edges(&[(
            "a/Sub.java",
            "package a;\nclass Plain { }\nclass Sub extends Plain { Sub() { super(); } }\n",
        )]);
        assert!(
            edges.contains(&"a.Sub#<init>() -reference-> a.Plain :3".to_string()),
            "an implicit superclass constructor is a reference to the type: {edges:?}"
        );
    }

    #[test]
    fn a_chain_stops_at_a_type_outside_the_index_a_lambda_parameter_or_a_type_variable() {
        let (edges, usages) = resolve_all(&[
            (
                "a/Repo.java",
                "\
package a;
import java.util.Optional;
import java.util.List;
class Item { void use() {} }
class Box<T> { T get() { return null; } Item item() { return null; } }
class Repo {
    Optional<Item> find() { return null; }
    List<Item> all() { return null; }
}
",
            ),
            (
                "a/S.java",
                "\
package a;
class S {
    Repo repo;
    Box<Item> box;
    void run() {
        repo.find().map(i -> i).get().use();
        repo.all().forEach(item -> item.use());
        repo.all().forEach((Item item) -> item.use());
        box.get().use();
        box.item().use();
    }
}
",
            ),
        ]);
        let from_run: Vec<&str> = edges
            .iter()
            .filter(|edge| edge.starts_with("a.S#run() -call->"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            from_run,
            [
                "a.S#run() -call-> a.Box#get() :9",
                "a.S#run() -call-> a.Box#item() :10",
                "a.S#run() -call-> a.Item#use() :10",
                "a.S#run() -call-> a.Item#use() :8",
                "a.S#run() -call-> a.Repo#all() :7",
                "a.S#run() -call-> a.Repo#all() :8",
                "a.S#run() -call-> a.Repo#find() :6",
            ],
            "Optional#map, an untyped lambda parameter and T stop the chain"
        );
        let mut unresolved: Vec<(&str, usize)> = usages
            .unresolved_calls
            .iter()
            .map(|(key, count)| (key.as_str(), *count))
            .collect();
        unresolved.sort();
        assert_eq!(
            unresolved,
            [
                ("?#get", 1),
                ("?#use", 3),
                ("List#forEach", 2),
                ("Optional#map", 1),
            ]
        );
        assert_eq!(
            usages.calls,
            CallSites {
                resolved: 7,
                implicit: 0,
                ambiguous: 0,
                unresolved: 7,
            }
        );
    }

    #[test]
    fn overloads_are_chosen_by_count_then_argument_types_else_all_are_ambiguous() {
        let calls = calls(&[
            (
                "a/Api.java",
                "\
package a;
class Msg {}
class SubMsg extends Msg {}
class Other {}
class Api {
    void count() {}
    void count(int n) {}
    void count(int n, int m) {}
    void typed(String s) {}
    void typed(Msg m) {}
    void typed(Other o) {}
    void sub(Msg m) {}
    void sub(SubMsg m) {}
    void spread(String first, Object... rest) {}
    void either(String s) {}
    void either(java.util.List<String> s) {}
    void unknown(Msg m) {}
    void unknown(Other o) {}
}
",
            ),
            (
                "a/U.java",
                "\
package a;
class U {
    void run(Api api, Object x) {
        api.count(1, 2);
        api.typed(\"s\");
        api.typed(new Other());
        api.sub(new SubMsg());
        api.spread(\"a\");
        api.spread(\"a\", 1, 2);
        api.either(String.valueOf(x));
        api.unknown(x.hashCode() > 0 ? null : null);
    }
}
",
            ),
        ]);
        assert_eq!(
            calls,
            [
                "a.U#run(Api,Object) -call-> a.Api#count(int,int) :4",
                "a.U#run(Api,Object) -call-> a.Api#either(String) :10",
                "a.U#run(Api,Object) -call-> a.Api#spread(String,Object...) :8",
                "a.U#run(Api,Object) -call-> a.Api#spread(String,Object...) :9",
                "a.U#run(Api,Object) -call-> a.Api#sub(SubMsg) :7",
                "a.U#run(Api,Object) -call-> a.Api#typed(Other) :6",
                "a.U#run(Api,Object) -call-> a.Api#typed(String) :5",
                "a.U#run(Api,Object) -call-> a.Api#unknown(Msg) :11 ambiguous",
                "a.U#run(Api,Object) -call-> a.Api#unknown(Other) :11 ambiguous",
            ]
        );
    }

    #[test]
    fn constructors_are_chosen_like_methods_and_implicit_ones_are_the_type() {
        let calls = calls(&[(
            "a/E.java",
            "\
package a;
import java.util.Collections;
class Event {
    Event(String id) {}
    Event(java.util.List<String> ids) {}
}
class Plain {}
class U {
    void run() {
        new Event(\"id\");
        new Event(Collections.singletonList(\"id\"));
        new Plain();
        java.util.function.Function<String, Event> f = Event::new;
        new Event(\"x\") {};
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#run() -instantiate-> a.Event#<init>(String) :10",
                "a.U#run() -instantiate-> a.Event#<init>(String) :13 ambiguous",
                "a.U#run() -instantiate-> a.Event#<init>(String) :14",
                "a.U#run() -instantiate-> a.Event#<init>(java.util.List<String>) :11",
                "a.U#run() -instantiate-> a.Event#<init>(java.util.List<String>) :13 ambiguous",
            ]
        );
    }

    #[test]
    fn method_references_and_static_imports_resolve_to_methods() {
        let calls = calls(&[
            (
                "a/Util.java",
                "\
package a;
public class Util {
    public static void helper() {}
    public static int twice(int n) { return n; }
    public static int twice(int n, int m) { return n; }
}
",
            ),
            (
                "a/Tools.java",
                "package a;\npublic class Tools { public static void tool() {} }\n",
            ),
            (
                "b/U.java",
                "\
package b;
import static a.Util.helper;
import static a.Util.twice;
import static a.Tools.*;
import a.Util;
class U {
    U other;
    void own() {}
    void run() {
        helper();
        twice(1);
        tool();
        Runnable r1 = this::own;
        Runnable r2 = other::own;
        Runnable r3 = Util::helper;
        java.util.function.IntUnaryOperator r4 = Util::twice;
    }
}
",
            ),
        ]);
        assert_eq!(
            calls,
            [
                "b.U#run() -call-> a.Tools#tool() :12",
                "b.U#run() -call-> a.Util#helper() :10",
                "b.U#run() -call-> a.Util#helper() :15",
                "b.U#run() -call-> a.Util#twice(int) :11",
                "b.U#run() -call-> a.Util#twice(int) :16 ambiguous",
                "b.U#run() -call-> a.Util#twice(int,int) :16 ambiguous",
                "b.U#run() -call-> b.U#own() :13",
                "b.U#run() -call-> b.U#own() :14",
            ]
        );
    }

    #[test]
    fn lombok_record_and_enum_members_are_references_to_their_type_and_carry_the_chain() {
        let edges = edges(&[
            (
                "a/Model.java",
                "\
package a;
import lombok.Builder;
import lombok.Data;
import lombok.Getter;
import lombok.Setter;
@Getter
class Holder { private Target target; private boolean active; }
class Fields { @Getter private Target target; @Setter private String name; private Target hidden; }
@Data
@Builder
class Dto { private Target target; }
class Target { void hit() {} }
record Point(Target target) {}
enum Color { RED }
",
            ),
            (
                "a/U.java",
                "\
package a;
class U {
    void run(Holder holder, Fields fields, Point point) {
        holder.getTarget().hit();
        holder.isActive();
        fields.getTarget().hit();
        fields.setName(\"n\");
        fields.getHidden();
        Dto.builder().target(null).build().getTarget().hit();
        point.target().hit();
        new Point(null);
        Color.valueOf(\"RED\").name();
        Color[] all = Color.values();
    }
}
",
            ),
        ]);
        let from_run: Vec<&str> = edges
            .iter()
            .filter_map(|edge| edge.strip_prefix("a.U#run(Holder,Fields,Point) "))
            .collect();
        assert_eq!(
            from_run,
            [
                "-call-> a.Target#hit() :10",
                "-call-> a.Target#hit() :4",
                "-call-> a.Target#hit() :6",
                "-call-> a.Target#hit() :9",
                "-instantiate-> a.Point :11",
                "-reference-> a.Color :12",
                "-reference-> a.Color :13",
                "-reference-> a.Dto :9",
                "-reference-> a.Fields :3",
                "-reference-> a.Fields :6",
                "-reference-> a.Fields :7",
                "-reference-> a.Holder :3",
                "-reference-> a.Holder :4",
                "-reference-> a.Holder :5",
                "-reference-> a.Point :10",
                "-reference-> a.Point :3",
            ],
            "getHidden() has no Lombok getter: no edge"
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
    fn a_field_of_another_type_is_a_reference_to_the_type_declaring_it() {
        let edges = edges(&[(
            "a/U.java",
            "\
package a;
class Base { int inherited; }
class Cfg extends Base { String queueName; static final int MAX = 1; }
class U {
    int own;
    void run(Cfg cfg) {
        String name = cfg.queueName;
        int n = cfg.inherited;
        int mine = this.own + own;
    }
}
",
        )]);
        let from_run: Vec<&str> = edges
            .iter()
            .filter_map(|edge| edge.strip_prefix("a.U#run(Cfg) "))
            .collect();
        assert_eq!(
            from_run,
            [
                "-reference-> a.Base :8",
                "-reference-> a.Cfg :6",
                "-reference-> a.Cfg :7",
            ]
        );
    }

    #[test]
    fn a_class_extending_a_type_outside_the_index_stops_an_unqualified_lookup() {
        let (edges, usages) = resolve_all(&[(
            "a/U.java",
            "\
package a;
class Outer {
    void start() {}
    class Worker extends Thread {
        void work() { start(); }
    }
    class Plain {
        void work() { start(); }
    }
    void anonymous() {
        new Thread() { public void run() { start(); } };
        new Plain() { void start() {} void go() { start(); } };
    }
}
",
        )]);
        let calls: Vec<&str> = edges
            .iter()
            .filter(|edge| edge.contains("-call->"))
            .map(String::as_str)
            .collect();
        assert_eq!(calls, ["a.Outer.Plain#work() -call-> a.Outer#start() :8"]);
        assert_eq!(
            usages.unresolved_calls.get("this#start"),
            Some(&3),
            "the anonymous Plain's own start() is not a symbol either"
        );
    }

    /// The edges that contain `part`.
    fn edges_with(edges: &[String], part: &str) -> Vec<String> {
        edges
            .iter()
            .filter(|edge| edge.contains(part))
            .cloned()
            .collect()
    }

    /// The unresolved call keys of `usages`, sorted.
    fn unresolved_calls(usages: &Usages) -> Vec<(String, usize)> {
        let mut keys: Vec<(String, usize)> = usages
            .unresolved_calls
            .iter()
            .map(|(key, count)| (key.clone(), *count))
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn an_indexed_argument_never_fits_a_string_box_or_primitive_parameter() {
        let calls = calls(&[(
            "a/V.java",
            "\
package a;
class Foo {}
class V {
    void m(Foo... fs) {}
    void m(String s) {}
    void n(int i) {}
    void n(Foo f) {}
    void w(Number n) {}
}
class U {
    void f(V v) {
        v.m(new Foo(), new Foo());
        v.m(new Foo());
        v.n(new Foo());
        v.w(1);
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#f(V) -call-> a.V#m(Foo...) :12",
                "a.U#f(V) -call-> a.V#m(Foo...) :13",
                "a.U#f(V) -call-> a.V#n(Foo) :14",
                "a.U#f(V) -call-> a.V#w(Number) :15",
            ]
        );
    }

    #[test]
    fn a_single_candidate_the_arguments_do_not_fit_is_no_edge() {
        let (edges, usages) = resolve_all(&[(
            "a/C.java",
            "\
package a;
import lombok.AllArgsConstructor;
class Foo {}
class C {
    public boolean equals(Foo other) { return false; }
}
class D extends java.util.ArrayList<String> {
    public boolean add(Foo f) { return true; }
}
class Plain { Plain(Foo f) {} }
@AllArgsConstructor
class Generated { String name; Generated(Foo f) {} }
class U {
    void f(C c, D d, Object o) {
        c.equals(o);
        c.equals(\"s\");
        d.add(\"x\");
        c.equals(new Foo());
        new Plain(\"x\");
        new Generated(\"x\");
    }
}
",
        )]);
        let calls: Vec<String> = edges
            .iter()
            .filter(|edge| {
                edge.starts_with("a.U#") && edge.contains("#<init>") || edge.contains("-call->")
            })
            .cloned()
            .collect();
        assert_eq!(calls, ["a.U#f(C,D,Object) -call-> a.C#equals(Foo) :18"]);
        assert_eq!(
            unresolved_calls(&usages),
            [
                ("C#equals".to_string(), 2),
                ("D#add".to_string(), 1),
                ("Plain#<init>".to_string(), 1),
            ],
            "Object#equals and ArrayList#add are not in the index; Lombok's constructor and Foo's default one are implicit"
        );
        assert_eq!(
            usages.calls,
            CallSites {
                resolved: 1,
                implicit: 2,
                ambiguous: 0,
                unresolved: 4,
            }
        );
    }

    #[test]
    fn object_and_enum_methods_called_unqualified_stay_in_the_innermost_class() {
        let (edges, usages) = resolve_all(&[(
            "a/O.java",
            "\
package a;
interface Api { void run(); }
class O {
    public String toString() { return \"\"; }
    public boolean equals(Object o) { return false; }
    int compareTo(Object o) { return 0; }
    void own() {}
    static class In {
        String f() { return toString(); }
        boolean g(Object o) { return equals(o); }
    }
    void h() {
        Api a = new Api() { public void run() { toString(); own(); } };
    }
    enum E { A; int f() { return compareTo(A); } }
}
",
        )]);
        assert_eq!(
            edges_with(&edges, "-call->"),
            ["a.O#h() -call-> a.O#own() :13"]
        );
        assert_eq!(
            unresolved_calls(&usages),
            [
                ("this#compareTo".to_string(), 1),
                ("this#equals".to_string(), 1),
                ("this#toString".to_string(), 2),
            ]
        );
    }

    #[test]
    fn overloads_of_one_type_do_not_override_each_other() {
        let calls = calls(&[(
            "a/B.java",
            "\
package a;
class Item {}
class B<T> {
    void put(String s) {}
    void put(T t) {}
}
class U {
    void f(B<Item> b) { b.put(new Item()); b.put(\"s\"); }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#f(B<Item>) -call-> a.B#put(String) :8",
                "a.U#f(B<Item>) -call-> a.B#put(T) :8",
            ],
            "put(T) takes the Item, put(String) wins on the exact String"
        );
    }

    #[test]
    fn exact_matches_decide_only_on_known_types_and_fixed_arity_comes_first() {
        let calls = calls(&[(
            "a/K.java",
            "\
package a;
class Foo {}
class K {
    void m(String s, Foo f) {}
    void m(Integer i, Object o) {}
    void v(Object a, Object b) {}
    void v(Foo a, Foo... rest) {}
}
class U {
    void f(K k, Foo foo, java.util.List<Integer> xs) {
        k.m(xs.get(0), foo);
        k.m(\"s\", foo);
        k.v(foo, foo);
        k.v(foo, foo, foo);
    }
}
",
        )]);
        assert_eq!(
            calls,
            [
                "a.U#f(K,Foo,java.util.List<Integer>) -call-> a.K#m(Integer,Object) :11 ambiguous",
                "a.U#f(K,Foo,java.util.List<Integer>) -call-> a.K#m(String,Foo) :11 ambiguous",
                "a.U#f(K,Foo,java.util.List<Integer>) -call-> a.K#m(String,Foo) :12",
                "a.U#f(K,Foo,java.util.List<Integer>) -call-> a.K#v(Foo,Foo...) :14",
                "a.U#f(K,Foo,java.util.List<Integer>) -call-> a.K#v(Object,Object) :13",
            ]
        );
    }

    #[test]
    fn an_inherited_superclass_method_comes_before_an_interface_method() {
        let calls = calls(&[(
            "a/I.java",
            "\
package a;
interface I { void m(); }
class A { public void m() {} }
class B extends A {}
class C extends B implements I {}
class U {
    void f(C c) { c.m(); }
}
",
        )]);
        assert_eq!(calls, ["a.U#f(C) -call-> a.A#m() :7"]);
    }

    #[test]
    fn implicit_members_and_recursion_are_counted_apart_from_call_edges() {
        let (edges, usages) = resolve_all(&[(
            "a/R.java",
            "\
package a;
import lombok.Getter;
@Getter
class R {
    String name;
    void f() { f(); g(); getName(); new R(); }
    void g() {}
}
",
        )]);
        assert_eq!(
            edges_with(&edges, "-call->"),
            ["a.R#f() -call-> a.R#g() :6"]
        );
        assert_eq!(
            usages.calls,
            CallSites {
                resolved: 2,
                implicit: 2,
                ambiguous: 0,
                unresolved: 0,
            },
            "f() calls itself: resolved, no edge"
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

    #[test]
    fn annotation_elements_and_builder_object_methods_reference_their_type() {
        let (edges, usages) = resolve_all(&[(
            "a/N.java",
            "\
package a;
import lombok.Builder;
@interface Ann { String type(); }
@Builder
class L {
    String name;
    public static class LBuilder { LBuilder upper(String s) { return this; } }
}
class N {
    void f(Ann a) { a.type(); a.toString(); }
    void g() { L.builder().name(\"x\").toString().length(); }
    void h() { L.builder().upper(\"x\").name(\"y\").build(); }
}
",
        )]);
        assert_eq!(
            edges_with(&edges, "a.N#"),
            [
                "a.N#f(Ann) -reference-> a.Ann :10",
                "a.N#g() -reference-> a.L :11",
                "a.N#h() -call-> a.L.LBuilder#upper(String) :12",
                "a.N#h() -reference-> a.L :12",
            ]
        );
        assert_eq!(
            unresolved_calls(&usages),
            [
                ("Ann#toString".to_string(), 1),
                ("String#length".to_string(), 1),
            ]
        );
    }
}
