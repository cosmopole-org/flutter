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
//! * `if`/`else`, `while`, C-style `for` (lowered to `while`), `return`, blocks;
//! * expressions: literals, identifiers, calls, list literals, indexing,
//!   assignment, `|| && == != < <= > >= + - * / % ~/`, unary `! -`;
//! * string interpolation (`"$x"`, `"${expr}"`) lowered to concatenation;
//! * `print(x)` lowered to `askHost("log",[x])`; `~/` lowered to a trunc-div
//!   helper. `main()` is auto-invoked if present.
//!
//! NOT yet covered (later phases): classes/mixins, generics, pattern matching,
//! `async`/`await` sugar (the runtime primitives exist; the sugar is Phase 4+).

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
    "String", "bool", "dynamic",
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
            b'+' => Tok::Op("+".into()),
            b'-' => Tok::Op("-".into()),
            b'*' => Tok::Op("*".into()),
            b'%' => Tok::Op("%".into()),
            b'/' => Tok::Op("/".into()),
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
    Call(Box<Expr>, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
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
enum Item {
    Func(String, Vec<String>, Vec<Stmt>),
    Stmt(Stmt),
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Parser { toks, i: 0 }
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
        if self.looks_like_function() {
            return self.parse_function();
        }
        Ok(Item::Stmt(self.parse_stmt()?))
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
        let lhs = self.parse_binary(0)?;
        if *self.peek() == Tok::Op("=".into()) {
            self.bump();
            let rhs = self.parse_assign()?;
            return Ok(Expr::Assign(Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
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
                    e = Expr::Call(Box::new(e), args);
                }
                Tok::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    self.eat(&Tok::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
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
    let toks = Lexer::new(dart).tokenize()?;
    let items = Parser::new(toks).parse_program()?;
    let mut out = String::from(PRELUDE);
    let mut has_main = false;
    for item in &items {
        match item {
            Item::Func(name, params, body) => {
                if name == "main" {
                    has_main = true;
                }
                out.push_str(&format!("function {}({}) {{\n", name, params.join(", ")));
                emit_stmts(body, &mut out, 1);
                out.push_str("}\n");
            }
            Item::Stmt(s) => {
                emit_stmt(s, &mut out, 0);
            }
        }
    }
    if has_main {
        out.push_str("main();\n");
    }
    Ok(out)
}

fn indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

fn emit_stmts(stmts: &[Stmt], out: &mut String, depth: usize) {
    for s in stmts {
        emit_stmt(s, out, depth);
    }
}

fn emit_stmt(s: &Stmt, out: &mut String, depth: usize) {
    indent(out, depth);
    match s {
        Stmt::Var(name, init) => {
            match init {
                Some(e) => out.push_str(&format!("var {} = {};\n", name, emit_expr(e))),
                None => out.push_str(&format!("var {};\n", name)),
            }
        }
        Stmt::Expr(e) => out.push_str(&format!("{};\n", emit_expr(e))),
        Stmt::Return(e) => match e {
            Some(e) => out.push_str(&format!("return {};\n", emit_expr(e))),
            None => out.push_str("return;\n"),
        },
        Stmt::If(c, t, e) => {
            out.push_str(&format!("if ({}) {{\n", emit_expr(c)));
            emit_stmts(t, out, depth + 1);
            indent(out, depth);
            out.push('}');
            if !e.is_empty() {
                out.push_str(" else {\n");
                emit_stmts(e, out, depth + 1);
                indent(out, depth);
                out.push('}');
            }
            out.push('\n');
        }
        Stmt::While(c, b) => {
            out.push_str(&format!("while ({}) {{\n", emit_expr(c)));
            emit_stmts(b, out, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        Stmt::Block(b) => {
            out.push_str("{\n");
            emit_stmts(b, out, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
    }
}

fn emit_expr(e: &Expr) -> String {
    match e {
        Expr::Int(i) => i.to_string(),
        Expr::Double(d) => {
            // Ensure a decimal point survives so it stays a double.
            if d.fract() == 0.0 {
                format!("{d:.1}")
            } else {
                d.to_string()
            }
        }
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".into(),
        Expr::Ident(s) => s.clone(),
        Expr::Str(parts) => emit_string(parts),
        Expr::List(xs) => {
            let inner: Vec<String> = xs.iter().map(emit_expr).collect();
            format!("[{}]", inner.join(", "))
        }
        Expr::Unary(op, x) => format!("({}{})", op, emit_expr(x)),
        Expr::Binary(op, a, b) => {
            if op == "~/" {
                format!("__truncDiv({}, {})", emit_expr(a), emit_expr(b))
            } else {
                format!("({} {} {})", emit_expr(a), op, emit_expr(b))
            }
        }
        Expr::Assign(a, b) => format!("{} = {}", emit_expr(a), emit_expr(b)),
        Expr::Index(a, i) => format!("{}[{}]", emit_expr(a), emit_expr(i)),
        Expr::Call(callee, args) => {
            // Lower `print(x)` to the log host call.
            if let Expr::Ident(name) = &**callee {
                if name == "print" && args.len() == 1 {
                    return format!("askHost(\"log\", [{}])", emit_expr(&args[0]));
                }
            }
            let a: Vec<String> = args.iter().map(emit_expr).collect();
            format!("{}({})", emit_expr(callee), a.join(", "))
        }
    }
}

fn emit_string(parts: &[StrPart]) -> String {
    if parts.len() == 1 {
        if let StrPart::Lit(s) = &parts[0] {
            return json_string(s);
        }
    }
    let mut pieces = Vec::new();
    // Start with an empty string so a leading expr coerces to string.
    pieces.push("\"\"".to_string());
    for p in parts {
        match p {
            StrPart::Lit(s) => pieces.push(json_string(s)),
            StrPart::Expr(raw) => {
                // Re-parse the interpolation expression and emit it.
                let sub = transpile_expr(raw).unwrap_or_else(|_| "null".into());
                pieces.push(format!("({})", sub));
            }
        }
    }
    format!("({})", pieces.join(" + "))
}

/// Parse and emit a bare expression (used for `${...}` interpolation chunks).
fn transpile_expr(src: &str) -> Result<String, String> {
    let toks = Lexer::new(src).tokenize()?;
    let mut p = Parser::new(toks);
    let e = p.parse_expr()?;
    Ok(emit_expr(&e))
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
}
