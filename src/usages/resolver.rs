//! The run's type table: simple and qualified type names resolved in
//! Java's order, the indexed super types (`lineage`) and fields.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::symbols::{TypeFacts, TypeRef};

use super::SourceFile;

/// An indexed type: its row, facts and file.
pub(super) struct TypeEntry<'a> {
    id: i64,
    pub(super) facts: &'a TypeFacts,
    file: usize,
}

/// Where a name is looked up: the super types of the anonymous and local
/// classes around it (innermost last), then the named type around it.
#[derive(Clone, Copy)]
pub(super) struct Scope<'s> {
    pub(super) unnamed: &'s [String],
    pub(super) enclosing: Option<&'s str>,
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
pub(super) enum FieldFilter {
    /// Fields, enum constants and record components.
    Any,
    /// Static fields and enum constants.
    Statics,
    /// Static fields only.
    StaticFields,
}

/// The outcome of looking a type name up.
pub(super) enum Lookup {
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
pub(super) enum Ty {
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

/// What a type variable of a super type stands for in a subtype
/// ([`Resolver::type_args`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Arg {
    Type(Ty),
    /// A type variable of the subtype, by name, with array dimensions
    /// (`Pair<Sms, P>` in `class Flipped<P>`).
    Var(String, usize),
}

impl Ty {
    /// Whether the type is known well enough to rank overloads by it.
    pub(super) fn is_known(&self) -> bool {
        !matches!(self, Ty::Unknown | Ty::Builder(_))
    }
}

/// The run's type table and the lookups on it.
pub(super) struct Resolver<'a> {
    pub(super) files: &'a [SourceFile<'a>],
    pub(super) types: HashMap<&'a str, TypeEntry<'a>>,
    /// The methods and constructors with a row, by fqn.
    pub(super) members: HashMap<&'a str, i64>,
    /// Per type: whether its superclass resolved (then it is the first
    /// super type), and its indexed super types.
    supertypes: RefCell<HashMap<String, (bool, Vec<String>)>>,
    pending: RefCell<HashSet<String>>,
}

impl<'a> Resolver<'a> {
    /// The table of every type with a row; for a duplicate fqn, the facts of
    /// the definition that was written.
    pub(super) fn new(files: &'a [SourceFile<'a>]) -> Self {
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

    pub(super) fn id(&self, fqn: &str) -> Option<i64> {
        self.types.get(fqn).map(|entry| entry.id)
    }

    /// A simple type name in `file`, in `scope`, in `javac`'s order.
    pub(super) fn resolve_simple(&self, file: usize, scope: Scope<'_>, name: &str) -> Lookup {
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
    pub(super) fn resolve_prefix(
        &self,
        file: usize,
        scope: Scope<'_>,
        segments: &[&str],
    ) -> (Lookup, usize) {
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
    pub(super) fn resolve_type(&self, file: usize, scope: Scope<'_>, segments: &[&str]) -> Lookup {
        match self.resolve_prefix(file, scope, segments) {
            (Lookup::Found(fqn), used) if used == segments.len() => Lookup::Found(fqn),
            (Lookup::Found(_), _) => Lookup::Unknown,
            (other, _) => other,
        }
    }

    /// The type `name` nested in `owner` or inherited from its indexed
    /// super types.
    pub(super) fn member_type(&self, owner: &str, name: &str) -> Option<String> {
        self.lineage(&[owner.to_string()])
            .iter()
            .map(|entry| format!("{}.{name}", entry.facts.fqn))
            .find(|fqn| self.types.contains_key(fqn.as_str()))
    }

    /// `owners` and their indexed super types in the order Java looks a
    /// member up: the indexed superclass chain of each owner first, then
    /// the interfaces breadth-first; each type once.
    pub(super) fn lineage(&self, owners: &[String]) -> Vec<&TypeEntry<'a>> {
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
        let resolve = |ty: &TypeRef| self.super_ref(entry, ty);
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

    /// A super type `ty` as written in the header of `entry`, resolved in
    /// its file around it; `None` for a type variable or a type outside
    /// the index.
    fn super_ref(&self, entry: &TypeEntry<'a>, ty: &TypeRef) -> Option<String> {
        let facts = entry.facts;
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
    }

    /// What `sup`'s type variables stand for in its subtype `sub`: the
    /// type arguments of each super type on the way from `sub` up,
    /// resolved in the file of the type that writes them around it, a type
    /// variable among them replaced by what the type below gives it, one
    /// of `sub` kept as a variable. A raw super type and a wildcard leave a
    /// variable out (unknown). `None` when `sup` is not `sub` or one of its
    /// indexed super types.
    pub(super) fn type_args(&self, sub: &str, sup: &str) -> Option<HashMap<String, Arg>> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, HashMap<String, Arg>)> = VecDeque::new();
        queue.push_back((sub.to_string(), HashMap::new()));
        while let Some((fqn, args)) = queue.pop_front() {
            if fqn == sup {
                return Some(args);
            }
            if !seen.insert(fqn.clone()) {
                continue;
            }
            let Some(entry) = self.types.get(fqn.as_str()) else {
                continue;
            };
            let facts = entry.facts;
            for written in facts.superclass.iter().chain(&facts.interfaces) {
                let Some(next) = self.super_ref(entry, written) else {
                    continue;
                };
                let Some(next_entry) = self.types.get(next.as_str()) else {
                    continue;
                };
                let mut next_args = HashMap::new();
                for (var, arg) in next_entry.facts.type_params.iter().zip(&written.args) {
                    let found = if facts.type_params.contains(&arg.name) && arg.args.is_empty() {
                        if fqn == sub {
                            Some(Arg::Var(arg.name.clone(), arg.dims))
                        } else {
                            args.get(&arg.name).cloned().map(|found| match found {
                                Arg::Type(ty) => Arg::Type(with_dims(ty, arg.dims)),
                                Arg::Var(name, dims) => Arg::Var(name, dims + arg.dims),
                            })
                        }
                    } else {
                        match self.declared_in(&fqn, facts.parent.as_deref(), arg, &[]) {
                            Ty::Unknown => None,
                            ty => Some(Arg::Type(ty)),
                        }
                    };
                    if let Some(found) = found {
                        next_args.insert(var.clone(), found);
                    }
                }
                queue.push_back((next, next_args));
            }
        }
        None
    }

    /// The type a declared `ty` of a member of `owner` stands for on a
    /// receiver of the type `receiver`: a type variable of `owner` is the
    /// type argument `receiver` gives it ([`Resolver::type_args`]), anything
    /// else as [`Resolver::declared`].
    pub(super) fn declared_on(
        &self,
        receiver: &str,
        owner: &str,
        ty: &TypeRef,
        member_vars: &[String],
    ) -> Ty {
        let is_owner_var = !member_vars.contains(&ty.name)
            && self
                .types
                .get(owner)
                .is_some_and(|entry| entry.facts.type_params.contains(&ty.name));
        if is_owner_var && ty.args.is_empty() {
            return self
                .type_args(receiver, owner)
                .and_then(|args| args.get(&ty.name).cloned())
                .map_or(Ty::Unknown, |found| match found {
                    Arg::Type(found) => with_dims(found, ty.dims),
                    Arg::Var(..) => Ty::Unknown,
                });
        }
        self.declared(owner, ty, member_vars)
    }

    /// The type a declared `ty` of a member of `declarer` stands for on a
    /// receiver of one of the types `owners`: through the first of them
    /// that is a subtype of `declarer` ([`Resolver::declared_on`]), else as
    /// declared.
    pub(super) fn declared_via(
        &self,
        owners: &[String],
        declarer: &str,
        ty: &TypeRef,
        member_vars: &[String],
    ) -> Ty {
        match owners
            .iter()
            .find(|owner| owner.as_str() != declarer && self.is_subtype(owner, declarer))
        {
            Some(receiver) => self.declared_on(receiver, declarer, ty, member_vars),
            None => self.declared(declarer, ty, member_vars),
        }
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
    pub(super) fn static_import_owner(
        &self,
        file: usize,
        name: &str,
        fields: FieldFilter,
    ) -> Option<String> {
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
    pub(super) fn superclass(&self, fqn: &str) -> Option<String> {
        match self.supers(fqn) {
            (true, supertypes) => supertypes.into_iter().next(),
            _ => None,
        }
    }

    /// Whether `fqn` or a superclass of it in the index extends a type
    /// outside the index, whose members are unknown.
    pub(super) fn has_external_superclass(&self, fqn: &str) -> bool {
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
    pub(super) fn is_subtype(&self, sub: &str, sup: &str) -> bool {
        self.lineage(&[sub.to_string()])
            .iter()
            .any(|entry| entry.facts.fqn == sup)
    }

    /// The type a declared `ty` of a member or field of `owner` stands for,
    /// resolved in `owner`'s file inside it; a type variable (of the member,
    /// `owner` or a type around it) or a wildcard is unknown.
    pub(super) fn declared(&self, owner: &str, ty: &TypeRef, member_vars: &[String]) -> Ty {
        self.declared_in(owner, Some(owner), ty, member_vars)
    }

    /// [`Resolver::declared`] with names looked up around `scope`: `owner`
    /// for a member, the type around it for its header.
    fn declared_in(
        &self,
        owner: &str,
        scope: Option<&str>,
        ty: &TypeRef,
        member_vars: &[String],
    ) -> Ty {
        if is_primitive(&ty.name) || ty.name == "void" {
            return Ty::External(ty.name.clone(), ty.dims);
        }
        if ty.name == "?" || member_vars.contains(&ty.name) {
            return Ty::Unknown;
        }
        let Some(entry) = self.types.get(owner) else {
            return Ty::Unknown;
        };
        if self.is_type_var(owner, &ty.name) {
            return Ty::Unknown;
        }
        let segments: Vec<&str> = ty.name.split('.').collect();
        match self.resolve_type(entry.file, Scope::around(scope), &segments) {
            Lookup::Found(fqn) => Ty::Indexed(fqn, ty.dims),
            Lookup::Ambiguous(_) => Ty::Unknown,
            Lookup::External | Lookup::Unknown => {
                Ty::External(segments[segments.len() - 1].to_string(), ty.dims)
            }
        }
    }

    /// Whether `name` is a type variable of `owner` or of a type around it.
    pub(super) fn is_type_var(&self, owner: &str, name: &str) -> bool {
        let mut current = self.types.get(owner).map(|entry| entry.facts);
        while let Some(facts) = current {
            if facts.type_params.iter().any(|var| var == name) {
                return true;
            }
            current = facts
                .parent
                .as_deref()
                .and_then(|parent| self.types.get(parent))
                .map(|entry| entry.facts);
        }
        false
    }

    /// The package of the file declaring the indexed type `fqn`.
    pub(super) fn package(&self, fqn: &str) -> Option<&'a str> {
        self.types
            .get(fqn)
            .map(|entry| self.files[entry.file].package)
    }

    /// The field (or enum constant, or record component) `name` of `owner`
    /// or its indexed super types: the type declaring it and its type, a
    /// type variable of an inherited field replaced by the type argument
    /// `owner` gives it.
    pub(super) fn field(&self, owner: &str, name: &str) -> Option<(&'a str, Ty)> {
        let owners = [owner.to_string()];
        self.lineage(&owners).iter().find_map(|entry| {
            let facts = entry.facts;
            let declarer = facts.fqn.as_str();
            if let Some(field) = facts.fields.iter().find(|field| field.name == name) {
                return Some((
                    declarer,
                    self.declared_via(&owners, declarer, &field.ty, &[]),
                ));
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
}

/// `ty` with `dims` more array dimensions.
pub(super) fn with_dims(ty: Ty, dims: usize) -> Ty {
    match ty {
        Ty::Indexed(fqn, inner) => Ty::Indexed(fqn, inner + dims),
        Ty::External(name, inner) => Ty::External(name, inner + dims),
        other => other,
    }
}

pub(super) fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "byte" | "short" | "int" | "long" | "float" | "double" | "char" | "boolean"
    )
}

#[cfg(test)]
mod tests {
    use crate::usages::tests::{edges, resolve_files};

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
