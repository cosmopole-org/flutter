//! A bounded **Dart → Elpian** front-end.
//!
//! Phase 3 of running Flutter logic on Elpian is a compiler from Dart source to
//! something the VM already executes. Elpian ingests a JS subset directly
//! (`create_vm_from_js`), so this front-end lowers a **Dart subset** to that JS
//! subset — parsing the Dart-specific surface (typed declarations, `~/`, string
//! interpolation, C-style `for`) and erasing/rewriting it into the VM's input.
//! Runtime intrinsics are still reached through `askHost`, exactly as the
//! hand-written JS path does, so the front-end is purely a *language* layer.
//!
//! Supported subset (grows over the roadmap):
//! * top-level function declarations and statements; typed or `var`/`final`
//!   locals (types are parsed and erased);
//! * **classes**: fields (with initializers), constructors incl. `this.x`
//!   initializing formals, methods, `extends`/`super`, instantiation
//!   (`ClassName(args)`), member access, and `this`. Bare field/method
//!   references inside methods resolve to `this.member` (including inherited
//!   members), so idiomatic Dart lowers to valid JS classes;
//! * `if`/`else`, `while`, C-style `for` and **`for-in`** (both lowered to
//!   `while`), `return`, blocks;
//! * expressions: literals (incl. **hex integers** `0xFF2196F3` for colours),
//!   identifiers, calls, list literals, indexing,
//!   assignment + compound assignment (`+= -= *= /=`), `++`/`--`, ternary
//!   `?:`, `|| && == != < <= > >= + - * / % ~/`, unary `! -`;
//! * string interpolation (`"$x"`, `"${expr}"`) lowered to concatenation;
//! * `print(x)` lowered to `askHost("log",[x])`; `~/` lowered to a trunc-div
//!   helper. `main()` is auto-invoked if present.
//!
//! * **closures / function expressions**: `(a) => expr`, `(a) { body }`, and
//!   arrow bodies for function/method declarations (`int f() => expr;`). These
//!   plus the VM's higher-order Iterable methods (`map`/`where`/`fold`/`reduce`/
//!   `any`/`every`, bound in the VM to prelude functions) run real functional
//!   Dart. Closures capture **by reference** for mutated captured locals: a
//!   source transform boxes such a local into a one-element list (a VM reference
//!   type), so `forEach((e) => acc += e)` and closure counters propagate
//!   correctly.
//!
//! * **named & optional parameters** (`{this.width}`, `[int x = 0]`, `required`)
//!   with defaults, lowered to a trailing options object; named arguments at
//!   call sites; and **generic type args** in type positions (erased).
//! * **idiomatic-Flutter surface**: metadata annotations (`@override`,
//!   `@immutable`, …, dropped), `abstract`/soft class modifiers (erased),
//!   `const` constructors/expressions (erased to plain instantiation), `enum`s
//!   (lowered to an object mapping each constant to its name string), `static`
//!   fields/methods and named constructors (reached as `Class.member`, backed by
//!   the VM's static-member support), **getters** (`T get x => …`, emitted as a
//!   method and called when read as `obj.x`), and the `??` null-coalescing
//!   operator (lowered to a helper). A `void` arrow body (`void f() => g();`) is
//!   a statement, not a `return`.
//! * **`async`/`await`**: `async` functions are CPS-transformed to return a
//!   `Future` built from `.then` continuations driven by the microtask loop;
//!   `await` sequences them. Bounded: awaits are transformed only at statement
//!   top level (var init, expression statement, `return await`) — awaits nested
//!   inside loops, conditionals, or sub-expressions need full state-machine
//!   lowering and are not yet handled.
//!
//! NOT yet covered (later phases): mixins, pattern matching, initializer lists
//! with super-args, async closures / awaits inside control flow, generic
//! *typed-local* declarations, and by-reference closure capture.

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    Double(f64),
    Str(Vec<StrPart>),
    Bool(bool),
    Null,
    // punctuation / operators
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Dot,
    Question,
    Colon,
    Op(String),
    Kw(String),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
enum StrPart {
    Lit(String),
    /// Raw source of an interpolation expression, re-parsed by the parser.
    Expr(String),
}

const KEYWORDS: &[&str] = &[
    "var", "final", "if", "else", "while", "for", "return", "void", "int", "double", "num",
    "String", "bool", "dynamic", "class", "extends", "this", "new", "super", "is", "as", "async",
    "await",
];

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer { src: src.as_bytes(), pos: 0 }
    }

    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }
    fn peek2(&self) -> u8 {
        *self.src.get(self.pos + 1).unwrap_or(&0)
    }
    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn skip_trivia(&mut self) {
        loop {
            let c = self.peek();
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else if c == b'/' && self.peek2() == b'/' {
                while self.peek() != b'\n' && self.peek() != 0 {
                    self.pos += 1;
                }
            } else if c == b'/' && self.peek2() == b'*' {
                self.pos += 2;
                while !(self.peek() == b'*' && self.peek2() == b'/') && self.peek() != 0 {
                    self.pos += 1;
                }
                self.pos += 2;
            } else {
                break;
            }
        }
    }

    fn tokenize(&mut self) -> Result<Vec<Tok>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let c = self.peek();
            if c == 0 {
                out.push(Tok::Eof);
                return Ok(out);
            }
            if c == b'@' {
                // Metadata annotation (`@override`, `@immutable`, `@Foo(bar)`):
                // consume it entirely at lex time so the parser never sees it.
                self.skip_annotation();
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                out.push(self.lex_ident());
            } else if c.is_ascii_digit() {
                out.push(self.lex_number()?);
            } else if c == b'"' || c == b'\'' {
                out.push(self.lex_string(c)?);
            } else {
                out.push(self.lex_op()?);
            }
        }
    }

    fn lex_ident(&mut self) -> Tok {
        let start = self.pos;
        while {
            let c = self.peek();
            c.is_ascii_alphanumeric() || c == b'_'
        } {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap().to_string();
        match s.as_str() {
            "true" => Tok::Bool(true),
            "false" => Tok::Bool(false),
            "null" => Tok::Null,
            _ if KEYWORDS.contains(&s.as_str()) => Tok::Kw(s),
            _ => Tok::Ident(s),
        }
    }

    /// Consume a metadata annotation: `@`, a dotted identifier
    /// (`Foo`, `Foo.bar`), and an optional balanced argument list `( ... )`.
    /// Annotations carry no runtime meaning here, so they are dropped.
    fn skip_annotation(&mut self) {
        self.pos += 1; // '@'
        // dotted identifier
        loop {
            while {
                let c = self.peek();
                c.is_ascii_alphanumeric() || c == b'_'
            } {
                self.pos += 1;
            }
            if self.peek() == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.skip_trivia();
        if self.peek() == b'(' {
            let mut depth = 0i32;
            loop {
                match self.peek() {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        self.pos += 1;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    0 => break,
                    _ => {}
                }
                self.pos += 1;
            }
        }
    }

    fn lex_number(&mut self) -> Result<Tok, String> {
        let start = self.pos;
        // Hex integer literal (`0xFF2196F3`) — common for ARGB colours.
        if self.peek() == b'0' && (self.peek2() == b'x' || self.peek2() == b'X') {
            self.pos += 2;
            let hstart = self.pos;
            while self.peek().is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == hstart {
                return Err("bad hex literal".into());
            }
            let s = std::str::from_utf8(&self.src[hstart..self.pos]).unwrap();
            return Ok(Tok::Int(i64::from_str_radix(s, 16).map_err(|_| "bad hex literal")?));
        }
        let mut is_double = false;
        while self.peek().is_ascii_digit() {
            self.pos += 1;
        }
        if self.peek() == b'.' && self.peek2().is_ascii_digit() {
            is_double = true;
            self.pos += 1;
            while self.peek().is_ascii_digit() {
                self.pos += 1;
            }
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        if is_double {
            Ok(Tok::Double(s.parse().map_err(|_| "bad double")?))
        } else {
            Ok(Tok::Int(s.parse().map_err(|_| "bad int")?))
        }
    }

    /// Lex a string literal into interpolation parts. Supports `\n \t \\ \" \$`,
    /// `$identifier`, and `${expression}`.
    fn lex_string(&mut self, quote: u8) -> Result<Tok, String> {
        self.bump(); // opening quote
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            let c = self.peek();
            if c == 0 {
                return Err("unterminated string".into());
            }
            if c == quote {
                self.bump();
                break;
            }
            if c == b'\\' {
                self.bump();
                let e = self.bump();
                lit.push(match e {
                    b'n' => '\n',
                    b't' => '\t',
                    b'r' => '\r',
                    b'\\' => '\\',
                    b'$' => '$',
                    other => other as char,
                });
                continue;
            }
            if c == b'$' {
                if !lit.is_empty() {
                    parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                }
                self.bump();
                if self.peek() == b'{' {
                    self.bump();
                    let start = self.pos;
                    let mut depth = 1;
                    while depth > 0 {
                        let d = self.bump();
                        match d {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            0 => return Err("unterminated interpolation".into()),
                            _ => {}
                        }
                    }
                    let raw = std::str::from_utf8(&self.src[start..self.pos - 1]).unwrap().to_string();
                    parts.push(StrPart::Expr(raw));
                } else {
                    let start = self.pos;
                    while {
                        let d = self.peek();
                        d.is_ascii_alphanumeric() || d == b'_'
                    } {
                        self.pos += 1;
                    }
                    let raw = std::str::from_utf8(&self.src[start..self.pos]).unwrap().to_string();
                    parts.push(StrPart::Expr(raw));
                }
                continue;
            }
            lit.push(self.bump() as char);
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(lit));
        }
        Ok(Tok::Str(parts))
    }

    fn lex_op(&mut self) -> Result<Tok, String> {
        let c = self.bump();
        let two = |a: u8, b: u8, s: &mut Self| -> bool {
            if s.peek() == b {
                s.pos += 1;
                let _ = a;
                true
            } else {
                false
            }
        };
        let tok = match c {
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            b',' => Tok::Comma,
            b';' => Tok::Semi,
            b'.' => Tok::Dot,
            b'?' => {
                if two(b'?', b'?', self) {
                    Tok::Op("??".into())
                } else {
                    Tok::Question
                }
            }
            b':' => Tok::Colon,
            b'+' => {
                if two(b'+', b'+', self) {
                    Tok::Op("++".into())
                } else if two(b'+', b'=', self) {
                    Tok::Op("+=".into())
                } else {
                    Tok::Op("+".into())
                }
            }
            b'-' => {
                if two(b'-', b'-', self) {
                    Tok::Op("--".into())
                } else if two(b'-', b'=', self) {
                    Tok::Op("-=".into())
                } else {
                    Tok::Op("-".into())
                }
            }
            b'*' => {
                if two(b'*', b'=', self) {
                    Tok::Op("*=".into())
                } else {
                    Tok::Op("*".into())
                }
            }
            b'%' => Tok::Op("%".into()),
            b'/' => {
                if two(b'/', b'=', self) {
                    Tok::Op("/=".into())
                } else {
                    Tok::Op("/".into())
                }
            }
            b'~' => {
                if two(b'~', b'/', self) {
                    Tok::Op("~/".into())
                } else {
                    return Err("unexpected '~'".into());
                }
            }
            b'=' => {
                if two(b'=', b'=', self) {
                    Tok::Op("==".into())
                } else if two(b'=', b'>', self) {
                    Tok::Op("=>".into())
                } else {
                    Tok::Op("=".into())
                }
            }
            b'!' => {
                if two(b'!', b'=', self) {
                    Tok::Op("!=".into())
                } else {
                    Tok::Op("!".into())
                }
            }
            b'<' => {
                if two(b'<', b'=', self) {
                    Tok::Op("<=".into())
                } else {
                    Tok::Op("<".into())
                }
            }
            b'>' => {
                if two(b'>', b'=', self) {
                    Tok::Op(">=".into())
                } else {
                    Tok::Op(">".into())
                }
            }
            b'&' => {
                if two(b'&', b'&', self) {
                    Tok::Op("&&".into())
                } else {
                    return Err("unexpected '&'".into());
                }
            }
            b'|' => {
                if two(b'|', b'|', self) {
                    Tok::Op("||".into())
                } else {
                    return Err("unexpected '|'".into());
                }
            }
            other => return Err(format!("unexpected character '{}'", other as char)),
        };
        Ok(tok)
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Expr {
    Int(i64),
    Double(f64),
    Str(Vec<StrPart>),
    Bool(bool),
    Null,
    Ident(String),
    List(Vec<Expr>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>),
    /// Compound assignment `lhs op= rhs` (`+= -= *= /=`).
    AssignOp(String, Box<Expr>, Box<Expr>),
    /// `++`/`--`; the bool is true for prefix form.
    Update(String, Box<Expr>, bool),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>, Vec<(String, Expr)>),
    Index(Box<Expr>, Box<Expr>),
    /// Member access `obj.name`.
    Member(Box<Expr>, String),
    /// `this`.
    This,
    /// Instantiation `ClassName(args)` — Dart has no `new` keyword required.
    New(String, Vec<Expr>, Vec<(String, Expr)>),
    /// `expr is Type` — a reified type test.
    Is(Box<Expr>, String),
    /// `expr as Type` — a reified cast.
    As(Box<Expr>, String),
    /// A function expression / closure: `(params) => expr` or `(params) { body }`.
    Closure(ParamList, Vec<Stmt>),
    /// `await expr` inside an `async` function.
    Await(Box<Expr>),
    /// A map literal `{k: v, ...}` (entries with values) or a set literal
    /// `{a, b, ...}` (entries without values, lowered to a list).
    MapOrSet(Vec<(Expr, Option<Expr>)>),
}

#[derive(Debug, Clone)]
enum Stmt {
    Var(String, Option<Expr>),
    Expr(Expr),
    Return(Option<Expr>),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Block(Vec<Stmt>),
}

/// A single formal parameter.
#[derive(Debug, Clone)]
struct Param {
    name: String,
    /// `this.x` shorthand (constructors) — assigns the field directly.
    is_this: bool,
    /// Default value for optional-positional / named params.
    default: Option<Expr>,
}

/// A parsed parameter list: required-positional, optional-positional (`[...]`),
/// and named (`{...}`). Named params are lowered to a single trailing options
/// object; optional-positional to trailing params with null-default fill-in.
#[derive(Debug, Clone, Default)]
struct ParamList {
    positional: Vec<Param>,
    optional_pos: Vec<Param>,
    named: Vec<Param>,
}

impl ParamList {
    fn all_this_params(&self) -> impl Iterator<Item = &Param> {
        self.positional
            .iter()
            .chain(self.optional_pos.iter())
            .chain(self.named.iter())
            .filter(|p| p.is_this)
    }
}

const NAMED_ARG: &str = "__named";

#[derive(Debug, Clone)]
struct Method {
    name: String,
    params: ParamList,
    body: Vec<Stmt>,
    is_async: bool,
    /// A getter (`T get x => …`): declared with no parameters and read as a bare
    /// member (`obj.x`), which the emitter rewrites to a call `obj.x()`.
    is_getter: bool,
    /// A `static` member: belongs to the class, reached as `Class.member`.
    is_static: bool,
}

#[derive(Debug, Clone)]
struct ClassDecl {
    name: String,
    superclass: Option<String>,
    fields: Vec<(String, Option<Expr>)>,
    /// `static` fields (`static const foo = …`), reached as `Class.foo`.
    static_fields: Vec<(String, Option<Expr>)>,
    ctor_params: ParamList,
    ctor_body: Vec<Stmt>,
    has_ctor: bool,
    calls_super: bool,
    methods: Vec<Method>,
}

/// A Dart `enum Name { a, b, c }`, lowered to a top-level object mapping each
/// constant to its name string, so `Name.a` reads as a stable comparable value.
#[derive(Debug, Clone)]
struct EnumDecl {
    name: String,
    variants: Vec<String>,
}

#[derive(Debug, Clone)]
enum Item {
    Func(String, ParamList, Vec<Stmt>, bool /* is_async */),
    Class(ClassDecl),
    Enum(EnumDecl),
    Stmt(Stmt),
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    i: usize,
    class_names: std::collections::HashSet<String>,
    /// Monotonic counter for synthesizing unique temporaries (e.g. the iterator
    /// and index locals a `for-in` loop desugars to).
    for_seq: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
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

    fn parse_program(&mut self) -> Result<Vec<Item>, String> {
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

    fn parse_expr(&mut self) -> Result<Expr, String> {
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

// ---------------------------------------------------------------------------
// Emitter (Dart AST -> Elpian JS subset)
// ---------------------------------------------------------------------------

/// Runtime prelude prepended to every program. `__truncDiv` backs `~/`; the
/// `__List_*` functions implement the higher-order `Iterable` methods in the
/// language itself (the VM's indexer binds them to the receiver as `this` when
/// `list.map`/`.where`/… is read). They rely on `this.length`, `this[i]`,
/// `out.add(...)`, and closure calls — all VM-supported.
const PRELUDE: &str = concat!(
    "function __truncDiv(a, b){ return (a - (a % b)) / b; }\n",
    "function __ifNull(a, b){ if (a != null) { return a; } return b; }\n",
    "function __List_map(f){ var out = []; var i = 0; while (i < this.length) { out.add(f(this[i])); i = i + 1; } return out; }\n",
    "function __List_where(f){ var out = []; var i = 0; while (i < this.length) { if (f(this[i])) { out.add(this[i]); } i = i + 1; } return out; }\n",
    "function __List_forEach(f){ var i = 0; while (i < this.length) { f(this[i]); i = i + 1; } return null; }\n",
    "function __List_fold(init, f){ var acc = init; var i = 0; while (i < this.length) { acc = f(acc, this[i]); i = i + 1; } return acc; }\n",
    "function __List_any(f){ var i = 0; while (i < this.length) { if (f(this[i])) { return true; } i = i + 1; } return false; }\n",
    "function __List_every(f){ var i = 0; while (i < this.length) { if (!f(this[i])) { return false; } i = i + 1; } return true; }\n",
    "function __List_reduce(f){ var acc = this[0]; var i = 1; while (i < this.length) { acc = f(acc, this[i]); i = i + 1; } return acc; }\n",
    // ---- async/await runtime: Future + microtask-driven continuations -------
    "var __cbReg = [];\n",
    "function __later(fn){ var id = __cbReg.length; __cbReg.add(fn); askHost(\"dart:async/scheduleMicrotask\", [id]); }\n",
    "function __dartDispatch(a){ var fn = __cbReg[a[0]]; fn(); }\n",
    "function __schedThen(value, cb, next){ __later(function(){ var r = cb(value); if (r != null && r.__isFuture) { r.then(function(rv){ next.complete(rv); }); } else { next.complete(r); } }); }\n",
    "class _Future { constructor(){ this.__isFuture = true; this.done = false; this.value = null; this.cbs = []; } then(cb){ var next = new _Future(); if (this.done) { __schedThen(this.value, cb, next); } else { var p = {}; p.cb = cb; p.next = next; this.cbs.add(p); } return next; } complete(v){ if (this.done) { return; } this.done = true; this.value = v; var i = 0; while (i < this.cbs.length) { var p = this.cbs[i]; __schedThen(v, p.cb, p.next); i = i + 1; } } }\n",
    "function __Future_value(v){ var f = new _Future(); __later(function(){ f.complete(v); }); return f; }\n",
    "function __await(x){ if (x != null && x.__isFuture) { return x; } return __Future_value(x); }\n",
);

/// Transpile Dart-subset source to the JS subset the Elpian VM ingests.
pub fn transpile(dart: &str) -> Result<String, String> {
    Ok(transpile_program(dart)?.0)
}

/// A declared class and its optional superclass (for building a runtime
/// [`crate::types::ClassTable`]).
pub type ClassInfo = (String, Option<String>);

/// Transpile and also return the declared class hierarchy, so the runtime can
/// answer reified `is`/`as` checks over the same class relationships.
pub fn transpile_program(dart: &str) -> Result<(String, Vec<ClassInfo>), String> {
    let toks = Lexer::new(dart).tokenize()?;
    let class_names = {
        let mut set = std::collections::HashSet::new();
        for w in toks.windows(2) {
            if w[0] == Tok::Kw("class".into()) {
                if let Tok::Ident(n) = &w[1] {
                    set.insert(n.clone());
                }
            }
        }
        set
    };
    let mut items = Parser::new(toks).parse_program()?;
    box_captured_program(&mut items);
    let classes: Vec<ClassInfo> = items
        .iter()
        .filter_map(|it| match it {
            Item::Class(c) => Some((c.name.clone(), c.superclass.clone())),
            _ => None,
        })
        .collect();
    let mut em = Emitter::new(class_names);
    em.emit_program(&items);
    Ok((em.out, classes))
}

// ---------------------------------------------------------------------------
// By-reference closure capture (source transform)
// ---------------------------------------------------------------------------
//
// The VM captures closure upvalues by value, so a closure mutating an enclosing
// variable does not propagate. We recover Dart's by-reference semantics without
// a VM change: a local that is *captured by a nested closure* is boxed into a
// one-element list (`var v = [init]`), reads become `v[0]` and writes `v[0] = …`.
// Lists are reference types in the VM, so the closure and the enclosing scope
// share the same box and mutations are visible on both sides.
//
// Bounded: boxes locals declared in a function body (recursing into blocks, not
// into nested closures), captured by any nested closure. Shadowing a boxed name
// with a closure parameter is respected; re-declaring it as a local inside a
// closure is not (documented).

use std::collections::HashSet;

fn box_captured_program(items: &mut [Item]) {
    for item in items.iter_mut() {
        match item {
            Item::Func(_, _, body, _) => box_captured_fn(body),
            Item::Class(c) => {
                box_captured_fn(&mut c.ctor_body);
                for m in &mut c.methods {
                    box_captured_fn(&mut m.body);
                }
            }
            Item::Enum(_) => {}
            Item::Stmt(_) => {}
        }
    }
}

fn box_captured_fn(body: &mut Vec<Stmt>) {
    let mut declared = HashSet::new();
    collect_declared_stmts(body, &mut declared);
    let mut in_closures = HashSet::new();
    collect_closure_refs_stmts(body, &mut in_closures);
    let boxed: HashSet<String> = declared.intersection(&in_closures).cloned().collect();
    if boxed.is_empty() {
        return;
    }
    let taken = std::mem::take(body);
    *body = taken.into_iter().map(|s| rewrite_stmt(s, &boxed)).collect();
}

/// Variable names declared in these statements, recursing into control-flow
/// bodies but NOT into nested closures (those own their locals).
fn collect_declared_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Var(name, _) => {
                out.insert(name.clone());
            }
            Stmt::If(_, t, e) => {
                collect_declared_stmts(t, out);
                collect_declared_stmts(e, out);
            }
            Stmt::While(_, b) | Stmt::Block(b) => collect_declared_stmts(b, out),
            _ => {}
        }
    }
}

/// All identifier names referenced inside any closure nested in these statements.
fn collect_closure_refs_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Var(_, Some(e)) | Stmt::Expr(e) | Stmt::Return(Some(e)) => {
                collect_closure_refs_expr(e, out)
            }
            Stmt::If(c, t, el) => {
                collect_closure_refs_expr(c, out);
                collect_closure_refs_stmts(t, out);
                collect_closure_refs_stmts(el, out);
            }
            Stmt::While(c, b) => {
                collect_closure_refs_expr(c, out);
                collect_closure_refs_stmts(b, out);
            }
            Stmt::Block(b) => collect_closure_refs_stmts(b, out),
            _ => {}
        }
    }
}

fn collect_closure_refs_expr(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Closure(_, body) => collect_idents_stmts(body, out),
        Expr::Unary(_, a) | Expr::Update(_, a, _) | Expr::Await(a) | Expr::Member(a, _) => {
            collect_closure_refs_expr(a, out)
        }
        Expr::Binary(_, a, b)
        | Expr::Assign(a, b)
        | Expr::AssignOp(_, a, b)
        | Expr::Index(a, b) => {
            collect_closure_refs_expr(a, out);
            collect_closure_refs_expr(b, out);
        }
        Expr::Ternary(a, b, c) => {
            collect_closure_refs_expr(a, out);
            collect_closure_refs_expr(b, out);
            collect_closure_refs_expr(c, out);
        }
        Expr::Call(c, pos, named) => {
            collect_closure_refs_expr(c, out);
            for p in pos {
                collect_closure_refs_expr(p, out);
            }
            for (_, v) in named {
                collect_closure_refs_expr(v, out);
            }
        }
        Expr::New(_, pos, named) => {
            for p in pos {
                collect_closure_refs_expr(p, out);
            }
            for (_, v) in named {
                collect_closure_refs_expr(v, out);
            }
        }
        Expr::List(xs) => {
            for x in xs {
                collect_closure_refs_expr(x, out);
            }
        }
        Expr::MapOrSet(entries) => {
            for (k, v) in entries {
                collect_closure_refs_expr(k, out);
                if let Some(v) = v {
                    collect_closure_refs_expr(v, out);
                }
            }
        }
        Expr::Is(a, _) | Expr::As(a, _) => collect_closure_refs_expr(a, out),
        _ => {}
    }
}

/// Every identifier used in these statements (deep, including nested closures).
fn collect_idents_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Var(_, Some(e)) | Stmt::Expr(e) | Stmt::Return(Some(e)) => {
                collect_idents_expr(e, out)
            }
            Stmt::If(c, t, el) => {
                collect_idents_expr(c, out);
                collect_idents_stmts(t, out);
                collect_idents_stmts(el, out);
            }
            Stmt::While(c, b) => {
                collect_idents_expr(c, out);
                collect_idents_stmts(b, out);
            }
            Stmt::Block(b) => collect_idents_stmts(b, out),
            _ => {}
        }
    }
}

fn collect_idents_expr(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Ident(n) => {
            out.insert(n.clone());
        }
        Expr::Closure(_, body) => collect_idents_stmts(body, out),
        Expr::Unary(_, a) | Expr::Update(_, a, _) | Expr::Await(a) | Expr::Member(a, _) => {
            collect_idents_expr(a, out)
        }
        Expr::Binary(_, a, b)
        | Expr::Assign(a, b)
        | Expr::AssignOp(_, a, b)
        | Expr::Index(a, b) => {
            collect_idents_expr(a, out);
            collect_idents_expr(b, out);
        }
        Expr::Ternary(a, b, c) => {
            collect_idents_expr(a, out);
            collect_idents_expr(b, out);
            collect_idents_expr(c, out);
        }
        Expr::Call(c, pos, named) => {
            collect_idents_expr(c, out);
            for p in pos {
                collect_idents_expr(p, out);
            }
            for (_, v) in named {
                collect_idents_expr(v, out);
            }
        }
        Expr::New(_, pos, named) => {
            for p in pos {
                collect_idents_expr(p, out);
            }
            for (_, v) in named {
                collect_idents_expr(v, out);
            }
        }
        Expr::List(xs) => {
            for x in xs {
                collect_idents_expr(x, out);
            }
        }
        Expr::MapOrSet(entries) => {
            for (k, v) in entries {
                collect_idents_expr(k, out);
                if let Some(v) = v {
                    collect_idents_expr(v, out);
                }
            }
        }
        Expr::Is(a, _) | Expr::As(a, _) => collect_idents_expr(a, out),
        _ => {}
    }
}

fn rewrite_stmt(s: Stmt, boxed: &HashSet<String>) -> Stmt {
    match s {
        Stmt::Var(name, init) => {
            let init = init.map(|e| rewrite_expr(e, boxed));
            if boxed.contains(&name) {
                // var name = [ init ];  (the box)
                let inner = init.unwrap_or(Expr::Null);
                Stmt::Var(name, Some(Expr::List(vec![inner])))
            } else {
                Stmt::Var(name, init)
            }
        }
        Stmt::Expr(e) => Stmt::Expr(rewrite_expr(e, boxed)),
        Stmt::Return(e) => Stmt::Return(e.map(|e| rewrite_expr(e, boxed))),
        Stmt::If(c, t, el) => Stmt::If(
            rewrite_expr(c, boxed),
            t.into_iter().map(|s| rewrite_stmt(s, boxed)).collect(),
            el.into_iter().map(|s| rewrite_stmt(s, boxed)).collect(),
        ),
        Stmt::While(c, b) => Stmt::While(
            rewrite_expr(c, boxed),
            b.into_iter().map(|s| rewrite_stmt(s, boxed)).collect(),
        ),
        Stmt::Block(b) => Stmt::Block(b.into_iter().map(|s| rewrite_stmt(s, boxed)).collect()),
    }
}

/// Replace `boxed` variable reads with `v[0]` and writes with `v[0] = …`.
fn rewrite_expr(e: Expr, boxed: &HashSet<String>) -> Expr {
    let box_read = |n: &str| Expr::Index(Box::new(Expr::Ident(n.to_string())), Box::new(Expr::Int(0)));
    match e {
        Expr::Ident(n) if boxed.contains(&n) => box_read(&n),
        Expr::Assign(lhs, rhs) => {
            let rhs = rewrite_expr(*rhs, boxed);
            match *lhs {
                Expr::Ident(n) if boxed.contains(&n) => {
                    Expr::Assign(Box::new(box_read(&n)), Box::new(rhs))
                }
                other => Expr::Assign(Box::new(rewrite_expr(other, boxed)), Box::new(rhs)),
            }
        }
        Expr::AssignOp(op, lhs, rhs) => {
            let rhs = rewrite_expr(*rhs, boxed);
            match *lhs {
                Expr::Ident(n) if boxed.contains(&n) => {
                    Expr::AssignOp(op, Box::new(box_read(&n)), Box::new(rhs))
                }
                other => Expr::AssignOp(op, Box::new(rewrite_expr(other, boxed)), Box::new(rhs)),
            }
        }
        Expr::Update(op, target, pre) => match *target {
            Expr::Ident(n) if boxed.contains(&n) => Expr::Update(op, Box::new(box_read(&n)), pre),
            other => Expr::Update(op, Box::new(rewrite_expr(other, boxed)), pre),
        },
        Expr::Closure(params, body) => {
            // A parameter shadowing a boxed name is a fresh, unboxed local.
            let mut inner = boxed.clone();
            for p in params.positional.iter().chain(params.optional_pos.iter()).chain(params.named.iter()) {
                inner.remove(&p.name);
            }
            Expr::Closure(params, body.into_iter().map(|s| rewrite_stmt(s, &inner)).collect())
        }
        Expr::Unary(op, a) => Expr::Unary(op, Box::new(rewrite_expr(*a, boxed))),
        Expr::Await(a) => Expr::Await(Box::new(rewrite_expr(*a, boxed))),
        Expr::Member(a, n) => Expr::Member(Box::new(rewrite_expr(*a, boxed)), n),
        Expr::Binary(op, a, b) => {
            Expr::Binary(op, Box::new(rewrite_expr(*a, boxed)), Box::new(rewrite_expr(*b, boxed)))
        }
        Expr::Index(a, b) => {
            Expr::Index(Box::new(rewrite_expr(*a, boxed)), Box::new(rewrite_expr(*b, boxed)))
        }
        Expr::Ternary(a, b, c) => Expr::Ternary(
            Box::new(rewrite_expr(*a, boxed)),
            Box::new(rewrite_expr(*b, boxed)),
            Box::new(rewrite_expr(*c, boxed)),
        ),
        Expr::Call(c, pos, named) => Expr::Call(
            Box::new(rewrite_expr(*c, boxed)),
            pos.into_iter().map(|p| rewrite_expr(p, boxed)).collect(),
            named.into_iter().map(|(k, v)| (k, rewrite_expr(v, boxed))).collect(),
        ),
        Expr::New(name, pos, named) => Expr::New(
            name,
            pos.into_iter().map(|p| rewrite_expr(p, boxed)).collect(),
            named.into_iter().map(|(k, v)| (k, rewrite_expr(v, boxed))).collect(),
        ),
        Expr::List(xs) => Expr::List(xs.into_iter().map(|x| rewrite_expr(x, boxed)).collect()),
        Expr::MapOrSet(entries) => Expr::MapOrSet(
            entries
                .into_iter()
                .map(|(k, v)| (rewrite_expr(k, boxed), v.map(|v| rewrite_expr(v, boxed))))
                .collect(),
        ),
        Expr::Is(a, t) => Expr::Is(Box::new(rewrite_expr(*a, boxed)), t),
        Expr::As(a, t) => Expr::As(Box::new(rewrite_expr(*a, boxed)), t),
        other => other,
    }
}

/// Scope-aware emitter. Inside a class body it resolves bare field references to
/// `this.field` and bare method calls to `this.method(...)`, so idiomatic Dart
/// (which omits `this.`) lowers to valid JS.
type NameSet = std::collections::HashSet<String>;
type NameMap = std::collections::HashMap<String, NameSet>;

struct Emitter {
    out: String,
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
    locals: Vec<NameSet>,
    in_class: bool,
}

/// Native member names the VM binds as properties (not zero-arg getters), which
/// must never be rewritten to a call even if a user class declares a like-named
/// getter.
const NATIVE_PROPS: &[&str] = &[
    "length", "isEmpty", "isNotEmpty", "first", "last", "single", "keys", "values",
    "reversed", "iterator", "runtimeType", "hashCode",
];

impl Emitter {
    fn new(class_names: NameSet) -> Self {
        Emitter {
            out: String::from(PRELUDE),
            class_names,
            field_map: Default::default(),
            method_map: Default::default(),
            fields: Default::default(),
            methods: Default::default(),
            getters: Default::default(),
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
                }
                for p in c.ctor_params.all_this_params() {
                    all_fields.insert(p.name.clone());
                }
                for m in &c.methods {
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

    fn emit_program(&mut self, items: &[Item]) {
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
                if op == "~/" {
                    format!("__truncDiv({}, {})", self.emit_expr(a), self.emit_expr(b))
                } else if op == "??" {
                    format!("__ifNull({}, {})", self.emit_expr(a), self.emit_expr(b))
                } else {
                    format!("({} {} {})", self.emit_expr(a), op, self.emit_expr(b))
                }
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
                // A numeric-literal receiver needs parens: `7.clamp` would lex as
                // the float `7.` followed by `clamp`.
                let base = if matches!(&**obj, Expr::Int(_) | Expr::Double(_)) {
                    format!("({o}).{name}")
                } else {
                    format!("{o}.{name}")
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
            Expr::Is(x, ty) => {
                format!("askHost(\"dart:core/isType\", [{}, {}])", self.emit_expr(x), json_string(ty))
            }
            Expr::As(x, ty) => {
                format!("askHost(\"dart:core/asType\", [{}, {}])", self.emit_expr(x), json_string(ty))
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
                    return format!("{}.{}({})", recv, name, a.join(", "));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erases_types_and_lowers_trunc_div() {
        let js = transpile("int x = 7 ~/ 2;").unwrap();
        assert!(js.contains("var x = __truncDiv(7, 2)"), "got: {js}");
    }

    #[test]
    fn lowers_for_to_while_and_print_to_host_call() {
        let js = transpile("void main() { for (int i = 0; i < 3; i = i + 1) { print(i); } }").unwrap();
        assert!(js.contains("while ("), "got: {js}");
        assert!(js.contains("askHost(\"log\""), "got: {js}");
        assert!(js.contains("main();"), "should auto-call main: {js}");
    }

    #[test]
    fn string_interpolation_becomes_concatenation() {
        let js = transpile(r#"var s = "n=$x done";"#).unwrap();
        assert!(js.contains('+'), "interpolation should concat: {js}");
        assert!(js.contains("(x)"), "should reference x: {js}");
    }

    #[test]
    fn annotations_abstract_and_const_are_erased() {
        let dart = "@immutable\nabstract class Shape { const Shape(); }\n\
                    var s = const Shape();";
        let js = transpile(dart).unwrap();
        assert!(js.contains("class Shape"), "abstract erased to a class: {js}");
        assert!(!js.contains('@'), "annotation stripped: {js}");
        assert!(js.contains("new Shape()"), "const erased to instantiation: {js}");
    }

    #[test]
    fn statics_getters_and_named_constructors() {
        let dart = "class Color {\n\
                      final int value;\n\
                      const Color(this.value);\n\
                      static const int black = 4278190080;\n\
                      static Color fromValue(int v) => Color(v);\n\
                      int get red => (value ~/ 65536) % 256;\n\
                    }";
        let js = transpile(dart).unwrap();
        assert!(js.contains("static black = 4278190080"), "static field: {js}");
        assert!(js.contains("static fromValue"), "static method: {js}");
        // A getter is emitted as a method and *called* when read as a bare member.
        assert!(js.contains("red("), "getter emitted as method: {js}");
    }

    #[test]
    fn enum_lowers_to_name_object() {
        let js = transpile("enum Axis { horizontal, vertical }").unwrap();
        assert!(
            js.contains("var Axis = {horizontal: \"horizontal\", vertical: \"vertical\"}"),
            "enum -> name object: {js}"
        );
    }

    #[test]
    fn null_coalescing_lowers_to_helper() {
        let js = transpile("var x = a ?? 5;").unwrap();
        assert!(js.contains("__ifNull("), "?? -> __ifNull: {js}");
    }

    #[test]
    fn void_arrow_body_is_a_statement_not_a_return() {
        // A void arrow function must not `return` its call's value.
        let js = transpile("void main() => run();").unwrap();
        assert!(js.contains("function main"), "got: {js}");
        assert!(!js.contains("return run()"), "void arrow must be a statement: {js}");
    }

    #[test]
    fn hex_integer_literals_lex() {
        let js = transpile("var c = 0xFF2196F3;").unwrap();
        assert!(js.contains("4280391411"), "hex parsed to its value: {js}");
    }

    #[test]
    fn for_in_desugars_to_indexed_while() {
        // Typed loop var, bare form, and a C-style `for` in the same program all
        // parse; the for-in lowers to a length-bounded while over the iterable.
        let js = transpile(
            "void main() { for (var x in xs) { total = total + x; } \
             for (int i = 0; i < 3; i = i + 1) { print(i); } }",
        )
        .unwrap();
        assert!(js.contains("__for_it0"), "for-in should bind an iterator temp: {js}");
        assert!(js.contains(".length"), "for-in should bound on length: {js}");
        assert!(js.contains("while"), "for-in lowers to while: {js}");
    }

    #[test]
    fn parses_function_with_typed_params() {
        let js = transpile("int add(int a, int b) { return a + b; }").unwrap();
        assert!(js.contains("function add(a, b)"), "got: {js}");
    }

    #[test]
    fn emits_native_class_with_field_resolution() {
        let dart = r#"
            class Counter {
                int value = 0;
                Counter(this.value);
                void inc() { value = value + 1; }
            }
        "#;
        let js = transpile(dart).unwrap();
        assert!(js.contains("class Counter {"), "got: {js}");
        assert!(js.contains("constructor(value)"), "got: {js}");
        assert!(js.contains("this.value = value"), "got: {js}");
        // Bare field ref inside a method resolves to this.value.
        assert!(js.contains("this.value = (this.value + 1)"), "got: {js}");
    }

    #[test]
    fn emits_inheritance_and_super() {
        let dart = "class A { } class B extends A { int x = 1; }";
        let js = transpile(dart).unwrap();
        assert!(js.contains("class B extends A {"), "got: {js}");
        assert!(js.contains("super();"), "got: {js}");
    }

    #[test]
    fn instantiation_and_ternary_and_compound() {
        let dart = "class P { } var p = P(); var y = 1 > 0 ? 2 : 3; var z = 5; z += 4;";
        let js = transpile(dart).unwrap();
        assert!(js.contains("new P()"), "got: {js}");
        assert!(js.contains("? 2 : 3"), "got: {js}");
        assert!(js.contains("z += 4"), "got: {js}");
    }
}
