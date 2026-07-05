//! JavaScript-spec conformance fixes in the VM, exercised through the real
//! JS → AST → bytecode → executor path. Each test pins a behaviour that the VM
//! previously got wrong (or rejected) but which standard JavaScript guarantees.

use elpian_vm::api;

/// Compile JS, run its top-level program, then call `f()` and return the result.
fn run(id: &str, js: &str) -> String {
    assert!(api::create_vm_from_js(id.to_string(), js.to_string()), "JS should compile");
    let _ = api::execute_vm(id.to_string());
    api::execute_vm_func(id.to_string(), "f".to_string(), 1).result_value
}

// ---- Call arity (flexible, like JS) ----------------------------------------

#[test]
fn calling_with_fewer_args_binds_the_rest_to_undefined() {
    // Previously the call desynced when fewer args than declared params were
    // passed; the provided args failed to bind at all.
    assert_eq!(run("arity-fewer", "function g(a, b) { return a; } function f() { return g(5); }"), "5");
}

#[test]
fn a_missing_argument_is_undefined_and_falsy() {
    let js = "function g(a, b) { if (b) { return 1; } return 2; } function f() { return g(5); }";
    assert_eq!(run("arity-undef", js), "2");
}

#[test]
fn extra_arguments_are_ignored() {
    assert_eq!(run("arity-extra", "function g(a) { return a; } function f() { return g(5, 6, 7); }"), "5");
}

// ---- Truthiness (JS coercion in conditions / `!`) --------------------------

#[test]
fn objects_and_nonzero_numbers_are_truthy() {
    assert_eq!(run("truthy-obj", "function f() { let o = { x: 1 }; if (o) { return 1; } return 0; }"), "1");
    assert_eq!(run("truthy-num", "function f() { if (7) { return 1; } return 0; }"), "1");
    assert_eq!(run("truthy-str", "function f() { if (\"hi\") { return 1; } return 0; }"), "1");
}

#[test]
fn zero_empty_string_and_null_are_falsy() {
    assert_eq!(run("falsy-zero", "function f() { if (0) { return 1; } return 0; }"), "0");
    assert_eq!(run("falsy-str", "function f() { if (\"\") { return 1; } return 0; }"), "0");
}

#[test]
fn not_operator_applies_to_any_value() {
    // `!x` used to panic on non-booleans; it now negates JS truthiness.
    assert_eq!(run("not-zero", "function f() { if (!0) { return 1; } return 0; }"), "1");
    assert_eq!(run("not-obj", "function f() { let o = { x: 1 }; if (!o) { return 1; } return 0; }"), "0");
    assert_eq!(run("not-num", "function f() { if (!5) { return 1; } return 0; }"), "0");
}

// ---- Nested / computed assignment targets ----------------------------------

#[test]
fn nested_member_assignment() {
    assert_eq!(run("asg-nested", "function f() { let o = { a: { b: 1 } }; o.a.b = 5; return o.a.b; }"), "5");
}

#[test]
fn computed_assignment_targets() {
    assert_eq!(run("asg-idxmem", "function f() { let a = [{ x: 1 }]; a[0].x = 7; return a[0].x; }"), "7");
    assert_eq!(run("asg-memidx", "function f() { let o = { a: [1, 2] }; o.a[1] = 9; return o.a[1]; }"), "9");
    assert_eq!(run("asg-deep", "function f() { let o = { a: { b: { c: 1 } } }; o.a.b.c = 42; return o.a.b.c; }"), "42");
}

#[test]
fn single_level_assignment_still_works() {
    assert_eq!(run("asg-mem", "function f() { let o = { a: 1 }; o.a = 3; return o.a; }"), "3");
    assert_eq!(run("asg-idx", "function f() { let a = [1, 2]; a[0] = 8; return a[0]; }"), "8");
}
