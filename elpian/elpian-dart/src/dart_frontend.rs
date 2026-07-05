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
//! * `if`/`else`, `while`, C-style `for` (lowered to `while`), `return`, blocks;
//! * expressions: literals, identifiers, calls, list literals, indexing,
//!   assignment + compound assignment (`+= -= *= /=`), `++`/`--`, ternary
//!   `?:`, `|| && == != < <= > >= + - * / % ~/`, unary `! -`;
//! * string interpolation (`"$x"`, `"${expr}"`) lowered to concatenation;
//! * `print(x)` lowered to `askHost("log",[x])`; `~/` lowered to a trunc-div
//!   helper. `main()` is auto-invoked if present.
//!
//! NOT yet covered (later phases): mixins, generics, named args, pattern
//! matching, initializer lists with super-args, `async`/`await` sugar (the
//! runtime primitives exist; the sugar is a later step).

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
    "String", "bool", "dynamic", "class", "extends", "this", "new", "super", "is", "as",
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

    fn lex_number(&mut self) -> Result<Tok, String> {
        let start = self.pos;
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
            b'?' => Tok::Question,
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
    Call(Box<Expr>, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    /// Member access `obj.name`.
    Member(Box<Expr>, String),
    /// `this`.
    This,
    /// Instantiation `ClassName(args)` — Dart has no `new` keyword required.
    New(String, Vec<Expr>),
    /// `expr is Type` — a reified type test.
    Is(Box<Expr>, String),
    /// `expr as Type` — a reified cast.
    As(Box<Expr>, String),
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

#[derive(Debug, Clone)]
struct CtorParam {
    name: String,
    /// `this.x` shorthand — assigns the field directly.
    is_this: bool,
}

#[derive(Debug, Clone)]
struct Method {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
struct ClassDecl {
    name: String,
    superclass: Option<String>,
    fields: Vec<(String, Option<Expr>)>,
    ctor_params: Vec<CtorParam>,
    ctor_body: Vec<Stmt>,
    has_ctor: bool,
    calls_super: bool,
    methods: Vec<Method>,
}

#[derive(Debug, Clone)]
enum Item {
    Func(String, Vec<String>, Vec<Stmt>),
    Class(ClassDecl),
    Stmt(Stmt),
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    i: usize,
    class_names: std::collections::HashSet<String>,
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
        Parser { toks, i: 0, class_names }
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

    /// A top-level item is a function declaration or a statement. Function form:
    /// `[type] name ( params ) { body }` — detected by lookahead for `(...) {`.
    fn parse_item(&mut self) -> Result<Item, String> {
        if *self.peek() == Tok::Kw("class".into()) {
            return Ok(Item::Class(self.parse_class()?));
        }
        if self.looks_like_function() {
            return self.parse_function();
        }
        Ok(Item::Stmt(self.parse_stmt()?))
    }

    fn parse_class(&mut self) -> Result<ClassDecl, String> {
        self.bump(); // 'class'
        let name = self.ident()?;
        let superclass = if *self.peek() == Tok::Kw("extends".into()) {
            self.bump();
            Some(self.ident()?)
        } else {
            None
        };
        self.eat(&Tok::LBrace)?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut ctor_params = Vec::new();
        let mut ctor_body = Vec::new();
        let mut has_ctor = false;

        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof {
            // Optional leading type (primitive kw, or `Type name` where Type is
            // an identifier followed by another identifier).
            if self.is_type_kw()
                || (matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek_at(1), Tok::Ident(_)))
                || *self.peek() == Tok::Kw("var".into())
                || *self.peek() == Tok::Kw("final".into())
            {
                self.bump(); // erase the type / var / final
            }
            let member_name = self.ident()?;
            if *self.peek() == Tok::LParen {
                // constructor or method
                if member_name == name {
                    has_ctor = true;
                    ctor_params = self.parse_ctor_params()?;
                    // A constructor body may be a block or just `;` (common with
                    // initializing formals: `Counter(this.value);`).
                    ctor_body = if *self.peek() == Tok::Semi {
                        self.bump();
                        Vec::new()
                    } else {
                        self.parse_block()?
                    };
                } else {
                    self.eat(&Tok::LParen)?;
                    let mut params = Vec::new();
                    while *self.peek() != Tok::RParen {
                        if self.is_type_kw()
                            || (matches!(self.peek(), Tok::Ident(_))
                                && matches!(self.peek_at(1), Tok::Ident(_)))
                        {
                            self.bump();
                        }
                        params.push(self.ident()?);
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    let body = self.parse_block()?;
                    methods.push(Method { name: member_name, params, body });
                }
            } else {
                // field: optional initializer, then ';'
                let init = if *self.peek() == Tok::Op("=".into()) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.eat(&Tok::Semi)?;
                fields.push((member_name, init));
            }
        }
        self.eat(&Tok::RBrace)?;
        let calls_super = superclass.is_some();
        Ok(ClassDecl {
            name,
            superclass,
            fields,
            ctor_params,
            ctor_body,
            has_ctor,
            calls_super,
            methods,
        })
    }

    fn parse_ctor_params(&mut self) -> Result<Vec<CtorParam>, String> {
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while *self.peek() != Tok::RParen {
            if *self.peek() == Tok::Kw("this".into()) {
                self.bump();
                self.eat(&Tok::Dot)?;
                params.push(CtorParam { name: self.ident()?, is_this: true });
            } else {
                if self.is_type_kw()
                    || (matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek_at(1), Tok::Ident(_)))
                {
                    self.bump();
                }
                params.push(CtorParam { name: self.ident()?, is_this: false });
            }
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RParen)?;
        Ok(params)
    }

    fn looks_like_function(&self) -> bool {
        let mut k = 0;
        if self.is_type_kw() {
            k += 1;
        }
        // need: ident '(' ... ')' '{'
        if !matches!(self.peek_at(k), Tok::Ident(_)) {
            return false;
        }
        k += 1;
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
        *self.peek_at(k) == Tok::LBrace
    }

    fn parse_function(&mut self) -> Result<Item, String> {
        if self.is_type_kw() {
            self.bump();
        }
        let name = self.ident()?;
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while *self.peek() != Tok::RParen {
            // optional type then name
            if self.is_type_kw() || matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump(); // erase the type
            }
            params.push(self.ident()?);
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RParen)?;
        let body = self.parse_block()?;
        Ok(Item::Func(name, params, body))
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

    fn parse_unary(&mut self) -> Result<Expr, String> {
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
                    self.bump();
                    let mut args = Vec::new();
                    while *self.peek() != Tok::RParen {
                        args.push(self.parse_expr()?);
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    // `ClassName(args)` (no member/index in front) is a Dart
                    // instantiation, not a plain call.
                    e = match e {
                        Expr::Ident(name) if self.class_names.contains(&name) => {
                            Expr::New(name, args)
                        }
                        callee => Expr::Call(Box::new(callee), args),
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
                self.eat(&Tok::LParen)?;
                let mut args = Vec::new();
                while *self.peek() != Tok::RParen {
                    args.push(self.parse_expr()?);
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    }
                }
                self.eat(&Tok::RParen)?;
                Ok(Expr::New(name, args))
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
            other => Err(format!("unexpected token in expression: {other:?}")),
        }
    }
}

fn binding_power(op: &str) -> Option<u8> {
    Some(match op {
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

const PRELUDE: &str = "function __truncDiv(a, b){ return (a - (a % b)) / b; }\n";

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
    let items = Parser::new(toks).parse_program()?;
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
    locals: Vec<NameSet>,
    in_class: bool,
}

impl Emitter {
    fn new(class_names: NameSet) -> Self {
        Emitter {
            out: String::from(PRELUDE),
            class_names,
            field_map: Default::default(),
            method_map: Default::default(),
            fields: Default::default(),
            methods: Default::default(),
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
        for item in items {
            if let Item::Class(c) = item {
                own_fields.insert(c.name.clone(), c.fields.iter().map(|(n, _)| n.clone()).collect());
                own_methods.insert(c.name.clone(), c.methods.iter().map(|m| m.name.clone()).collect());
                // `this.x` params are also fields.
                if let Some(set) = own_fields.get_mut(&c.name) {
                    for p in &c.ctor_params {
                        if p.is_this {
                            set.insert(p.name.clone());
                        }
                    }
                }
                supers.insert(c.name.clone(), c.superclass.clone());
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
                Item::Func(name, params, body) => {
                    if name == "main" {
                        has_main = true;
                    }
                    self.out.push_str(&format!("function {}({}) {{\n", name, params.join(", ")));
                    self.push_scope();
                    for p in params {
                        self.declare(p);
                    }
                    self.emit_stmts(body, 1);
                    self.pop_scope();
                    self.out.push_str("}\n");
                }
                Item::Class(c) => self.emit_class(c),
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
            let sig: Vec<String> = c.ctor_params.iter().map(|p| p.name.clone()).collect();
            self.out.push_str(&format!("  constructor({}) {{\n", sig.join(", ")));
            self.push_scope();
            for p in &c.ctor_params {
                self.declare(&p.name);
            }
            if c.calls_super {
                self.out.push_str("    super();\n");
            }
            // Reified-type tag: most-derived ctor wins (runs last).
            self.out.push_str(&format!("    this.__class = {};\n", json_string(&c.name)));
            // Field initializers run first; an initializing formal (`this.x`)
            // then wins, matching Dart's initialization order.
            for (fname, init) in &c.fields {
                if let Some(e) = init {
                    let v = self.emit_expr(e);
                    self.out.push_str(&format!("    this.{fname} = {v};\n"));
                }
            }
            for p in &c.ctor_params {
                if p.is_this {
                    self.out.push_str(&format!("    this.{} = {};\n", p.name, p.name));
                }
            }
            self.emit_stmts(&c.ctor_body, 2);
            self.pop_scope();
            self.out.push_str("  }\n");
        }

        for m in &c.methods {
            self.out.push_str(&format!("  {}({}) {{\n", m.name, m.params.join(", ")));
            self.push_scope();
            for p in &m.params {
                self.declare(p);
            }
            self.emit_stmts(&m.body, 2);
            self.pop_scope();
            self.out.push_str("  }\n");
        }

        self.out.push_str("}\n");
        self.in_class = false;
        self.fields.clear();
        self.methods.clear();
    }

    fn emit_stmts(&mut self, stmts: &[Stmt], depth: usize) {
        for s in stmts {
            self.emit_stmt(s, depth);
        }
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
            Expr::Member(obj, name) => format!("{}.{}", self.emit_expr(obj), name),
            Expr::New(name, args) => {
                let a: Vec<String> = args.iter().map(|x| self.emit_expr(x)).collect();
                format!("new {}({})", name, a.join(", "))
            }
            Expr::Is(x, ty) => {
                format!("askHost(\"dart:core/isType\", [{}, {}])", self.emit_expr(x), json_string(ty))
            }
            Expr::As(x, ty) => {
                format!("askHost(\"dart:core/asType\", [{}, {}])", self.emit_expr(x), json_string(ty))
            }
            Expr::Call(callee, args) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "print" && args.len() == 1 {
                        let a0 = self.emit_expr(&args[0]);
                        return format!("askHost(\"log\", [{a0}])");
                    }
                    // Bare call to an own method inside a class -> this.method().
                    if self.in_class && !self.is_local(name) && self.methods.contains(name) {
                        let a: Vec<String> = args.iter().map(|x| self.emit_expr(x)).collect();
                        return format!("this.{}({})", name, a.join(", "));
                    }
                }
                let c = self.emit_expr(callee);
                let a: Vec<String> = args.iter().map(|x| self.emit_expr(x)).collect();
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
