//! Methods and constructors: lookup through the super types, implicit
//! members (Lombok, records, enums), overload choice and the JDK results
//! that decide one.

use crate::symbols::{FieldFacts, MethodFacts, SymbolKind, TypeFacts, TypeRef};

use super::resolver::{Resolver, Ty, is_primitive, with_dims};

/// A method or constructor with a row.
#[derive(Clone, Copy)]
pub(super) struct Method<'a> {
    pub(super) id: i64,
    pub(super) facts: &'a MethodFacts,
    /// The fqn of the type declaring it.
    pub(super) owner: &'a str,
}

/// How an argument fits a parameter type.
enum Fit {
    Exact,
    Compatible,
    No,
}

impl<'a> Resolver<'a> {
    /// The methods called `name` that `owners` have or inherit from their
    /// indexed super types; a method overridden nearer is left out (same
    /// parameter count and simple type names, a type variable of the
    /// farther one matching any type), the overloads of one type never.
    pub(super) fn methods(&self, owners: &[String], name: &str) -> Vec<Method<'a>> {
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
    pub(super) fn constructors(&self, owner: &str) -> Vec<Method<'a>> {
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
    pub(super) fn static_methods(&self, file: usize, name: &str) -> Vec<Method<'a>> {
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
    pub(super) fn implicit(
        &self,
        owners: &[String],
        name: &str,
        argc: usize,
    ) -> Option<(&'a str, Ty)> {
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
    pub(super) fn generates_constructors(&self, owner: &str) -> bool {
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
    pub(super) fn choose(&self, candidates: Vec<Method<'a>>, args: &[Ty]) -> Vec<Method<'a>> {
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

/// The result type of a few JDK methods whose arguments often decide an
/// overload: `toString()`, the `String` factories and the `List`, `Set` and
/// `Map` factories.
pub(super) fn jdk_result(receiver: &Ty, name: &str, argc: usize) -> Ty {
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

/// Whether every class inherits a method `name` from `Object`.
pub(super) fn is_object_method(name: &str) -> bool {
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
pub(super) fn is_enum_method(name: &str) -> bool {
    matches!(
        name,
        "compareTo" | "getDeclaringClass" | "describeConstable"
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

#[cfg(test)]
mod tests {
    use crate::usages::CallSites;
    use crate::usages::tests::{calls, edges, edges_with, resolve_all, unresolved_calls};

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
