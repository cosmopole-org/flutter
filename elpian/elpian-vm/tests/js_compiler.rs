//! End-to-end tests for the JavaScript front-end: JS source is lowered to the
//! Elpian AST by the compiler module and run through the exact same
//! AST → bytecode → executor path as hand-written ASTs, via the public `api`.

use elpian_vm::api;

/// Register a VM from JS, run its top-level program, then call `func` and return
/// the stringified result value.
fn run_js_and_call(id: &str, js: &str, func: &str) -> String {
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()), "JS should compile");
    let _ = api::execute_vm(id.to_string());
    api::execute_vm_func(id.to_string(), func.to_string(), 1).result_value
}

#[test]
fn arithmetic_respects_precedence() {
    // 2 + 3 * 4 == 14 ; ** binds tightest and is right-associative.
    let js = "function f() { return 2 + 3 * 4; }";
    assert_eq!(run_js_and_call("js-arith", js, "f"), "14");

    let js2 = "function f() { return 2 ** 3 ** 2; }"; // 2 ** (3 ** 2) = 512
    assert_eq!(run_js_and_call("js-pow", js2, "f"), "512");
}

#[test]
fn builtins_are_callable_from_js() {
    let js = "function f() { return pow(2, 10); }";
    assert_eq!(run_js_and_call("js-builtin", js, "f"), "1024");

    // Nested calls compose: max(gcd(54, 24), sqrt(81)) = max(6, 9) = 9.
    let js2 = "function f() { return max(gcd(54, 24), sqrt(81)); }";
    assert_eq!(run_js_and_call("js-nested", js2, "f"), "9");
}

#[test]
fn string_builtins_from_js() {
    let js = "function f() { return upper(concat(\"el\", \"pa\")); }";
    assert_eq!(run_js_and_call("js-str", js, "f"), "\"ELPA\"");
}

#[test]
fn let_and_assignment() {
    let js = "function f() { let x = 5; x = x + 1; return x; }";
    assert_eq!(run_js_and_call("js-assign", js, "f"), "6");

    // Compound assignment.
    let js2 = "function f() { let x = 10; x *= 3; x -= 4; return x; }";
    assert_eq!(run_js_and_call("js-compound", js2, "f"), "26");
}

#[test]
fn if_else_if_else_chain() {
    // Exercises the full `ifStmt` / `elseifStmt` / `elseStmt` lowering with a
    // `return` inside each branch (early return out of the function).
    let js = "function classify(n) {
        if (n > 10) { return 1; }
        else if (n > 5) { return 2; }
        else { return 3; }
    }";
    let id = "js-if";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    let call = |n: i64| {
        api::execute_vm_func_with_input(id.to_string(), "classify".into(), n.to_string(), 1)
            .result_value
    };
    assert_eq!(call(7), "2");
    assert_eq!(call(20), "1");
    assert_eq!(call(1), "3");
}

#[test]
fn early_return_skips_rest_of_body() {
    // The statement after the taken branch's return must not run.
    let js = "function f(n) {
        if (n > 0) { return 1; }
        return 2;
    }";
    let id = "js-early-return";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    let call = |n: i64| {
        api::execute_vm_func_with_input(id.to_string(), "f".into(), n.to_string(), 1).result_value
    };
    assert_eq!(call(5), "1");
    assert_eq!(call(-5), "2");
}

#[test]
fn return_from_inside_loop() {
    // Return out of a while loop: find the first i whose square reaches 10.
    let js = "function firstBig() {
        let i = 0;
        while (i < 100) {
            if (i * i >= 10) { return i; }
            i = i + 1;
        }
        return -1;
    }";
    assert_eq!(run_js_and_call("js-ret-loop", js, "firstBig"), "4");
}

#[test]
fn guard_clause_in_called_function() {
    // An in-program call whose result comes from a guard-clause return nested in
    // an `if`, consumed by the caller's own expression.
    let js = "function pick(n) {
        if (n > 0) { return 100; }
        return 200;
    }
    function f() { return pick(5) + pick(-5); }";
    assert_eq!(run_js_and_call("js-guard", js, "f"), "300");
}

#[test]
fn while_loop_accumulates() {
    // sum 0..4 = 10
    let js = "function f() {
        let i = 0;
        let s = 0;
        while (i < 5) { s = s + i; i = i + 1; }
        return s;
    }";
    assert_eq!(run_js_and_call("js-while", js, "f"), "10");
}

#[test]
fn for_loop_desugars_and_runs() {
    // Uses both `i++` in the update clause and a body assignment.
    let js = "function f() {
        let s = 0;
        for (let i = 0; i < 5; i++) { s = s + i; }
        return s;
    }";
    assert_eq!(run_js_and_call("js-for", js, "f"), "10");
}

#[test]
fn top_level_state_and_function() {
    // Top-level `let` runs during execute_vm; the function closes over it.
    let js = "let x = 5; function getx() { return x; }";
    let id = "js-toplevel";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    assert_eq!(api::execute_vm_func(id.to_string(), "getx".into(), 1).result_value, "5");
}

#[test]
fn unary_minus_and_not() {
    let js = "function f() { return -3 + 5; }";
    assert_eq!(run_js_and_call("js-neg", js, "f"), "2");
}

#[test]
fn recursion_with_guard_clause() {
    // Recursive factorial: a base-case `return` nested in an `if`, plus a
    // recursive in-program call inside an arithmetic expression. Exercises the
    // return-unwinding across many stacked call frames.
    let js = "function fact(n) {
        if (n <= 1) { return 1; }
        return n * fact(n - 1);
    }
    function f() { return fact(5); }";
    assert_eq!(run_js_and_call("js-fact", js, "f"), "120");
}

#[test]
fn switch_with_returns() {
    // Return out of a switch case; execution after the switch is reached only
    // when no case matched.
    let js = "function classify(n) {
        switch (n) {
            case 1: return 10;
            case 2: return 20;
        }
        return 0;
    }";
    let id = "js-switch";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    let call = |n: i64| {
        api::execute_vm_func_with_input(id.to_string(), "classify".into(), n.to_string(), 1)
            .result_value
    };
    assert_eq!(call(1), "10");
    assert_eq!(call(2), "20");
    assert_eq!(call(3), "0");
}

#[test]
fn function_without_return_does_not_leak_previous_result() {
    // A function with an explicit return followed by one without a return: the
    // second must not inherit the first's value (no stale pending result).
    let js = "function getfive() { return 5; } function noret() { let x = 1; }";
    let id = "js-noleak";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    assert_eq!(api::execute_vm_func(id.to_string(), "getfive".into(), 1).result_value, "5");
    let noret = api::execute_vm_func(id.to_string(), "noret".into(), 2).result_value;
    assert_ne!(noret, "5", "no-return function leaked the previous result");
}

#[test]
fn validate_js_accepts_and_rejects() {
    assert!(api::validate_js("function f() { return 1 + 2; }".to_string()));
    // Unterminated block is outside the supported subset → rejected, no panic.
    assert!(!api::validate_js("function f() { return ".to_string()));
}

#[test]
fn invalid_js_fails_to_create_vm() {
    // A stray operator with no operand cannot be lowered; creation returns false.
    assert!(!api::create_vm_from_js("js-bad".to_string(), "let x = = ;".to_string()));
}

#[test]
fn compile_js_to_ast_produces_program_node() {
    let ast = api::compile_js_to_ast("let x = 1;".to_string());
    assert!(ast.contains("\"program\""), "ast was: {ast}");
    assert!(ast.contains("\"definition\""), "ast was: {ast}");
}

#[test]
fn arrow_function_value_is_callable() {
    // A concise-body arrow stored in a variable, then invoked.
    let js = "let dbl = x => x * 2; function f() { return dbl(21); }";
    assert_eq!(run_js_and_call("js-arrow", js, "f"), "42");

    // Multi-param arrow with a block body and an explicit return.
    let js2 = "let add = (a, b) => { return a + b; }; function f() { return add(40, 2); }";
    assert_eq!(run_js_and_call("js-arrow2", js2, "f"), "42");
}

#[test]
fn arrow_passed_as_callback_argument() {
    // An arrow handed straight to an in-program higher-order function, then
    // called through the parameter that holds it — exercising the lift into a
    // synthetic definition plus calling a function value held in a variable.
    let js = "
        function apply(fn, v) { return fn(v); }
        function f() { return apply(n => n + 1, 10); }";
    assert_eq!(run_js_and_call("js-arrow-arg", js, "f"), "11");
}

#[test]
fn arrow_closes_over_local_per_iteration() {
    // Each loop iteration's `let k` is captured independently by the closure
    // created that turn — the canonical closure-per-iteration behaviour.
    let js = "
        function build() {
            let fns = [];
            for (let i = 0; i < 3; i++) {
                let k = i;
                push(fns, () => k * 10);
            }
            return fns;
        }
        function f() {
            let fns = build();
            return fns[0]() + fns[1]() + fns[2]();
        }"; // 0 + 10 + 20
    assert_eq!(run_js_and_call("js-arrow-closure", js, "f"), "30");
}

#[test]
fn arrow_in_object_field_is_invocable() {
    // A function value stored in an object field and called as `obj.field()` —
    // the shape the demo's widgets use for tap callbacks.
    let js = "
        function f() {
            let base = 5;
            let w = { onTap: () => base + 2 };
            return w.onTap();
        }";
    assert_eq!(run_js_and_call("js-arrow-field", js, "f"), "7");
}

#[test]
fn anonymous_function_expression_is_callable() {
    let js = "let sq = function (x) { return x * x; }; function f() { return sq(9); }";
    assert_eq!(run_js_and_call("js-fnexpr", js, "f"), "81");
}

#[test]
fn member_and_index_assignment_mutate_in_place() {
    // Assigning to `obj.field`, `obj[key]`, and `arr[i]` — the lvalues a widget
    // framework leans on. Exercises the indexer-assignment path end to end.
    let js = "
        function f() {
            let o = { a: 1 };
            o.a = 5;            // member assign
            o[\"b\"] = 7;        // computed string-key assign
            let arr = [10, 20, 30];
            arr[1] = 99;        // array element assign
            return o.a + o.b + arr[1];   // 5 + 7 + 99
        }";
    assert_eq!(run_js_and_call("js-member-assign", js, "f"), "111");
}

#[test]
fn closure_mutates_shared_object_field() {
    // A closure stored in a field mutates another field of the same object — the
    // component-`update` / widget-`onTap` shape.
    let js = "
        function f() {
            let w = { n: 0 };
            w.bump = () => { w.n = w.n + 1; };
            w.bump(); w.bump(); w.bump();
            return w.n;
        }";
    assert_eq!(run_js_and_call("js-field-closure", js, "f"), "3");
}

#[test]
fn statement_after_control_block_does_not_unbalance_in_called_fn() {
    // Regression: a control-flow body (`if`/`for`/nested `if`) whose body holds a
    // *call statement* must not pop the enclosing function frame's `DummyOp` when
    // it ends. Before the fix, a statement following such a block in a CALLED
    // function leaked its discarded value into the caller's awaiting expression,
    // corrupting returns (`return a` after conditional pushes came back null) and
    // mis-aligning later object literals ("array used as object key" traps).
    let id = "js-ctrl-balance";
    let js = "
        function build(node) {
            let a = [];
            if (has(node, \"x\")) { push(a, node.x); }   // call-statement in if-body
            if (has(node, \"y\")) { push(a, node.y); }   // ...followed by another
            push(a, 99);                                  // ...and a trailing statement
            return a;                                     // return the built array
        }
        function f() {
            let r = build({ x: 1, y: 2 });
            // Build an object *after* the call; a leaked frame would desync its
            // key/value pairing.
            let o = { len: len(r), first: r[0], last: r[2] };
            return o.len * 100 + o.first * 10 + o.last;   // 3*100 + 1*10 + 99 = 409
        }";
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()));
    let _ = api::execute_vm(id.to_string());
    assert_eq!(api::execute_vm_func(id.to_string(), "f".into(), 1).result_value, "409");

    // The same shape with a `for` loop body and an in-loop conditional value.
    let id2 = "js-ctrl-balance-loop";
    let js2 = "
        function build() {
            let a = [];
            for (let i = 0; i < 3; i++) {
                let tag = \"lo\";
                if (i == 2) { tag = \"hi\"; }
                push(a, { i: i, tag: tag });   // object literal after let+if in a loop
            }
            return a;
        }
        function f() { let a = build(); return concat(a[2].tag, str(len(a))); }";
    assert!(api::create_vm_from_js(id2.to_string(), js2.to_string()));
    let _ = api::execute_vm(id2.to_string());
    assert_eq!(api::execute_vm_func(id2.to_string(), "f".into(), 1).result_value, "\"hi3\"");
}

#[test]
fn closure_captures_only_referenced_free_vars_transitively() {
    // Free-variable capture must be transitive: `mid` does not itself use `base`,
    // but the closure it returns does — so `base` has to flow through `mid`'s
    // capture even though `mid`'s own body never names it. Also exercises that a
    // closure surrounded by many unrelated locals still resolves the ones it uses.
    let js = "
        function make(base) {
            let noise1 = 100; let noise2 = 200; let noise3 = 300;
            let mid = (k) => {
                let local = k + 1;            // uses only its param + an inner closure
                return () => base + local;    // inner closure needs `base` (grandparent)
            };
            return mid;
        }
        function f() {
            let mid = make(10);
            let inner = mid(5);   // local = 6
            return inner();       // base 10 + local 6 = 16
        }";
    assert_eq!(run_js_and_call("js-fv-transitive", js, "f"), "16");
}

#[test]
fn class_methods_and_this() {
    // A plain class: constructor sets a field, a method reads `this` and an
    // argument. `new C(...)` and a bare `C(...)` call construct identically.
    let js = "
        class Counter {
            constructor(start) { this.n = start; }
            add(k) { this.n = this.n + k; return this.n; }
            get() { return this.n; }
        }
        function f() {
            let c = new Counter(10);
            c.add(5);
            let d = Counter(100);   // construction without `new`
            d.add(1);
            return c.get() * 1000 + d.get();   // 15*1000 + 101 = 15101
        }";
    assert_eq!(run_js_and_call("js-class", js, "f"), "15101");
}

#[test]
fn class_method_calls_sibling_method_via_this() {
    let js = "
        class Math2 {
            constructor(b) { this.b = b; }
            dbl(x) { return x * 2; }
            calc(x) { return this.dbl(x) + this.b; }
        }
        function f() { let m = new Math2(7); return m.calc(10); }"; // 20 + 7 = 27
    assert_eq!(run_js_and_call("js-class-self", js, "f"), "27");
}

#[test]
fn class_method_builds_closures_in_loop_capturing_this() {
    // The SDK shape: a paint method loops, building per-iteration tap closures
    // that capture both the loop local and `this`, and stores them; calling them
    // later mutates `this`. Exercises closure capture of `this` inside a method.
    let js = "
        class Bar {
            constructor() { this.total = 0; this.taps = []; }
            build(n) {
                for (let i = 0; i < n; i++) {
                    let amount = (i + 1) * 10;
                    push(this.taps, () => { this.total = this.total + amount; });
                }
            }
            fireAll() { for (let i = 0; i < len(this.taps); i++) { this.taps[i](); } }
        }
        function f() {
            let b = new Bar();
            b.build(3);          // closures add 10, 20, 30
            b.fireAll();
            return b.total;      // 60
        }";
    assert_eq!(run_js_and_call("js-class-loop-closure", js, "f"), "60");
}

#[test]
fn class_inheritance_super_and_override() {
    // `extends` + `super(...)`: the child inherits `area`, overrides `name`, and
    // chains the parent constructor. Inherited and overridden dispatch both work.
    let js = "
        class Shape {
            constructor(w, h) { this.w = w; this.h = h; }
            area() { return this.w * this.h; }
            name() { return 1; }
        }
        class Square extends Shape {
            constructor(s) { super(s, s); this.s = s; }
            name() { return 2; }
        }
        function f() {
            let sq = new Square(5);
            return sq.area() * 10 + sq.name();   // inherited area 25 → 250 + 2 = 252
        }";
    assert_eq!(run_js_and_call("js-class-inherit", js, "f"), "252");
}

#[test]
fn class_fields_and_independent_instances() {
    // Class-field initialisers run per instance; two instances don't share state.
    let js = "
        class Box {
            tag = \"x\";
            constructor(v) { this.v = v; }
            bump() { this.v = this.v + 1; return this.v; }
        }
        function f() {
            let a = new Box(1); let b = new Box(10);
            a.bump(); a.bump();
            b.bump();
            return a.v * 100 + b.v;   // 3*100 + 11 = 311
        }";
    assert_eq!(run_js_and_call("js-class-fields", js, "f"), "311");
}
