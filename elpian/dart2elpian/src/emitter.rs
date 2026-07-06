//! The emitter: lowers the Dart AST to the JS subset the Elpian VM ingests.

use crate::token::StrPart;
use crate::ast::*;
use crate::lexer::Lexer;
use crate::parser::Parser;

// ---------------------------------------------------------------------------
// Emitter (Dart AST -> Elpian JS subset)
// ---------------------------------------------------------------------------

/// Runtime prelude prepended to every program. The `__List_*` functions
/// implement the higher-order `Iterable` methods in the language itself (the
/// VM's indexer binds them to the receiver as `this` when `list.map`/`.where`/…
/// is read). They rely on `this.length`, `this[i]`, `out.add(...)`, and closure
/// calls — all VM-supported.
///
/// The Dart-specific operators `~/` (truncating integer division) and `??`
/// (null-coalescing) are **not** in this prelude: they lower to native VM
/// opcodes emitted directly by [`Emitter::emit_expr`], so no helper functions
/// are needed.
const PRELUDE: &str = concat!(
    "function __List_map(f){ var out = []; var i = 0; while (i < this.length) { out.push(f(this[i])); i = i + 1; } return out; }\n",
    "function __List_where(f){ var out = []; var i = 0; while (i < this.length) { if (f(this[i])) { out.push(this[i]); } i = i + 1; } return out; }\n",
    "function __List_forEach(f){ var i = 0; while (i < this.length) { f(this[i]); i = i + 1; } return null; }\n",
    "function __List_fold(init, f){ var acc = init; var i = 0; while (i < this.length) { acc = f(acc, this[i]); i = i + 1; } return acc; }\n",
    "function __List_any(f){ var i = 0; while (i < this.length) { if (f(this[i])) { return true; } i = i + 1; } return false; }\n",
    "function __List_every(f){ var i = 0; while (i < this.length) { if (!f(this[i])) { return false; } i = i + 1; } return true; }\n",
    "function __List_reduce(f){ var acc = this[0]; var i = 1; while (i < this.length) { acc = f(acc, this[i]); i = i + 1; } return acc; }\n",
    // ---- async/await runtime: Future + microtask-driven continuations -------
    // These helpers are raw Elpian-JS (they bypass the Dart emitter), so they are
    // written directly against the VM's universal names (`push`, not `add`).
    "var __cbReg = [];\n",
    "function __later(fn){ var id = __cbReg.length; __cbReg.push(fn); askHost(\"dart:async/scheduleMicrotask\", [id]); }\n",
    "function __dartDispatch(a){ var fn = __cbReg[a[0]]; fn(); }\n",
    "function __schedThen(value, cb, next){ __later(function(){ var r = cb(value); if (r != null && r.__isFuture) { r.then(function(rv){ next.complete(rv); }); } else { next.complete(r); } }); }\n",
    "class _Future { constructor(){ this.__isFuture = true; this.done = false; this.value = null; this.cbs = []; } then(cb){ var next = new _Future(); if (this.done) { __schedThen(this.value, cb, next); } else { var p = {}; p.cb = cb; p.next = next; this.cbs.push(p); } return next; } complete(v){ if (this.done) { return; } this.done = true; this.value = v; var i = 0; while (i < this.cbs.length) { var p = this.cbs[i]; __schedThen(v, p.cb, p.next); i = i + 1; } } }\n",
    "function __Future_value(v){ var f = new _Future(); __later(function(){ f.complete(v); }); return f; }\n",
    "function __await(x){ if (x != null && x.__isFuture) { return x; } return __Future_value(x); }\n",
);


/// Scope-aware emitter. Inside a class body it resolves bare field references to
/// `this.field` and bare method calls to `this.method(...)`, so idiomatic Dart
/// (which omits `this.`) lowers to valid JS.
type NameSet = std::collections::HashSet<String>;
type NameMap = std::collections::HashMap<String, NameSet>;

pub(crate) struct Emitter {
    pub(crate) out: String,
    class_names: NameSet,
    /// Transitive (own + inherited) field names per class.
    field_map: NameMap,
    /// Transitive method names per class.
    method_map: NameMap,
    fields: NameSet,
    methods: NameSet,
    /// Instance getter names (across all classes) that should be *called* when
    /// read as a bare member `obj.name` → `obj.name()`. Excludes names that are
    /// also fields somewhere, or native properties, to avoid mis-calling those.
    getters: NameSet,
    /// Every member name declared by a user class (fields, `this.` params,
    /// methods, getters). A Dart core-member spelling that collides with one of
    /// these is left untranslated, so a user's own `add`/`remove`/… method still
    /// resolves against its object rather than being rewritten to a builtin.
    user_members: NameSet,
    locals: Vec<NameSet>,
    in_class: bool,
}

/// Compile-time resolution of a Dart core-type member spelling to the VM's
/// single **universal** stdlib name. This is the whole point of doing name
/// resolution in the compiler: the VM never sees `toUpperCase`/`add`/`containsKey`
/// — it only ever sees `upper`/`push`/`has`. Names that Dart already spells the
/// universal way (`contains`, `indexOf`, `insert`, `floor`, `keys`, `map`, …) are
/// not listed here because no translation is needed. Returns the universal name,
/// or `None` when the spelling needs no change.
fn universal_member(name: &str) -> Option<&'static str> {
    Some(match name {
        // List
        "add" => "push",
        "addAll" => "pushAll",
        "removeLast" => "pop",
        "sublist" => "slice",
        // String
        "toUpperCase" => "upper",
        "toLowerCase" => "lower",
        "replaceAll" => "replace",
        "padLeft" => "padStart",
        "padRight" => "padEnd",
        "trimLeft" => "trimStart",
        "trimRight" => "trimEnd",
        // num
        "toInt" => "int",
        // Map
        "containsKey" => "has",
        _ => return None,
    })
}

/// Native member names the VM binds as properties (not zero-arg getters), which
/// must never be rewritten to a call even if a user class declares a like-named
/// getter.
const NATIVE_PROPS: &[&str] = &[
    "length", "isEmpty", "isNotEmpty", "first", "last", "single", "keys", "values",
    "reversed", "iterator", "runtimeType", "hashCode",
];

impl Emitter {
    pub(crate) fn new(class_names: NameSet) -> Self {
        Emitter {
            out: String::from(PRELUDE),
            class_names,
            field_map: Default::default(),
            method_map: Default::default(),
            fields: Default::default(),
            methods: Default::default(),
            getters: Default::default(),
            user_members: Default::default(),
            locals: Vec::new(),
            in_class: false,
        }
    }

    /// Build transitive field/method sets so inherited members inside a subclass
    /// still resolve to `this.member`.
    fn build_member_maps(&mut self, items: &[Item]) {
        let mut own_fields: NameMap = Default::default();
        let mut own_methods: NameMap = Default::default();
        let mut supers: std::collections::HashMap<String, Option<String>> = Default::default();
        let mut all_getters = NameSet::new();
        let mut all_fields = NameSet::new();
        for item in items {
            if let Item::Class(c) = item {
                own_fields.insert(c.name.clone(), c.fields.iter().map(|(n, _)| n.clone()).collect());
                // Only *instance*, non-getter methods participate in bare-name
                // `this.method` resolution; statics are reached as `Class.m`.
                own_methods.insert(
                    c.name.clone(),
                    c.methods
                        .iter()
                        .filter(|m| !m.is_static && !m.is_getter)
                        .map(|m| m.name.clone())
                        .collect(),
                );
                // `this.x` params are also fields.
                if let Some(set) = own_fields.get_mut(&c.name) {
                    for p in c.ctor_params.all_this_params() {
                        set.insert(p.name.clone());
                    }
                }
                for (n, _) in &c.fields {
                    all_fields.insert(n.clone());
                    self.user_members.insert(n.clone());
                }
                for p in c.ctor_params.all_this_params() {
                    all_fields.insert(p.name.clone());
                    self.user_members.insert(p.name.clone());
                }
                for m in &c.methods {
                    self.user_members.insert(m.name.clone());
                    if m.is_getter && !m.is_static {
                        all_getters.insert(m.name.clone());
                    }
                }
                supers.insert(c.name.clone(), c.superclass.clone());
            }
        }
        // A getter is call-rewritten only if it is unambiguous: never also a
        // field, never a native property.
        for g in all_getters {
            if !all_fields.contains(&g) && !NATIVE_PROPS.contains(&g.as_str()) {
                self.getters.insert(g);
            }
        }
        // Walk the superclass chain for each class.
        for name in own_fields.keys().cloned().collect::<Vec<_>>() {
            let mut fields = NameSet::new();
            let mut methods = NameSet::new();
            let mut cur = Some(name.clone());
            let mut guard = 0;
            while let Some(c) = cur {
                if guard > 64 {
                    break;
                }
                guard += 1;
                if let Some(f) = own_fields.get(&c) {
                    fields.extend(f.iter().cloned());
                }
                if let Some(m) = own_methods.get(&c) {
                    methods.extend(m.iter().cloned());
                }
                cur = supers.get(&c).cloned().flatten();
            }
            self.field_map.insert(name.clone(), fields);
            self.method_map.insert(name, methods);
        }
    }

    fn push_scope(&mut self) {
        self.locals.push(Default::default());
    }
    fn pop_scope(&mut self) {
        self.locals.pop();
    }
    fn declare(&mut self, name: &str) {
        if let Some(top) = self.locals.last_mut() {
            top.insert(name.to_string());
        }
    }
    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|s| s.contains(name))
    }

    /// The JS signature names for a param list: required + optional positional,
    /// plus one trailing options object when there are named params.
    fn param_sig(&self, pl: &ParamList) -> Vec<String> {
        let mut names: Vec<String> = pl
            .positional
            .iter()
            .chain(pl.optional_pos.iter())
            .map(|p| p.name.clone())
            .collect();
        if !pl.named.is_empty() {
            names.push(NAMED_ARG.to_string());
        }
        names
    }

    /// Declare all parameter names (and the options object) as locals so bare
    /// references inside the body don't resolve to `this.field`.
    fn declare_params(&mut self, pl: &ParamList) {
        for p in pl.positional.iter().chain(pl.optional_pos.iter()).chain(pl.named.iter()) {
            let n = p.name.clone();
            self.declare(&n);
        }
        if !pl.named.is_empty() {
            self.declare(NAMED_ARG);
        }
    }

    /// Emit the prologue that fills optional-positional defaults and destructures
    /// named params out of the options object (with defaults).
    fn emit_param_prologue(&mut self, pl: &ParamList, depth: usize) {
        for p in &pl.optional_pos {
            if let Some(d) = &p.default {
                let dv = self.emit_expr(d);
                self.indent(depth);
                self.out.push_str(&format!("if ({} == null) {{ {} = {}; }}\n", p.name, p.name, dv));
            }
        }
        for p in &pl.named {
            self.indent(depth);
            self.out.push_str(&format!(
                "var {n} = (({na} != null) ? {na}.{n} : null);\n",
                n = p.name,
                na = NAMED_ARG
            ));
            if let Some(d) = &p.default {
                let dv = self.emit_expr(d);
                self.indent(depth);
                self.out.push_str(&format!("if ({} == null) {{ {} = {}; }}\n", p.name, p.name, dv));
            }
        }
    }

    /// Emit a named-argument options object literal: `{ "k": v, ... }`.
    fn emit_named_object(&mut self, named: &[(String, Expr)]) -> String {
        let pairs: Vec<String> = named
            .iter()
            .map(|(k, e)| format!("{}: {}", json_string(k), self.emit_expr(e)))
            .collect();
        format!("{{{}}}", pairs.join(", "))
    }

    fn indent(&mut self, n: usize) {
        for _ in 0..n {
            self.out.push_str("  ");
        }
    }

    pub(crate) fn emit_program(&mut self, items: &[Item]) {
        self.build_member_maps(items);
        let mut has_main = false;
        for item in items {
            match item {
                Item::Func(name, params, body, is_async) => {
                    if name == "main" {
                        has_main = true;
                    }
                    let sig = self.param_sig(params);
                    self.out.push_str(&format!("function {}({}) {{\n", name, sig.join(", ")));
                    self.push_scope();
                    self.declare_params(params);
                    self.emit_param_prologue(params, 1);
                    if *is_async {
                        self.emit_async_seq(body, 1);
                    } else {
                        self.emit_stmts(body, 1);
                    }
                    self.pop_scope();
                    self.out.push_str("}\n");
                }
                Item::Class(c) => self.emit_class(c),
                Item::Enum(e) => self.emit_enum(e),
                Item::Stmt(s) => self.emit_stmt(s, 0),
            }
        }
        if has_main {
            self.out.push_str("main();\n");
        }
    }

    fn emit_class(&mut self, c: &ClassDecl) {
        self.in_class = true;
        // Use transitive sets so inherited members resolve to `this.member`.
        self.fields = self.field_map.get(&c.name).cloned().unwrap_or_default();
        self.methods = self.method_map.get(&c.name).cloned().unwrap_or_default();

        let ext = match &c.superclass {
            Some(s) => format!(" extends {s}"),
            None => String::new(),
        };
        self.out.push_str(&format!("class {}{} {{\n", c.name, ext));

        // Always emit a constructor so every instance is tagged with its class
        // name (used by the reified `is`/`as` checks host-side).
        {
            let sig = self.param_sig(&c.ctor_params);
            self.out.push_str(&format!("  constructor({}) {{\n", sig.join(", ")));
            self.push_scope();
            self.declare_params(&c.ctor_params);
            if c.calls_super {
                self.out.push_str("    super();\n");
            }
            // Reified-type tag: most-derived ctor wins (runs last).
            self.out.push_str(&format!("    this.__class = {};\n", json_string(&c.name)));
            // Destructure named / fill optional-positional params first, so the
            // `this.x` assignments below can read their locals.
            self.emit_param_prologue(&c.ctor_params, 2);
            // Field initializers, then initializing formals (`this.x`) win —
            // matching Dart's initialization order.
            for (fname, init) in &c.fields {
                if let Some(e) = init {
                    let v = self.emit_expr(e);
                    self.out.push_str(&format!("    this.{fname} = {v};\n"));
                }
            }
            let this_params: Vec<String> =
                c.ctor_params.all_this_params().map(|p| p.name.clone()).collect();
            for name in this_params {
                self.out.push_str(&format!("    this.{name} = {name};\n"));
            }
            self.emit_stmts(&c.ctor_body, 2);
            self.pop_scope();
            self.out.push_str("  }\n");
        }

        // Static fields belong to the class, reached as `Class.field`.
        for (fname, init) in &c.static_fields {
            let v = init.as_ref().map(|e| self.emit_expr(e)).unwrap_or_else(|| "null".into());
            self.out.push_str(&format!("  static {fname} = {v};\n"));
        }

        for m in &c.methods {
            let sig = self.param_sig(&m.params);
            let prefix = if m.is_static { "static " } else { "" };
            self.out.push_str(&format!("  {}{}({}) {{\n", prefix, m.name, sig.join(", ")));
            self.push_scope();
            self.declare_params(&m.params);
            // Inside a static member `this`/instance-field resolution is invalid;
            // suppress it so bare names stay bare (they refer to locals / statics).
            let saved_in_class = self.in_class;
            let saved_fields = if m.is_static { std::mem::take(&mut self.fields) } else { NameSet::new() };
            let saved_methods = if m.is_static { std::mem::take(&mut self.methods) } else { NameSet::new() };
            if m.is_static {
                self.in_class = false;
            }
            self.emit_param_prologue(&m.params, 2);
            if m.is_async {
                self.emit_async_seq(&m.body, 2);
            } else {
                self.emit_stmts(&m.body, 2);
            }
            if m.is_static {
                self.in_class = saved_in_class;
                self.fields = saved_fields;
                self.methods = saved_methods;
            }
            self.pop_scope();
            self.out.push_str("  }\n");
        }

        self.out.push_str("}\n");
        self.in_class = false;
        self.fields.clear();
        self.methods.clear();
    }

    /// Emit an enum as a top-level object mapping each constant to its name
    /// string, so `Name.a` is a stable, comparable value.
    fn emit_enum(&mut self, e: &EnumDecl) {
        let pairs: Vec<String> = e
            .variants
            .iter()
            .map(|v| format!("{}: {}", v, json_string(v)))
            .collect();
        self.out.push_str(&format!("var {} = {{{}}};\n", e.name, pairs.join(", ")));
    }

    fn emit_stmts(&mut self, stmts: &[Stmt], depth: usize) {
        for s in stmts {
            self.emit_stmt(s, depth);
        }
    }

    /// Lower the body of an `async` function to CPS: each top-level `await`
    /// splits the remaining statements into a `.then` continuation, and the
    /// function returns a `Future` (via `__Future_value` / the awaited future).
    /// Bounded: only awaits at statement top level (var init, expression
    /// statement, or `return await`) are transformed; awaits nested inside loops,
    /// conditionals, or sub-expressions are not (documented limitation).
    fn emit_async_seq(&mut self, stmts: &[Stmt], depth: usize) {
        let mut i = 0;
        while i < stmts.len() {
            match &stmts[i] {
                Stmt::Var(name, Some(Expr::Await(e))) => {
                    let ev = self.emit_expr(e);
                    self.indent(depth);
                    self.out.push_str(&format!("return __await({ev}).then(function({name}) {{\n"));
                    self.push_scope();
                    self.declare(name);
                    self.emit_async_seq(&stmts[i + 1..], depth + 1);
                    self.pop_scope();
                    self.indent(depth);
                    self.out.push_str("});\n");
                    return;
                }
                Stmt::Expr(Expr::Await(e)) => {
                    let ev = self.emit_expr(e);
                    self.indent(depth);
                    self.out.push_str(&format!("return __await({ev}).then(function(__u) {{\n"));
                    self.push_scope();
                    self.emit_async_seq(&stmts[i + 1..], depth + 1);
                    self.pop_scope();
                    self.indent(depth);
                    self.out.push_str("});\n");
                    return;
                }
                Stmt::Return(Some(Expr::Await(e))) => {
                    let ev = self.emit_expr(e);
                    self.indent(depth);
                    self.out.push_str(&format!("return __await({ev});\n"));
                    return;
                }
                Stmt::Return(Some(e)) => {
                    let ev = self.emit_expr(e);
                    self.indent(depth);
                    self.out.push_str(&format!("return __Future_value({ev});\n"));
                    return;
                }
                Stmt::Return(None) => {
                    self.indent(depth);
                    self.out.push_str("return __Future_value(null);\n");
                    return;
                }
                s => self.emit_stmt(s, depth),
            }
            i += 1;
        }
        // No explicit return: an async function still yields a completed Future.
        self.indent(depth);
        self.out.push_str("return __Future_value(null);\n");
    }

    fn emit_stmt(&mut self, s: &Stmt, depth: usize) {
        self.indent(depth);
        match s {
            Stmt::Var(name, init) => {
                match init {
                    Some(e) => {
                        let v = self.emit_expr(e);
                        self.out.push_str(&format!("var {name} = {v};\n"));
                    }
                    None => self.out.push_str(&format!("var {name};\n")),
                }
                self.declare(name);
            }
            Stmt::Expr(e) => {
                let v = self.emit_expr(e);
                self.out.push_str(&format!("{v};\n"));
            }
            Stmt::Return(e) => match e {
                Some(e) => {
                    let v = self.emit_expr(e);
                    self.out.push_str(&format!("return {v};\n"));
                }
                None => self.out.push_str("return;\n"),
            },
            Stmt::If(c, t, e) => {
                let cond = self.emit_expr(c);
                self.out.push_str(&format!("if ({cond}) {{\n"));
                self.push_scope();
                self.emit_stmts(t, depth + 1);
                self.pop_scope();
                self.indent(depth);
                self.out.push('}');
                if !e.is_empty() {
                    self.out.push_str(" else {\n");
                    self.push_scope();
                    self.emit_stmts(e, depth + 1);
                    self.pop_scope();
                    self.indent(depth);
                    self.out.push('}');
                }
                self.out.push('\n');
            }
            Stmt::While(c, b) => {
                let cond = self.emit_expr(c);
                self.out.push_str(&format!("while ({cond}) {{\n"));
                self.push_scope();
                self.emit_stmts(b, depth + 1);
                self.pop_scope();
                self.indent(depth);
                self.out.push_str("}\n");
            }
            Stmt::Block(b) => {
                self.out.push_str("{\n");
                self.push_scope();
                self.emit_stmts(b, depth + 1);
                self.pop_scope();
                self.indent(depth);
                self.out.push_str("}\n");
            }
        }
    }

    fn resolve_ident(&self, name: &str) -> String {
        if self.is_local(name) {
            name.to_string()
        } else if self.in_class && self.fields.contains(name) {
            format!("this.{name}")
        } else {
            name.to_string()
        }
    }

    /// Map a Dart member spelling to the VM's universal stdlib name at compile
    /// time. A name a user class declares (field/method/getter) addresses that
    /// object and is never rewritten, so a user's own `add`/`remove`/… still
    /// resolves against its instance rather than a core-type builtin.
    fn resolve_member<'a>(&self, name: &'a str) -> &'a str {
        if self.user_members.contains(name) {
            name
        } else {
            universal_member(name).unwrap_or(name)
        }
    }

    fn emit_expr(&mut self, e: &Expr) -> String {
        match e {
            Expr::Int(i) => i.to_string(),
            Expr::Double(d) => {
                if d.fract() == 0.0 {
                    format!("{d:.1}")
                } else {
                    d.to_string()
                }
            }
            Expr::Bool(b) => b.to_string(),
            Expr::Null => "null".into(),
            Expr::This => "this".into(),
            Expr::Ident(s) => self.resolve_ident(s),
            Expr::Str(parts) => self.emit_string(parts),
            Expr::List(xs) => {
                let inner: Vec<String> = xs.iter().map(|x| self.emit_expr(x)).collect();
                format!("[{}]", inner.join(", "))
            }
            Expr::Unary(op, x) => format!("({}{})", op, self.emit_expr(x)),
            Expr::Update(op, x, prefix) => {
                // No wrapping parens: Elpian's JS parser accepts `i++` as a
                // statement but rejects `(i++)`.
                let v = self.emit_expr(x);
                if *prefix {
                    format!("{op}{v}")
                } else {
                    format!("{v}{op}")
                }
            }
            Expr::Binary(op, a, b) => {
                // `~/` and `??` are native VM operators (truncating division and
                // null-coalescing); every operator, these included, emits as the
                // shared parenthesised infix form the front-end shares with JS.
                format!("({} {} {})", self.emit_expr(a), op, self.emit_expr(b))
            }
            Expr::Ternary(c, t, e) => {
                format!("({} ? {} : {})", self.emit_expr(c), self.emit_expr(t), self.emit_expr(e))
            }
            Expr::Assign(a, b) => format!("{} = {}", self.emit_expr(a), self.emit_expr(b)),
            Expr::AssignOp(op, a, b) => {
                format!("{} {} {}", self.emit_expr(a), op, self.emit_expr(b))
            }
            Expr::Index(a, i) => format!("{}[{}]", self.emit_expr(a), self.emit_expr(i)),
            Expr::Member(obj, name) => {
                let o = self.emit_expr(obj);
                let resolved = self.resolve_member(name);
                // A numeric-literal receiver needs parens: `7.clamp` would lex as
                // the float `7.` followed by `clamp`.
                let base = if matches!(&**obj, Expr::Int(_) | Expr::Double(_)) {
                    format!("({o}).{resolved}")
                } else {
                    format!("{o}.{resolved}")
                };
                // A bare read of a getter invokes it (`obj.x` -> `obj.x()`).
                if self.getters.contains(name) {
                    format!("{base}()")
                } else {
                    base
                }
            }
            Expr::New(name, pos, named) => {
                let mut a: Vec<String> = pos.iter().map(|x| self.emit_expr(x)).collect();
                if !named.is_empty() {
                    a.push(self.emit_named_object(named));
                }
                format!("new {}({})", name, a.join(", "))
            }
            Expr::Closure(params, body) => {
                // Emit a JS function expression; params are locals in the body so
                // bare field refs still resolve correctly around the closure.
                let sig = self.param_sig(params);
                let saved = std::mem::take(&mut self.out);
                self.push_scope();
                self.declare_params(params);
                self.emit_param_prologue(params, 1);
                self.emit_stmts(body, 1);
                let body_str = std::mem::replace(&mut self.out, saved);
                self.pop_scope();
                format!("function({}) {{\n{}}}", sig.join(", "), body_str)
            }
            // A stray/nested await (outside the CPS statement positions) can't
            // suspend; surface the awaited future's wrapper so it at least
            // type-checks. Top-level awaits are handled by emit_async_seq.
            Expr::Await(e) => format!("__await({})", self.emit_expr(e)),
            Expr::MapOrSet(entries) => {
                let is_set = !entries.is_empty() && entries.iter().all(|(_, v)| v.is_none());
                if is_set {
                    // Set literal -> list (iteration works; set uniqueness is not
                    // modelled).
                    let items: Vec<String> = entries.iter().map(|(k, _)| self.emit_expr(k)).collect();
                    format!("[{}]", items.join(", "))
                } else {
                    // Map literal -> object literal with the given keys.
                    let pairs: Vec<String> = entries
                        .iter()
                        .map(|(k, v)| {
                            let val = v.as_ref().map(|e| self.emit_expr(e)).unwrap_or_else(|| "null".into());
                            format!("{}: {}", self.emit_expr(k), val)
                        })
                        .collect();
                    format!("{{{}}}", pairs.join(", "))
                }
            }
            // Reified `is` / `as` are native VM operations, reached through the
            // `__isType` / `__asType` compiler intrinsics (which js2elpian lowers
            // to the type-test opcode) rather than a host round-trip. The type is
            // erased to its base name — the reified check is by base type / class.
            Expr::Is(x, ty) => {
                format!("__isType({}, {})", self.emit_expr(x), json_string(ty))
            }
            Expr::As(x, ty) => {
                format!("__asType({}, {})", self.emit_expr(x), json_string(ty))
            }
            Expr::Call(callee, pos, named) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "print" && pos.len() == 1 && named.is_empty() {
                        let a0 = self.emit_expr(&pos[0]);
                        return format!("askHost(\"log\", [{a0}])");
                    }
                    // Bare call to an own method inside a class -> this.method().
                    if self.in_class && !self.is_local(name) && self.methods.contains(name) {
                        let mut a: Vec<String> = pos.iter().map(|x| self.emit_expr(x)).collect();
                        if !named.is_empty() {
                            a.push(self.emit_named_object(named));
                        }
                        return format!("this.{}({})", name, a.join(", "));
                    }
                }
                let mut a: Vec<String> = pos.iter().map(|x| self.emit_expr(x)).collect();
                if !named.is_empty() {
                    a.push(self.emit_named_object(named));
                }
                // A method call on a member is emitted directly, so getter
                // call-rewriting (which fires for a *read* `obj.x`) does not turn
                // `obj.m(args)` into `obj.m()(args)`.
                if let Expr::Member(obj, name) = &**callee {
                    let o = self.emit_expr(obj);
                    let recv = if matches!(&**obj, Expr::Int(_) | Expr::Double(_)) {
                        format!("({o})")
                    } else {
                        o
                    };
                    return format!("{}.{}({})", recv, self.resolve_member(name), a.join(", "));
                }
                let c = self.emit_expr(callee);
                format!("{}({})", c, a.join(", "))
            }
        }
    }

    fn emit_string(&mut self, parts: &[StrPart]) -> String {
        if parts.len() == 1 {
            if let StrPart::Lit(s) = &parts[0] {
                return json_string(s);
            }
        }
        let mut pieces = vec!["\"\"".to_string()];
        for p in parts {
            match p {
                StrPart::Lit(s) => pieces.push(json_string(s)),
                StrPart::Expr(raw) => {
                    let sub = self.emit_interp(raw);
                    pieces.push(format!("({sub})"));
                }
            }
        }
        format!("({})", pieces.join(" + "))
    }

    /// Parse and emit an interpolation chunk in the current scope, so field/
    /// local resolution applies inside `${...}`.
    fn emit_interp(&mut self, src: &str) -> String {
        let toks = match Lexer::new(src).tokenize() {
            Ok(t) => t,
            Err(_) => return "null".into(),
        };
        let mut p = Parser::new(toks);
        p.class_names = self.class_names.clone();
        match p.parse_expr() {
            Ok(e) => self.emit_expr(&e),
            Err(_) => "null".into(),
        }
    }
}

fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

