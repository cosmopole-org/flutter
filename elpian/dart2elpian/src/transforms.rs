//! Source-to-source transforms applied after parsing (by-reference closure
//! capture via the one-element-list box).

use crate::ast::*;

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

pub(crate) fn box_captured_program(items: &mut [Item]) {
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

