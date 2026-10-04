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
//! the chain on. A member inherited from a generic super type has, on a
//! subtype, the type argument the subtype gives its type variable; a
//! variable typed by a bounded type variable of its method has the bound's
//! type. A receiver outside the index (a library type, an untyped lambda
//! parameter, an unbound type variable) stops it: no edge, counted by
//! `Receiver#name`.
//!
//! Last, every method gets an `overrides` edge to each method it overrides
//! nearest in its indexed super types (same name and parameter count,
//! parameter types equal once the super types' type arguments are
//! substituted).

mod expr;
mod members;
mod resolver;
mod walk;

use std::collections::HashMap;

use tree_sitter::Tree;

use crate::symbols::{Import, Symbol, TypeFacts};

use resolver::Resolver;
use walk::{FileWalk, Region};

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
    /// A method to the method it overrides in an indexed super type.
    Overrides,
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
            EdgeKind::Overrides => "overrides",
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
    usages.edges.extend(resolver.override_edges());
    usages
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::symbols::{JavaParser, ParsedFile};

    /// The edges of `files` (path, source) as `src -kind-> dst :line`
    /// (`ambiguous` appended when so), sorted, and the unresolved names.
    /// Every symbol gets a row.
    pub(super) fn resolve_files(files: &[(&str, &str)]) -> (Vec<String>, HashMap<String, usize>) {
        let (edges, usages) = resolve_all(files);
        (edges, usages.unresolved)
    }

    /// As [`resolve_files`], with the whole [`Usages`].
    pub(super) fn resolve_all(files: &[(&str, &str)]) -> (Vec<String>, Usages) {
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
    pub(super) fn calls(files: &[(&str, &str)]) -> Vec<String> {
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

    pub(super) fn edges(files: &[(&str, &str)]) -> Vec<String> {
        resolve_files(files).0
    }

    /// The edges that contain `part`.
    pub(super) fn edges_with(edges: &[String], part: &str) -> Vec<String> {
        edges
            .iter()
            .filter(|edge| edge.contains(part))
            .cloned()
            .collect()
    }

    /// The unresolved call keys of `usages`, sorted.
    pub(super) fn unresolved_calls(usages: &Usages) -> Vec<(String, usize)> {
        let mut keys: Vec<(String, usize)> = usages
            .unresolved_calls
            .iter()
            .map(|(key, count)| (key.clone(), *count))
            .collect();
        keys.sort();
        keys
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
                EdgeKind::Overrides,
            ]
            .map(EdgeKind::as_str),
            [
                "extends",
                "implements",
                "instantiate",
                "reference",
                "call",
                "overrides"
            ]
        );
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
}
