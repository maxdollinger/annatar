//! Expression typing on the walk: receivers, calls, method references,
//! field access and names, each typed and turned into edges.

use tree_sitter::Node;

use crate::symbols::SymbolKind;

use super::EdgeKind;
use super::members::{Method, is_enum_method, is_object_method, jdk_result};
use super::resolver::{FieldFilter, Lookup, Ty};
use super::walk::{FileWalk, line, name_chain, node_text, type_segments};

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

impl<'a> FileWalk<'_, 'a> {
    /// An expression: its usages, and its static type.
    pub(super) fn expr(&mut self, node: Node<'a>) -> Ty {
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
    pub(super) fn constructor_invocation(&mut self, node: Node<'a>) {
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
        self.return_type(&[], &chosen)
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
            return self.return_type(owners, &chosen);
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

    /// The return type the chosen methods agree on, called on a receiver
    /// of the types `owners`: a type variable of an inherited method's type
    /// is the type argument the receiver's type gives it.
    fn return_type(&self, owners: &[String], chosen: &[Method<'a>]) -> Ty {
        let mut types = chosen.iter().map(|method| {
            method.facts.return_type.as_ref().map_or(Ty::Unknown, |ty| {
                self.resolver
                    .declared_via(owners, method.owner, ty, &method.facts.type_params)
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
    pub(super) fn bare_name(&mut self, node: Node<'a>) -> Ty {
        let label = node
            .parent()
            .is_some_and(|parent| parent.kind() == "switch_label");
        self.variable(node_text(node, self.source), line(node), label)
            .unwrap_or(Ty::Unknown)
    }

    /// Whether `name` can be a type here: not `var`, a type variable or a
    /// local class.
    pub(super) fn is_type_name(&self, name: &str) -> bool {
        name != "var"
            && !self.type_vars.iter().any(|var| var == name)
            && !self.region.type_vars.contains(name)
            && !self.region.types.contains(name)
    }
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

#[cfg(test)]
mod tests {
    use crate::usages::CallSites;
    use crate::usages::tests::{
        calls, edges, edges_with, resolve_all, resolve_files, unresolved_calls,
    };

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
    fn an_inherited_member_returns_the_type_argument_its_super_type_gives() {
        let (edges, usages) = resolve_all(&[
            (
                "a/Bearer.java",
                "\
package a;
import lombok.Getter;
class Bearer<T> {
    T payload;
    T getPayload() { return payload; }
}
class Mid<X> extends Bearer<X> {}
@Getter
class Holder<T> { private T item; }
class Data { void id() {} }
class AddMessage extends Mid<Data> {
    void self() { getPayload().id(); }
}
class DataHolder extends Holder<Data> {}
",
            ),
            (
                "a/Consumer.java",
                "\
package a;
class Consumer {
    void consume(AddMessage message, DataHolder holder, Bearer<Data> raw) {
        message.getPayload().id();
        message.payload.id();
        holder.getItem().id();
        raw.getPayload().id();
    }
}
",
            ),
        ]);
        assert_eq!(
            edges_with(&edges, "-call-> a.Data#id()"),
            [
                "a.AddMessage#self() -call-> a.Data#id() :12",
                "a.Consumer#consume(AddMessage,DataHolder,Bearer<Data>) -call-> a.Data#id() :4",
                "a.Consumer#consume(AddMessage,DataHolder,Bearer<Data>) -call-> a.Data#id() :5",
                "a.Consumer#consume(AddMessage,DataHolder,Bearer<Data>) -call-> a.Data#id() :6",
            ]
        );
        assert_eq!(unresolved_calls(&usages), [("?#id".to_string(), 1)]);
    }

    #[test]
    fn a_variable_of_a_bounded_type_variable_is_typed_by_its_bound() {
        let calls = calls(&[(
            "a/Publisher.java",
            "\
package a;
class Msg { String type() { return null; } }
class Publisher {
    <T extends Msg> void send(T message) { message.type(); }
    <U> void raw(U message) { message.type(); }
}
",
        )]);
        assert_eq!(calls, ["a.Publisher#send(T) -call-> a.Msg#type() :4"]);
    }

    #[test]
    fn a_bound_does_not_leak_into_a_variable_of_the_same_name() {
        let calls = calls(&[(
            "a/Publisher.java",
            "\
package a;
class Msg { String type() { return null; } }
class Publisher {
    <T> void run(T value) {
        new Runnable() {
            public void run() {}
            <T extends Msg> void go(T t) {}
        };
        value.type();
    }
}
",
        )]);
        assert!(calls.is_empty(), "{calls:?}");
    }
}
