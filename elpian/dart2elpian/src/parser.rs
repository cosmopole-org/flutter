//! The recursive-descent / precedence-climbing parser: tokens -> AST.

use crate::token::Tok;
use crate::ast::*;

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub(crate) struct Parser {
    toks: Vec<Tok>,
    i: usize,
    pub(crate) class_names: std::collections::HashSet<String>,
    /// Monotonic counter for synthesizing unique temporaries (e.g. the iterator
    /// and index locals a `for-in` loop desugars to).
    for_seq: usize,
}

impl Parser {
    pub(crate) fn new(toks: Vec<Tok>) -> Self {
        // Pre-scan for class names so `ClassName(args)` instantiations resolve
        // even when the class is declared later in the file.
        let mut class_names = std::collections::HashSet::new();
        for w in toks.windows(2) {
            if w[0] == Tok::Kw("class".into()) {
                if let Tok::Ident(n) = &w[1] {
                    class_names.insert(n.clone());
                }
            }
        }
        Parser { toks, i: 0, class_names, for_seq: 0 }
    }

    fn peek(&self) -> &Tok {
        self.toks.get(self.i).unwrap_or(&Tok::Eof)
    }
    fn peek_at(&self, k: usize) -> &Tok {
        self.toks.get(self.i + k).unwrap_or(&Tok::Eof)
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks.get(self.i).cloned().unwrap_or(Tok::Eof);
        self.i += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> Result<(), String> {
        if self.peek() == t {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {t:?}, found {:?}", self.peek()))
        }
    }
    fn is_type_kw(&self) -> bool {
        matches!(self.peek(), Tok::Kw(k) if matches!(k.as_str(), "int"|"double"|"num"|"String"|"bool"|"void"|"dynamic"))
    }

    pub(crate) fn parse_program(&mut self) -> Result<Vec<Item>, String> {
        let mut items = Vec::new();
        while *self.peek() != Tok::Eof {
            items.push(self.parse_item()?);
        }
        Ok(items)
    }

    /// A top-level item is a class, an enum, a function declaration, or a
    /// statement. Class form may be prefixed with `abstract`/`final`/`base`/…
    /// modifiers, which carry no runtime meaning here and are skipped.
    fn parse_item(&mut self) -> Result<Item, String> {
        self.skip_class_modifiers();
        if *self.peek() == Tok::Kw("class".into()) {
            return Ok(Item::Class(self.parse_class()?));
        }
        if matches!(self.peek(), Tok::Ident(s) if s == "enum") {
            return Ok(Item::Enum(self.parse_enum()?));
        }
        if self.looks_like_function() {
            return self.parse_function();
        }
        Ok(Item::Stmt(self.parse_stmt()?))
    }

    /// Skip class-level soft modifiers (`abstract`, `final`, `base`, `interface`,
    /// `sealed`, `mixin`) that may precede `class`. Only consumed when a `class`
    /// keyword follows (possibly after more modifiers), so a plain identifier
    /// named e.g. `final` in another position is untouched.
    fn skip_class_modifiers(&mut self) {
        let is_mod = |t: &Tok| match t {
            Tok::Ident(s) => {
                matches!(s.as_str(), "abstract" | "base" | "interface" | "sealed" | "mixin" | "final")
            }
            _ => false,
        };
        // Look ahead: a run of modifiers ending in `class`.
        let mut k = 0;
        while is_mod(self.peek_at(k)) {
            k += 1;
        }
        if *self.peek_at(k) == Tok::Kw("class".into()) {
            for _ in 0..k {
                self.bump();
            }
        }
    }

    /// `enum Name { a, b, c }` (simple constant list; enhanced-enum bodies are
    /// not supported). `with`/`implements` clauses are skipped.
    fn parse_enum(&mut self) -> Result<EnumDecl, String> {
        self.bump(); // 'enum'
        let name = self.ident()?;
        // Skip any `with`/`implements` clause up to the body.
        while *self.peek() != Tok::LBrace && *self.peek() != Tok::Eof {
            self.bump();
        }
        self.eat(&Tok::LBrace)?;
        let mut variants = Vec::new();
        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof && *self.peek() != Tok::Semi {
            variants.push(self.ident()?);
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        // Tolerate a trailing `;` + members block by skipping to the closing brace.
        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof {
            self.bump();
        }
        self.eat(&Tok::RBrace)?;
        Ok(EnumDecl { name, variants })
    }

    fn parse_class(&mut self) -> Result<ClassDecl, String> {
        self.bump(); // 'class'
        let name = self.ident()?;
        // Skip generic type params `<T>` on the class name.
        self.skip_generic_params();
        let superclass = if *self.peek() == Tok::Kw("extends".into()) {
            self.bump();
            let s = self.ident()?;
            self.skip_generic_params();
            Some(s)
        } else {
            None
        };
        // Skip `with M1, M2` and `implements I1, I2` clauses (erased).
        while matches!(self.peek(), Tok::Ident(s) if s == "with" || s == "implements") {
            self.bump();
            loop {
                let _ = self.ident()?;
                self.skip_generic_params();
                if *self.peek() == Tok::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.eat(&Tok::LBrace)?;
        let mut fields = Vec::new();
        let mut static_fields = Vec::new();
        let mut methods = Vec::new();
        let mut ctor_params = ParamList::default();
        let mut ctor_body = Vec::new();
        let mut has_ctor = false;

        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof {
            // Member modifiers, in any order: static / const / final / late / var.
            let mut is_static = false;
            loop {
                match self.peek() {
                    Tok::Ident(s) if s == "static" => { is_static = true; self.bump(); }
                    Tok::Ident(s) if s == "const" || s == "late" || s == "covariant" => { self.bump(); }
                    Tok::Kw(s) if s == "final" || s == "var" => { self.bump(); }
                    _ => break,
                }
            }

            // A getter: `[Type] get name => …` / `{ … }`.
            let is_getter = matches!(self.peek(), Tok::Ident(s) if s == "get")
                || (self.skip_type_at(0) > 0 && *self.peek_at(self.skip_type_at(0)) == Tok::Ident("get".into()));
            if is_getter {
                // Skip an optional return type, then the `get` keyword.
                if !matches!(self.peek(), Tok::Ident(s) if s == "get") {
                    self.maybe_skip_type();
                }
                self.bump(); // 'get'
                let gname = self.ident()?;
                let body = self.parse_fn_body(false)?;
                methods.push(Method {
                    name: gname,
                    params: ParamList::default(),
                    body,
                    is_async: false,
                    is_getter: true,
                    is_static,
                });
                continue;
            }

            // Optional type annotation before the member name (fields/methods).
            let ret_void = *self.peek() == Tok::Kw("void".into());
            self.maybe_skip_type();
            let member_name = self.ident()?;

            // A named constructor `ClassName.factoryName(...)` — lowered to a
            // static factory method whose body may assign to a fresh `this`.
            let named_ctor = member_name == name && *self.peek() == Tok::Dot;

            if named_ctor {
                self.bump(); // '.'
                let ctor_name = self.ident()?;
                let params = self.parse_param_list()?;
                self.skip_initializers();
                let body = if *self.peek() == Tok::Semi {
                    self.bump();
                    Vec::new()
                } else {
                    self.parse_block()?
                };
                methods.push(Method {
                    name: ctor_name,
                    params,
                    body,
                    is_async: false,
                    is_getter: false,
                    is_static: true,
                });
                continue;
            }

            if *self.peek() == Tok::LParen {
                if member_name == name {
                    // The unnamed constructor.
                    has_ctor = true;
                    ctor_params = self.parse_param_list()?;
                    self.skip_initializers();
                    ctor_body = if *self.peek() == Tok::Semi {
                        self.bump();
                        Vec::new()
                    } else {
                        self.parse_block()?
                    };
                } else {
                    let params = self.parse_param_list()?;
                    let is_async = self.eat_async_modifier();
                    let body = self.parse_fn_body(ret_void)?;
                    methods.push(Method {
                        name: member_name,
                        params,
                        body,
                        is_async,
                        is_getter: false,
                        is_static,
                    });
                }
            } else {
                // Field: optional initializer, then ';'. Additional comma-separated
                // names share the type (`double left, top, right, bottom;`).
                let mut names = vec![member_name];
                let init = if *self.peek() == Tok::Op("=".into()) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                let mut extra: Vec<(String, Option<Expr>)> = Vec::new();
                while *self.peek() == Tok::Comma {
                    self.bump();
                    let n = self.ident()?;
                    let e = if *self.peek() == Tok::Op("=".into()) {
                        self.bump();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    names.push(n.clone());
                    extra.push((n, e));
                }
                self.eat(&Tok::Semi)?;
                let target = if is_static { &mut static_fields } else { &mut fields };
                target.push((names.remove(0), init));
                for (n, e) in extra {
                    target.push((n, e));
                }
            }
        }
        self.eat(&Tok::RBrace)?;
        let calls_super = superclass.is_some();
        Ok(ClassDecl {
            name,
            superclass,
            fields,
            static_fields,
            ctor_params,
            ctor_body,
            has_ctor,
            calls_super,
            methods,
        })
    }

    /// Skip a `<...>` generic-parameter/argument clause at the cursor, if present.
    fn skip_generic_params(&mut self) {
        if matches!(self.peek(), Tok::Op(o) if o == "<") {
            let mut depth = 0i32;
            loop {
                match self.peek() {
                    Tok::Op(o) if o == "<" => depth += 1,
                    Tok::Op(o) if o == ">" => {
                        depth -= 1;
                        self.bump();
                        if depth == 0 {
                            return;
                        }
                        continue;
                    }
                    Tok::Eof => return,
                    _ => {}
                }
                self.bump();
            }
        }
    }

    /// Skip a constructor initializer list: `: a = b, this.c = d, super(...)`.
    /// The `field = value` initializers are erased (fields are assigned in the
    /// body / via initializing formals in this subset); a `super(...)` call is
    /// likewise dropped since the emitter always emits a bare `super()`.
    fn skip_initializers(&mut self) {
        if *self.peek() != Tok::Colon {
            return;
        }
        // Consume everything up to the constructor body `{` or the terminating `;`.
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RParen | Tok::RBracket => depth -= 1,
                Tok::LBrace if depth == 0 => return,
                Tok::Semi if depth == 0 => return,
                Tok::Eof => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// Non-consuming: index just past an optional type (keyword or identifier,
    /// with balanced `<...>` generic arguments) starting at `k`.
    fn skip_type_at(&self, mut k: usize) -> usize {
        let is_type_start = matches!(self.peek_at(k), Tok::Ident(_))
            || matches!(self.peek_at(k), Tok::Kw(kw)
                if matches!(kw.as_str(), "int"|"double"|"num"|"String"|"bool"|"void"|"dynamic"));
        if !is_type_start {
            return k;
        }
        k += 1;
        if matches!(self.peek_at(k), Tok::Op(o) if o == "<") {
            let mut depth = 0;
            loop {
                match self.peek_at(k) {
                    Tok::Op(o) if o == "<" => depth += 1,
                    Tok::Op(o) if o == ">" => {
                        depth -= 1;
                        k += 1;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    Tok::Eof => break,
                    _ => {}
                }
                k += 1;
            }
        }
        k
    }

    /// Consuming: skip a type annotation iff it is immediately followed by an
    /// identifier (the declared name), so a bare `name` is not eaten as a type.
    fn maybe_skip_type(&mut self) {
        let at = self.skip_type_at(0);
        if at > 0 && matches!(self.peek_at(at), Tok::Ident(_)) {
            for _ in 0..at {
                self.bump();
            }
        }
    }

    fn looks_like_function(&self) -> bool {
        // Optional return type (incl. generics), then the name.
        let after_type = self.skip_type_at(0);
        let name_pos = if after_type > 0 && matches!(self.peek_at(after_type), Tok::Ident(_)) {
            after_type
        } else {
            0
        };
        if !matches!(self.peek_at(name_pos), Tok::Ident(_)) {
            return false;
        }
        let mut k = name_pos + 1;
        if *self.peek_at(k) != Tok::LParen {
            return false;
        }
        // scan to matching ')'
        let mut depth = 0;
        loop {
            match self.peek_at(k) {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        k += 1;
                        break;
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
        // A declaration body is a block `{`, an arrow `=>`, or `async` first.
        *self.peek_at(k) == Tok::LBrace
            || matches!(self.peek_at(k), Tok::Op(o) if o == "=>")
            || *self.peek_at(k) == Tok::Kw("async".into())
    }

    fn parse_function(&mut self) -> Result<Item, String> {
        let is_void = *self.peek() == Tok::Kw("void".into());
        self.maybe_skip_type(); // optional return type (incl. generics)
        let name = self.ident()?;
        let params = self.parse_param_list()?;
        let is_async = self.eat_async_modifier();
        let body = self.parse_fn_body(is_void)?;
        Ok(Item::Func(name, params, body, is_async))
    }

    /// Consume an `async` (or `async*`/`sync*`, treated as `async`) body modifier.
    fn eat_async_modifier(&mut self) -> bool {
        if *self.peek() == Tok::Kw("async".into()) {
            self.bump();
            // async* — ignore the `*` (streams not modelled yet).
            if matches!(self.peek(), Tok::Op(o) if o == "*") {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    /// Parse a `( ... )` formal parameter list: required-positional, plus an
    /// optional-positional group `[ ... ]` and/or a named group `{ ... }`.
    fn parse_param_list(&mut self) -> Result<ParamList, String> {
        self.eat(&Tok::LParen)?;
        let mut pl = ParamList::default();
        while *self.peek() != Tok::RParen {
            if *self.peek() == Tok::LBracket {
                self.bump();
                while *self.peek() != Tok::RBracket {
                    pl.optional_pos.push(self.parse_one_param()?);
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    }
                }
                self.eat(&Tok::RBracket)?;
            } else if *self.peek() == Tok::LBrace {
                self.bump();
                while *self.peek() != Tok::RBrace {
                    pl.named.push(self.parse_one_param()?);
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    }
                }
                self.eat(&Tok::RBrace)?;
            } else {
                pl.positional.push(self.parse_one_param()?);
                if *self.peek() == Tok::Comma {
                    self.bump();
                }
            }
        }
        self.eat(&Tok::RParen)?;
        Ok(pl)
    }

    fn parse_one_param(&mut self) -> Result<Param, String> {
        // `required` is a contextual modifier (an identifier in our lexer).
        if matches!(self.peek(), Tok::Ident(s) if s == "required") {
            self.bump();
        }
        let mut is_this = false;
        if *self.peek() == Tok::Kw("this".into()) {
            self.bump();
            self.eat(&Tok::Dot)?;
            is_this = true;
        } else {
            self.maybe_skip_type(); // erase the declared type (incl. generics)
        }
        let name = self.ident()?;
        let default = if *self.peek() == Tok::Op("=".into()) {
            self.bump();
            // Dart const defaults: drop a leading `const`.
            if matches!(self.peek(), Tok::Ident(s) if s == "const") {
                self.bump();
            }
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Param { name, is_this, default })
    }

    /// Parse a call argument list into (positional, named) argument groups.
    fn parse_args(&mut self) -> Result<(Vec<Expr>, Vec<(String, Expr)>), String> {
        self.eat(&Tok::LParen)?;
        let mut pos = Vec::new();
        let mut named = Vec::new();
        while *self.peek() != Tok::RParen {
            if matches!(self.peek(), Tok::Ident(_)) && *self.peek_at(1) == Tok::Colon {
                let n = self.ident()?;
                self.eat(&Tok::Colon)?;
                named.push((n, self.parse_expr()?));
            } else {
                pos.push(self.parse_expr()?);
            }
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RParen)?;
        Ok((pos, named))
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(format!("expected identifier, found {other:?}")),
        }
    }

    /// Parse a type name for `is`/`as`: a primitive keyword or a class ident.
    /// Any generic arguments `<...>` are consumed and ignored (erased).
    fn parse_type_name(&mut self) -> Result<String, String> {
        let name = match self.bump() {
            Tok::Ident(s) => s,
            Tok::Kw(k) => k,
            other => return Err(format!("expected a type name, found {other:?}")),
        };
        // Skip `<...>` generic arguments if present.
        if *self.peek() == Tok::Op("<".into()) {
            let mut depth = 0;
            loop {
                match self.peek() {
                    Tok::Op(o) if o == "<" => depth += 1,
                    Tok::Op(o) if o == ">" => {
                        depth -= 1;
                        self.bump();
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    Tok::Eof => break,
                    _ => {}
                }
                self.bump();
            }
        }
        Ok(name)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.eat(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof {
            stmts.push(self.parse_stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Tok::LBrace => Ok(Stmt::Block(self.parse_block()?)),
            Tok::Kw(k) if k == "var" || k == "final" => {
                self.bump();
                self.maybe_skip_type();
                self.parse_var_tail()
            }
            // `const` / `late` (contextual) declaration modifiers, alone or with a
            // type: `const double x = 8.0;`, `late final Foo y = ...;`.
            Tok::Ident(s) if s == "const" || s == "late" => {
                while matches!(self.peek(), Tok::Ident(x) if x == "const" || x == "late")
                    || matches!(self.peek(), Tok::Kw(x) if x == "final" || x == "var")
                {
                    self.bump();
                }
                self.maybe_skip_type();
                self.parse_var_tail()
            }
            Tok::Kw(k) if matches!(k.as_str(), "int" | "double" | "num" | "String" | "bool") => {
                // typed local declaration: erase the type, then `name [= expr];`
                self.bump();
                self.parse_var_tail()
            }
            Tok::Kw(k) if k == "if" => self.parse_if(),
            Tok::Kw(k) if k == "while" => self.parse_while(),
            Tok::Kw(k) if k == "for" => self.parse_for(),
            Tok::Kw(k) if k == "return" => {
                self.bump();
                if *self.peek() == Tok::Semi {
                    self.bump();
                    Ok(Stmt::Return(None))
                } else {
                    let e = self.parse_expr()?;
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::Return(Some(e)))
                }
            }
            // Class-typed local declaration: `Type name [= expr];`.
            Tok::Ident(_) if matches!(self.peek_at(1), Tok::Ident(_)) => {
                self.bump(); // erase the type
                self.parse_var_tail()
            }
            _ => {
                let e = self.parse_expr()?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Expr(e))
            }
        }
    }

    fn parse_var_tail(&mut self) -> Result<Stmt, String> {
        let name = self.ident()?;
        let init = if *self.peek() == Tok::Op("=".into()) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.eat(&Tok::Semi)?;
        Ok(Stmt::Var(name, init))
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.bump();
        self.eat(&Tok::LParen)?;
        let cond = self.parse_expr()?;
        self.eat(&Tok::RParen)?;
        let then = self.stmt_as_block()?;
        let els = if matches!(self.peek(), Tok::Kw(k) if k == "else") {
            self.bump();
            self.stmt_as_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt::If(cond, then, els))
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.bump();
        self.eat(&Tok::LParen)?;
        let cond = self.parse_expr()?;
        self.eat(&Tok::RParen)?;
        let body = self.stmt_as_block()?;
        Ok(Stmt::While(cond, body))
    }

    /// C-style `for (init; cond; update) body` lowered to `{ init; while (cond) { body; update; } }`.
    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.bump();
        self.eat(&Tok::LParen)?;
        // A `for-in` header has no top-level `;` before the closing `)`.
        if self.header_is_for_in() {
            return self.parse_for_in();
        }
        let init = if *self.peek() == Tok::Semi {
            self.bump();
            None
        } else {
            Some(self.parse_stmt()?) // consumes the ';'
        };
        let cond = if *self.peek() == Tok::Semi {
            Expr::Bool(true)
        } else {
            self.parse_expr()?
        };
        self.eat(&Tok::Semi)?;
        let update = if *self.peek() == Tok::RParen {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.eat(&Tok::RParen)?;
        let mut body = self.stmt_as_block()?;
        if let Some(u) = update {
            body.push(Stmt::Expr(u));
        }
        let while_stmt = Stmt::While(cond, body);
        let mut block = Vec::new();
        if let Some(i) = init {
            block.push(i);
        }
        block.push(while_stmt);
        Ok(Stmt::Block(block))
    }

    /// True if the tokens from the current position (just past `for (`) are a
    /// `for-in` header — i.e. there is no top-level `;` before the matching `)`.
    fn header_is_for_in(&self) -> bool {
        let mut k = 0usize;
        let mut depth = 0i32;
        loop {
            match self.peek_at(k) {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                Tok::Semi if depth == 0 => return false,
                Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    /// Parse `for (<var> in <iterable>) <body>` and desugar it to an indexed
    /// `while` over `<iterable>.length`, re-declaring the loop variable each
    /// iteration (Dart's closure-capture semantics). The `(` is already consumed.
    fn parse_for_in(&mut self) -> Result<Stmt, String> {
        // Optional `var` / `final`.
        if matches!(self.peek(), Tok::Kw(k) if k == "var" || k == "final") {
            self.bump();
        }
        // Optional type annotation: only skip it when a type is followed by the
        // loop-variable identifier which is itself followed by `in` — so a bare
        // `name in` is never mistaken for a `Type name`.
        let in_tok = Tok::Ident("in".into());
        let after_type = self.skip_type_at(0);
        let has_type = after_type > 0
            && matches!(self.peek_at(after_type), Tok::Ident(_))
            && self.peek_at(after_type) != &in_tok
            && self.peek_at(after_type + 1) == &in_tok;
        if has_type {
            for _ in 0..after_type {
                self.bump();
            }
        }
        let name = match self.bump() {
            Tok::Ident(n) => n,
            other => return Err(format!("for-in expected a loop variable, found {other:?}")),
        };
        self.eat(&in_tok)?;
        let iter = self.parse_expr()?;
        self.eat(&Tok::RParen)?;
        let mut body = self.stmt_as_block()?;

        let n = self.for_seq;
        self.for_seq += 1;
        let it = format!("__for_it{n}");
        let idx = format!("__for_i{n}");
        let idx_read = || Expr::Ident(idx.clone());

        // var name = __forIt[__forI];
        let mut loop_body = vec![Stmt::Var(
            name,
            Some(Expr::Index(
                Box::new(Expr::Ident(it.clone())),
                Box::new(idx_read()),
            )),
        )];
        loop_body.append(&mut body);
        // __forI = __forI + 1;
        loop_body.push(Stmt::Expr(Expr::Assign(
            Box::new(idx_read()),
            Box::new(Expr::Binary("+".into(), Box::new(idx_read()), Box::new(Expr::Int(1)))),
        )));

        let cond = Expr::Binary(
            "<".into(),
            Box::new(idx_read()),
            Box::new(Expr::Member(Box::new(Expr::Ident(it.clone())), "length".into())),
        );
        Ok(Stmt::Block(vec![
            Stmt::Var(it, Some(iter)),
            Stmt::Var(idx, Some(Expr::Int(0))),
            Stmt::While(cond, loop_body),
        ]))
    }

    fn stmt_as_block(&mut self) -> Result<Vec<Stmt>, String> {
        if *self.peek() == Tok::LBrace {
            self.parse_block()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    // ---- expressions (precedence climbing) ----

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_assign()
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let lhs = self.parse_ternary()?;
        if *self.peek() == Tok::Op("=".into()) {
            self.bump();
            let rhs = self.parse_assign()?;
            return Ok(Expr::Assign(Box::new(lhs), Box::new(rhs)));
        }
        if let Tok::Op(o) = self.peek() {
            if matches!(o.as_str(), "+=" | "-=" | "*=" | "/=") {
                let op = o.clone();
                self.bump();
                let rhs = self.parse_assign()?;
                return Ok(Expr::AssignOp(op, Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let mut cond = self.parse_binary(0)?;
        // `is` / `as` bind tighter than `?:` but looser than the binary ops.
        loop {
            match self.peek() {
                Tok::Kw(k) if k == "is" => {
                    self.bump();
                    let ty = self.parse_type_name()?;
                    cond = Expr::Is(Box::new(cond), ty);
                }
                Tok::Kw(k) if k == "as" => {
                    self.bump();
                    let ty = self.parse_type_name()?;
                    cond = Expr::As(Box::new(cond), ty);
                }
                _ => break,
            }
        }
        if *self.peek() == Tok::Question {
            self.bump();
            let then = self.parse_assign()?;
            self.eat(&Tok::Colon)?;
            let els = self.parse_assign()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(then), Box::new(els)));
        }
        Ok(cond)
    }

    fn parse_binary(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, bp) = match self.peek() {
                Tok::Op(o) => match binding_power(o) {
                    Some(bp) => (o.clone(), bp),
                    None => break,
                },
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_binary(bp + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// True when a `(` begins a closure param list (`(a) => …` or `(a) { … }`)
    /// rather than a parenthesized expression.
    fn looks_like_lambda(&self) -> bool {
        if *self.peek() != Tok::LParen {
            return false;
        }
        let mut k = 0;
        let mut depth = 0;
        loop {
            match self.peek_at(k) {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        k += 1;
                        break;
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
        matches!(self.peek_at(k), Tok::Op(o) if o == "=>") || *self.peek_at(k) == Tok::LBrace
    }

    fn parse_closure(&mut self) -> Result<Expr, String> {
        let params = self.parse_param_list()?;
        let body = self.parse_fn_body(false)?;
        Ok(Expr::Closure(params, body))
    }

    /// A function/method body: a block, or an arrow body `=> expr;`. `is_void`
    /// is true for a `void`-returning declaration, whose arrow body is a
    /// *statement* (it must not `return` a value — `void f() => g();` runs `g()`
    /// for effect).
    fn parse_fn_body(&mut self, is_void: bool) -> Result<Vec<Stmt>, String> {
        // An abstract / external declaration has no body, just `;`.
        if *self.peek() == Tok::Semi {
            self.bump();
            return Ok(Vec::new());
        }
        if *self.peek() == Tok::Op("=>".into()) {
            self.bump();
            let e = self.parse_expr()?;
            // A closure arrow body has no trailing `;`; a declaration does. Accept
            // an optional semicolon so both forms parse.
            if *self.peek() == Tok::Semi {
                self.bump();
            }
            // A void body, or an assignment/update (which Elpian treats as a
            // statement, not an expression), becomes a statement rather than
            // `return <expr>`.
            let stmt = match e {
                Expr::Assign(..) | Expr::AssignOp(..) | Expr::Update(..) => Stmt::Expr(e),
                _ if is_void => Stmt::Expr(e),
                _ => Stmt::Return(Some(e)),
            };
            Ok(vec![stmt])
        } else {
            self.parse_block()
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        // `const` before an expression (`const Text(...)`, `const [...]`,
        // `const EdgeInsets.all(8)`) is erased — const-ness is not modelled.
        if matches!(self.peek(), Tok::Ident(s) if s == "const") {
            self.bump();
            return self.parse_unary();
        }
        if self.looks_like_lambda() {
            return self.parse_closure();
        }
        if *self.peek() == Tok::Kw("await".into()) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr::Await(Box::new(e)));
        }
        if let Tok::Op(o) = self.peek() {
            if o == "!" || o == "-" {
                let op = o.clone();
                self.bump();
                let e = self.parse_unary()?;
                return Ok(Expr::Unary(op, Box::new(e)));
            }
            if o == "++" || o == "--" {
                let op = o.clone();
                self.bump();
                let e = self.parse_unary()?;
                return Ok(Expr::Update(op, Box::new(e), true));
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek() {
                Tok::LParen => {
                    let (pos, named) = self.parse_args()?;
                    // `ClassName(args)` (no member/index in front) is a Dart
                    // instantiation, not a plain call.
                    e = match e {
                        Expr::Ident(name) if self.class_names.contains(&name) => {
                            Expr::New(name, pos, named)
                        }
                        callee => Expr::Call(Box::new(callee), pos, named),
                    };
                }
                Tok::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    self.eat(&Tok::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
                Tok::Dot => {
                    self.bump();
                    let name = self.ident()?;
                    e = Expr::Member(Box::new(e), name);
                }
                Tok::Op(o) if o == "++" || o == "--" => {
                    let op = o.clone();
                    self.bump();
                    e = Expr::Update(op, Box::new(e), false);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Tok::Int(i) => Ok(Expr::Int(i)),
            Tok::Double(d) => Ok(Expr::Double(d)),
            Tok::Str(p) => Ok(Expr::Str(p)),
            Tok::Bool(b) => Ok(Expr::Bool(b)),
            Tok::Null => Ok(Expr::Null),
            Tok::Kw(k) if k == "this" => Ok(Expr::This),
            Tok::Kw(k) if k == "new" => {
                // Optional `new` keyword: `new ClassName(args)`.
                let name = self.ident()?;
                let (pos, named) = self.parse_args()?;
                Ok(Expr::New(name, pos, named))
            }
            Tok::Ident(s) => Ok(Expr::Ident(s)),
            Tok::LParen => {
                let e = self.parse_expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::LBracket => {
                let mut elems = Vec::new();
                while *self.peek() != Tok::RBracket {
                    elems.push(self.parse_expr()?);
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    }
                }
                self.eat(&Tok::RBracket)?;
                Ok(Expr::List(elems))
            }
            Tok::LBrace => {
                // Map literal `{k: v}` or set literal `{a, b}` (empty `{}` is a Map).
                let mut entries = Vec::new();
                while *self.peek() != Tok::RBrace {
                    let k = self.parse_expr()?;
                    let v = if *self.peek() == Tok::Colon {
                        self.bump();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    entries.push((k, v));
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    }
                }
                self.eat(&Tok::RBrace)?;
                Ok(Expr::MapOrSet(entries))
            }
            other => Err(format!("unexpected token in expression: {other:?}")),
        }
    }
}

fn binding_power(op: &str) -> Option<u8> {
    Some(match op {
        "??" => 0,
        "||" => 1,
        "&&" => 2,
        "==" | "!=" => 3,
        "<" | "<=" | ">" | ">=" => 4,
        "+" | "-" => 5,
        "*" | "/" | "%" | "~/" => 6,
        _ => return None,
    })
}

