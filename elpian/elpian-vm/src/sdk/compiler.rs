use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use serde_json::{json, Value};

// #[wasm_bindgen]
// extern "C" {
//     #[wasm_bindgen(js_namespace = console)]
//     fn log(s: &str);

//     #[wasm_bindgen(js_namespace = console, js_name = log)]
//     fn log_u32(a: u32);

//     #[wasm_bindgen(js_namespace = console, js_name = log)]
//     fn log_many(a: &str, b: &str);
// }

fn log(s: &str) {
    println!("{}", s);
}

fn serialize_expr(val: serde_json::Value) -> Vec<u8> {
    // log(&val.to_string());
    let mut result: Vec<u8> = vec![];
    match val["type"].as_str().unwrap() {
        "i16" => {
            result.push(1);
            result.append(
                &mut i16::to_be_bytes(val["data"]["value"].as_i64().unwrap() as i16).to_vec(),
            );
        }
        "i32" => {
            result.push(2);
            result.append(
                &mut i32::to_be_bytes(val["data"]["value"].as_i64().unwrap() as i32).to_vec(),
            );
        }
        "i64" => {
            result.push(3);
            result.append(
                &mut i64::to_be_bytes(val["data"]["value"].as_i64().unwrap() as i64).to_vec(),
            );
        }
        "f32" => {
            result.push(4);
            result.append(
                &mut f32::to_be_bytes(val["data"]["value"].as_f64().unwrap() as f32).to_vec(),
            );
        }
        "f64" => {
            result.push(5);
            result.append(
                &mut f64::to_be_bytes(val["data"]["value"].as_f64().unwrap() as f64).to_vec(),
            );
        }
        "bool" => {
            result.push(6);
            result.push(if val["data"]["value"].as_bool().unwrap() {
                0x01
            } else {
                0x00
            });
        }
        "string" => {
            result.push(7);
            let mut value_bytes = val["data"]["value"].as_str().unwrap().as_bytes().to_vec();
            result.append(&mut i32::to_be_bytes(value_bytes.len() as i32).to_vec());
            result.append(&mut value_bytes);
        }
        "identifier" => {
            result.push(0x0b);
            let mut value_bytes = val["data"]["name"].as_str().unwrap().as_bytes().to_vec();
            result.append(&mut i32::to_be_bytes(value_bytes.len() as i32).to_vec());
            result.append(&mut value_bytes);
        }
        "indexer" => {
            result.push(0x0c);
            result.append(&mut serialize_expr(val["data"]["target"].clone()));
            result.append(&mut serialize_expr(val["data"]["index"].clone()));
        }
        "cast" => {
            result.push(0xfd);
            result.append(&mut serialize_expr(val["data"]["value"].clone()));
            let mut tt_bytes = val["data"]["targetType"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec();
            result.append(&mut i32::to_be_bytes(tt_bytes.len() as i32).to_vec());
            result.append(&mut tt_bytes);
        }
        "object" => {
            result.push(8);
            result.append(&mut i64::to_be_bytes(-2).to_vec());
            result.append(&mut i32::to_be_bytes(val["data"]["value"].as_object().unwrap().iter().len() as i32).to_vec());
            for (k, v) in val["data"]["value"].as_object().unwrap().iter() {
                result.push(7);
                let mut key_bytes = k.as_bytes().to_vec();
                result.append(&mut i32::to_be_bytes(key_bytes.len() as i32).to_vec());
                result.append(&mut key_bytes);
                result.append(&mut serialize_expr(v.clone()));
            }
        }
        "array" => {
            result.push(9);
            result.append(
                &mut i32::to_be_bytes(val["data"]["value"].as_array().unwrap().iter().len() as i32)
                    .to_vec(),
            );
            for v in val["data"]["value"].as_array().unwrap().iter() {
                result.append(&mut serialize_expr(v.clone()));
            }
        }
        "callback" => {
            result.append(&mut serialize_expr(val["data"]["value"]["funcId"].clone()));
        }
        "not" => {
            result.push(0xfc);
            result.append(&mut serialize_expr(val["data"]["value"].clone()));
        }
        "logical" => {
            // Short-circuit `&&` / `||`. Layout: [0xef][flag][op1][op2], where
            // `flag` is 0 for `&&` and 1 for `||`. The "skip the right operand"
            // target is recovered at decode time as a unit index (the unit just
            // past `op2`), so no byte offsets are baked here.
            let is_or = val["data"]["operation"].as_str().unwrap() == "||";
            result.push(0xef);
            result.push(if is_or { 1 } else { 0 });
            result.append(&mut serialize_expr(val["data"]["operand1"].clone()));
            result.append(&mut serialize_expr(val["data"]["operand2"].clone()));
        }
        "ternary" => {
            // `c ? a : b`. Layout: [0xee][cond][consequent][alternate]. The
            // branch boundaries are recovered as unit indices at decode time.
            result.push(0xee);
            result.append(&mut serialize_expr(val["data"]["condition"].clone()));
            result.append(&mut serialize_expr(val["data"]["consequent"].clone()));
            result.append(&mut serialize_expr(val["data"]["alternate"].clone()));
        }
        "arithmetic" => {
            match val["data"]["operation"].as_str().unwrap() {
                "==" => {
                    result.push(0xf0);
                }
                ">" => {
                    result.push(0xf1);
                }
                ">=" => {
                    result.push(0xf2);
                }
                "<" => {
                    result.push(0xf3);
                }
                "<=" => {
                    result.push(0xf4);
                }
                "!=" => {
                    result.push(0xf5);
                }
                "+" => {
                    result.push(0xf6);
                }
                "-" => {
                    result.push(0xf7);
                }
                "*" => {
                    result.push(0xf8);
                }
                "/" => {
                    result.push(0xf9);
                }
                "%" => {
                    result.push(0xfa);
                }
                "^" => {
                    result.push(0xfb);
                }
                _ => {}
            };
            result.append(&mut serialize_expr(val["data"]["operand1"].clone()));
            result.append(&mut serialize_expr(val["data"]["operand2"].clone()));
        }
        "functionCall" => {
            result.push(0x0d);
            result.append(&mut serialize_expr(val["data"]["callee"].clone()));
            result.append(
                &mut i32::to_be_bytes(val["data"]["args"].as_array().unwrap().len() as i32)
                    .to_vec(),
            );
            val["data"]["args"]
                .as_array()
                .unwrap()
                .iter()
                .for_each(|arg| {
                    result.append(&mut serialize_expr(arg.clone()));
                });
        }
        "host_call" => {
            result.push(0x0d);
            result.append(&mut serialize_expr(json!(
                {
                    "type": "identifier",
                    "data": {
                        "name": "askHost",
                    }
                }
            )));
            result.append(&mut i32::to_be_bytes(2).to_vec());
            result.append(&mut serialize_expr(json!(
                {
                    "type": "string",
                    "data": {
                        "value": val["data"]["name"].as_str().unwrap().to_string(),
                    }
                }
            )));
            let args = val["data"]["args"].as_array().unwrap().clone();
            let input = json!({
                "type": "array",
                "data": {
                    "value": args
                },
            });
            result.append(&mut serialize_expr(input.clone()));
        }
        _ => {
            panic!("unknown val type");
        }
    }
    result
}

fn serialize_condition_chain(
    operation: Value,
    is_conditioned: bool,
    start_point: usize,
) -> (Vec<u8>, Vec<usize>) {
    let mut result: Vec<u8> = vec![];
    let mut baps: Vec<usize> = vec![];
    result.push(0x10);
    if is_conditioned {
        result.push(0x01);
        result.append(&mut serialize_expr(operation["data"]["condition"].clone()).to_vec());
    } else {
        result.push(0x00);
    }
    let body_start = if is_conditioned {
        start_point + result.len() + 8 + 8 + 8 + 8
    } else {
        start_point + result.len() + 8 + 8 + 8
    };
    let body = compile_ast(operation["data"].clone(), body_start);
    let body_end = body_start + body.len();
    result.append(&mut i64::to_be_bytes(body_start as i64).to_vec());
    result.append(&mut i64::to_be_bytes(body_end as i64).to_vec());
    let mut after_body: Vec<u8> = vec![];
    if let Some(elseif_stmt) = operation["data"].get("elseifStmt") {
        let (mut compiled_body, mut branch_after_points) =
            serialize_condition_chain(elseif_stmt.clone(), true, body_end);
        after_body.append(&mut compiled_body);
        baps.append(&mut branch_after_points);
    } else if let Some(else_stmt) = operation["data"].get("elseStmt") {
        let (mut compiled_body, mut branch_after_points) =
            serialize_condition_chain(else_stmt.clone(), false, body_end);
        after_body.append(&mut compiled_body);
        baps.append(&mut branch_after_points);
    }
    if is_conditioned {
        result.append(&mut i64::to_be_bytes(body_end as i64).to_vec());
    }
    baps.push(start_point + result.len());
    result.append(&mut vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    result.append(&mut body.clone());
    result.append(&mut after_body);
    (result, baps)
}

// ---- free-variable (closure capture) analysis ------------------------------
//
// A closure only needs to snapshot the enclosing locals it actually references
// — not the whole scope chain. For each `functionDefinition` the compiler walks
// the body and computes the set of identifiers it uses that are *not* bound
// within it (its own params, `let`/`const`/`var` declarations, and nested
// function names), unioned transitively with the free variables of any nested
// closures (so an upvalue needed only by an inner closure still flows through).
// This list is serialised with the function; at runtime the executor captures
// just these names from the enclosing frames (see `capture_named`) instead of
// cloning every local — far cheaper to create a closure, and a smaller frame to
// seed on every call. Names that turn out to be globals simply aren't found in
// the enclosing scopes and resolve normally, exactly as before.

/// Identifiers bound *at this function's own level*: nested-function names and
/// `let`/`const`/`var` declarations, including those inside its `if`/`loop`/
/// `switch` blocks — but never descending into a nested function's body (that is
/// a separate scope). References to these resolve locally, so they are not free.
fn collect_bound(node: &Value, bound: &mut std::collections::BTreeSet<String>) {
    match node["type"].as_str().unwrap_or("") {
        "definition" => {
            if let Some(n) = node["data"]["leftSide"]["data"]["name"].as_str() {
                bound.insert(n.to_string());
            }
        }
        "functionDefinition" => {
            if let Some(n) = node["data"]["name"].as_str() {
                bound.insert(n.to_string());
            }
            // Do not descend: the nested function's locals are its own scope.
        }
        "ifStmt" => {
            let d = &node["data"];
            if let Some(b) = d["body"].as_array() { for s in b { collect_bound(s, bound); } }
            if d.get("elseifStmt").is_some() { collect_bound(&d["elseifStmt"], bound); }
            if let Some(e) = d.get("elseStmt") {
                if let Some(b) = e["data"]["body"].as_array() { for s in b { collect_bound(s, bound); } }
            }
        }
        "loopStmt" => {
            if let Some(b) = node["data"]["body"].as_array() { for s in b { collect_bound(s, bound); } }
        }
        "switchStmt" => {
            if let Some(cases) = node["data"]["cases"].as_array() {
                for c in cases {
                    if let Some(b) = c["body"]["body"].as_array() { for s in b { collect_bound(s, bound); } }
                }
            }
        }
        _ => {}
    }
}

/// Identifiers *referenced* in `node` (and the free variables of nested
/// closures, which must flow through this scope). A `definition`'s left side is
/// a binding, not a use; a nested `functionDefinition` contributes its own free
/// set rather than its raw identifiers.
fn collect_used(node: &Value, used: &mut std::collections::BTreeSet<String>) {
    match node["type"].as_str().unwrap_or("") {
        "identifier" => {
            if let Some(n) = node["data"]["name"].as_str() { used.insert(n.to_string()); }
        }
        "functionDefinition" => {
            let nparams = node["data"]["params"].as_array().cloned().unwrap_or_default();
            let nbody = node["data"]["body"].as_array().cloned().unwrap_or_default();
            for f in free_vars(&nparams, &nbody) { used.insert(f); }
        }
        "indexer" => {
            collect_used(&node["data"]["target"], used);
            collect_used(&node["data"]["index"], used);
        }
        "functionCall" => {
            collect_used(&node["data"]["callee"], used);
            if let Some(args) = node["data"]["args"].as_array() { for a in args { collect_used(a, used); } }
        }
        "arithmetic" | "logical" => {
            collect_used(&node["data"]["operand1"], used);
            collect_used(&node["data"]["operand2"], used);
        }
        "ternary" => {
            collect_used(&node["data"]["condition"], used);
            collect_used(&node["data"]["consequent"], used);
            collect_used(&node["data"]["alternate"], used);
        }
        "not" | "cast" => collect_used(&node["data"]["value"], used),
        "definition" => collect_used(&node["data"]["rightSide"], used),
        "assignment" => {
            collect_used(&node["data"]["leftSide"], used);
            collect_used(&node["data"]["rightSide"], used);
        }
        "returnOperation" => collect_used(&node["data"]["value"], used),
        "object" => {
            if let Some(obj) = node["data"]["value"].as_object() {
                for (_k, v) in obj { collect_used(v, used); }
            }
        }
        "array" => {
            if let Some(arr) = node["data"]["value"].as_array() {
                for v in arr { collect_used(v, used); }
            }
        }
        "ifStmt" => {
            let d = &node["data"];
            collect_used(&d["condition"], used);
            if let Some(b) = d["body"].as_array() { for s in b { collect_used(s, used); } }
            if d.get("elseifStmt").is_some() { collect_used(&d["elseifStmt"], used); }
            if let Some(e) = d.get("elseStmt") {
                if let Some(b) = e["data"]["body"].as_array() { for s in b { collect_used(s, used); } }
            }
        }
        "loopStmt" => {
            collect_used(&node["data"]["condition"], used);
            if let Some(b) = node["data"]["body"].as_array() { for s in b { collect_used(s, used); } }
        }
        "switchStmt" => {
            collect_used(&node["data"]["value"], used);
            if let Some(cases) = node["data"]["cases"].as_array() {
                for c in cases {
                    collect_used(&c["value"], used);
                    if let Some(b) = c["body"]["body"].as_array() { for s in b { collect_used(s, used); } }
                }
            }
        }
        _ => {}
    }
}

/// The free variables of a function: identifiers it (transitively) references,
/// minus everything bound at its own level (params, locals, nested-fn names).
fn free_vars(params: &[Value], body: &[Value]) -> Vec<String> {
    let mut bound: std::collections::BTreeSet<String> =
        params.iter().filter_map(|p| p.as_str().map(|s| s.to_string())).collect();
    for stmt in body { collect_bound(stmt, &mut bound); }
    let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for stmt in body { collect_used(stmt, &mut used); }
    used.into_iter().filter(|n| !bound.contains(n)).collect()
}

pub fn compile_ast(program: serde_json::Value, start_point: usize) -> Vec<u8> {
    let mut result: Vec<u8> = vec![];
    let mut op_counter: i64 = 1;
    let mut step_start_map: HashMap<i64, usize> = HashMap::new();
    let mut reserved_branch_map: HashMap<i64, Vec<usize>> = HashMap::new();
    for operation in program["body"].as_array().unwrap().iter() {
        step_start_map
            .entry(op_counter)
            .or_insert(start_point + result.len());
        match operation["type"].as_str().unwrap() {
            "jumpOperation" => {
                result.push(0x15);
                let true_branch = result.len();
                result.extend_from_slice(&[0u8; 8]);
                let true_step = operation["data"]["stepNumber"].as_i64().unwrap();
                reserved_branch_map
                    .entry(true_step)
                    .or_default()
                    .push(true_branch);
            }
            "conditionalBranch" => {
                result.push(0x16);
                result.append(&mut serialize_expr(operation["data"]["condition"].clone()));
                let true_branch = result.len();
                result.extend_from_slice(&[0u8; 8]);
                let false_branch = result.len();
                result.extend_from_slice(&[0u8; 8]);
                let true_step = operation["data"]["trueBranch"].as_i64().unwrap();
                let false_step = operation["data"]["falseBranch"].as_i64().unwrap();
                reserved_branch_map
                    .entry(true_step)
                    .or_default()
                    .push(true_branch);
                reserved_branch_map
                    .entry(false_step)
                    .or_default()
                    .push(false_branch);
            }
            "host_call" => {
                result.push(0x0d);
                result.append(&mut serialize_expr(json!(
                    {
                        "type": "identifier",
                        "data": {
                            "name": "askHost",
                        }
                    }
                )));
                result.append(&mut i32::to_be_bytes(2).to_vec());
                result.append(&mut serialize_expr(json!(
                    {
                        "type": "string",
                        "data": {
                            "value": operation["data"]["name"].as_str().unwrap().to_string(),
                        }
                    }
                )));
                let args = operation["data"]["args"].as_array().unwrap().clone();
                let input = json!({
                    "type": "array",
                    "data": {
                        "value": args
                    },
                });
                result.append(&mut serialize_expr(input.clone()));
            }
            "returnOperation" => {
                result.push(0x14);
                result.append(&mut serialize_expr(operation["data"]["value"].clone()).to_vec());
            }
            "continueStmt" => {
                result.push(0x17);
            }
            "breakStmt" => {
                result.push(0x18);
            }
            // A bare short-circuit / conditional expression statement (e.g.
            // `ready && start()`): evaluate it for its side effects; the produced
            // value is discarded like any other expression-statement result.
            "logical" | "ternary" => {
                result.append(&mut serialize_expr(operation.clone()));
            }
            "ifStmt" => {
                let (mut compiled_code, baps) =
                    serialize_condition_chain(operation.clone(), true, start_point + result.len());
                let branch_after =
                    i64::to_be_bytes((start_point + result.len() + compiled_code.len()) as i64)
                        .to_vec();
                for bap in baps.iter() {
                    let s = *bap - start_point - result.len();
                    let e = *bap + 8 - start_point - result.len();
                    compiled_code[s..e].copy_from_slice(branch_after.as_slice());
                }
                result.append(&mut compiled_code);
            }
            "loopStmt" => {
                let loop_start = start_point + result.len();
                result.push(0x11);
                result.append(&mut serialize_expr(operation["data"]["condition"].clone()).to_vec());
                let body_start = start_point + result.len() + 8 + 8 + 8;
                let mut body = compile_ast(operation["data"].clone(), body_start);
                body.push(0x15);
                body.append(&mut i64::to_be_bytes(loop_start as i64).to_vec());
                let body_end = body_start + body.len();
                result.append(&mut i64::to_be_bytes(body_start as i64).to_vec());
                result.append(&mut i64::to_be_bytes(body_end as i64).to_vec());
                result.append(&mut i64::to_be_bytes(body_end as i64).to_vec());
                result.append(&mut body.clone());
            }
            "switchStmt" => {
                result.push(0x12);
                result.append(&mut serialize_expr(operation["data"]["value"].clone()).to_vec());
                let mut inner: Vec<u8> = vec![];
                for case_val in operation["data"]["cases"].as_array().unwrap().iter() {
                    inner.append(&mut serialize_expr(case_val["value"].clone()));
                    let body_start = start_point + result.len() + 8 + 8 + inner.len() + 8 + 8;
                    let mut body: Vec<u8> = compile_ast(case_val["body"].clone(), body_start);
                    let body_end = body_start + body.len();
                    inner.append(&mut i64::to_be_bytes(body_start as i64).to_vec());
                    inner.append(&mut i64::to_be_bytes(body_end as i64).to_vec());
                    inner.append(&mut body);
                }
                result.append(
                    &mut i64::to_be_bytes(
                        (start_point + result.len() + inner.len() + 8 + 8) as i64,
                    )
                    .to_vec(),
                );
                result.append(
                    &mut i64::to_be_bytes(
                        operation["data"]["cases"].as_array().unwrap().len() as i64
                    )
                    .to_vec(),
                );
                result.append(&mut inner);
            }
            "functionDefinition" => {
                result.push(0x13);
                let mut str_bytes = operation["data"]["name"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec();
                let mut len_bytes = i32::to_be_bytes(str_bytes.len() as i32).to_vec();
                result.append(&mut len_bytes);
                result.append(&mut str_bytes);
                result.append(
                    &mut i32::to_be_bytes(
                        operation["data"]["params"].as_array().unwrap().len() as i32
                    )
                    .to_vec(),
                );
                for p_name in operation["data"]["params"].as_array().unwrap().iter() {
                    let mut str_bytes = p_name.as_str().unwrap().as_bytes().to_vec();
                    let mut len_bytes = i32::to_be_bytes(str_bytes.len() as i32).to_vec();
                    result.append(&mut len_bytes);
                    result.append(&mut str_bytes);
                }
                // Free-variable (closure capture) list: the enclosing names this
                // function references, so the runtime captures only these rather
                // than cloning the whole enclosing scope.
                let empty_params = vec![];
                let frees = free_vars(
                    operation["data"]["params"].as_array().unwrap_or(&empty_params),
                    operation["data"]["body"].as_array().unwrap_or(&empty_params),
                );
                result.append(&mut i32::to_be_bytes(frees.len() as i32).to_vec());
                for f in frees.iter() {
                    let mut str_bytes = f.as_bytes().to_vec();
                    result.append(&mut i32::to_be_bytes(str_bytes.len() as i32).to_vec());
                    result.append(&mut str_bytes);
                }
                let func_start = start_point + result.len() + 8 + 8;
                let body = compile_ast(operation["data"].clone(), func_start);
                let func_end = func_start + body.len();
                result.append(&mut i64::to_be_bytes(func_start as i64).to_vec());
                result.append(&mut i64::to_be_bytes(func_end as i64).to_vec());
                result.append(&mut body.clone());
            }
            "functionCall" => {
                result.push(0x0d);
                result.append(&mut serialize_expr(operation["data"]["callee"].clone()));
                result.append(
                    &mut i32::to_be_bytes(
                        operation["data"]["args"].as_array().unwrap().len() as i32
                    )
                    .to_vec(),
                );
                operation["data"]["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .for_each(|arg| {
                        result.append(&mut serialize_expr(arg.clone()));
                    });
            }
            "definition" => {
                result.push(0x0e);
                if operation["data"]["leftSide"]["type"].as_str().unwrap() == "identifier" {
                    result.push(0x0b);
                    let mut str_bytes = operation["data"]["leftSide"]["data"]["name"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec();
                    let mut len_bytes = i32::to_be_bytes(str_bytes.len() as i32).to_vec();
                    result.append(&mut len_bytes);
                    result.append(&mut str_bytes);
                    result.append(&mut serialize_expr(operation["data"]["rightSide"].clone()));
                }
            }
            "assignment" => {
                result.push(0x0f);
                if operation["data"]["leftSide"]["type"].as_str().unwrap() == "identifier" {
                    result.push(0x0b);
                    let mut str_bytes = operation["data"]["leftSide"]["data"]["name"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec();
                    let mut len_bytes = i32::to_be_bytes(str_bytes.len() as i32).to_vec();
                    result.append(&mut len_bytes);
                    result.append(&mut str_bytes);
                    result.append(&mut serialize_expr(operation["data"]["rightSide"].clone()));
                } else if operation["data"]["leftSide"]["type"].as_str().unwrap() == "indexer" {
                    result.push(0x0c);
                    let mut str_bytes = operation["data"]["leftSide"]["data"]["target"]["data"]
                        ["name"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec();
                    let mut len_bytes = i32::to_be_bytes(str_bytes.len() as i32).to_vec();
                    result.append(&mut len_bytes);
                    result.append(&mut str_bytes);
                    // The executor reads the index expression *before* the value
                    // (AssignVarExtractName → AssignVarExtractIndex → ...Value), so
                    // `a[i] = v` / `a.b = v` must serialize the index here. Without
                    // it the operands desync and the index stays unset.
                    result.append(&mut serialize_expr(
                        operation["data"]["leftSide"]["data"]["index"].clone(),
                    ));
                    result.append(&mut serialize_expr(operation["data"]["rightSide"].clone()));
                }
            }
            _ => {
                // skip
            }
        }
        op_counter += 1;
    }
    for (key, value) in reserved_branch_map {
        let step_point = *step_start_map.get(&key).unwrap();
        let sp_bytes = i64::to_be_bytes(step_point as i64).to_vec();
        for space in value.iter() {
            let address: usize = *space;
            result[address..address + 8].copy_from_slice(sp_bytes.as_slice());
        }
    }
    if result.is_empty() {
        result.push(0x00);
    }
    result
}

pub fn parse_code(program: String) -> serde_json::Value {
    let temp_prog = program.clone();
    let mut tokens: Vec<String> = vec![];
    let mut temp_token = "".to_string();
    let mut inside_string = false;
    for c in temp_prog.chars() {
        if c == '"' {
            if inside_string {
                inside_string = false;
                temp_token.push(c);
                tokens.push(temp_token);
                temp_token = "".to_string();
            } else {
                inside_string = true;
                temp_token.push(c);
            }
            continue;
        }
        let c_stred: &str = &c.to_string();
        if c == ' ' || c == '\n' || c == '\t' {
            if temp_token.len() > 0 {
                tokens.push(temp_token);
                temp_token = "".to_string();
            }
            continue;
        } else if vec![
            "=", "+", "-", "*", "/", "^", "%", "==", ">", "<", ">=", "<=", "!=", ".", "(", ")",
            "[", "]", "{", "}", ":", ",",
        ]
        .contains(&c_stred)
        {
            if temp_token.len() > 0 {
                tokens.push(temp_token);
                temp_token = "".to_string();
            }
            tokens.push(c.to_string());
            continue;
        }
        temp_token.push(c);
    }
    if temp_token.len() > 0 {
        tokens.push(temp_token);
    }
    // log(&format!("{:?}", tokens));
    let mut result = json!({});
    let mut state_num = 0;
    let mut stack: Vec<HashMap<String, Value>> = vec![];
    let mut first_stage: HashMap<String, Value> = HashMap::new();
    first_stage.insert("body".to_string(), json!([]));
    first_stage.insert("type".to_string(), json!("program".to_string()));
    stack.push(first_stage);
    let mut p: usize = 0;
    let mut current_reg: Value = json!(0);
    let mut counter = 0;
    let mut reserved_identifier = "".to_string();
    loop {
        counter += 1;
        // log(&p.to_string());
        // log(&state_num.to_string());
        // log(&format!("{:?}", stack));
        if counter > 50 {
            break;
        }
        if stack.len() == 0 && p >= tokens.len() {
            break;
        }
        if p >= tokens.len() {
            if state_num == 0 {
                result["type"] = json!("program");
                result["body"] = stack.last().unwrap().get("body").unwrap().clone();
                stack.pop();
                continue;
            } else if state_num == 101 {
                if current_reg
                    .get("type")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
                    == "functionCall"
                {
                    stack
                        .last_mut()
                        .unwrap()
                        .get_mut("body")
                        .unwrap()
                        .as_array_mut()
                        .unwrap()
                        .push(current_reg.clone());
                    state_num = 0;
                    continue;
                }
                let last_stage = stack.last().unwrap().clone();
                stack.pop();
                let last_type = last_stage["type"].as_str().unwrap().to_string();
                if last_type == "arithmetic" {
                    current_reg = json!({
                        "type": "arithmetic",
                        "data": {
                            "operation": last_stage.get("operation").unwrap().clone(),
                            "operand1": last_stage.get("operand1").unwrap().clone(),
                            "operand2": current_reg
                        }
                    });
                } else if last_type == "definition" {
                    stack.last_mut().unwrap().get_mut("body").unwrap().as_array_mut().unwrap().push(json!({
                        "type": "definition",
                        "data": {
                            "leftSide": {
                                "type": "identifier",
                                "data": {
                                    "name": last_stage.get("leftSide").unwrap().as_str().unwrap().to_string()
                                }
                            },
                            "rightSide": current_reg
                        }
                    }));
                    state_num = 0;
                } else if last_type == "assignment" {
                    stack.last_mut().unwrap().get_mut("body").unwrap().as_array_mut().unwrap().push(json!({
                        "type": "assignment",
                        "data": {
                            "leftSide": {
                                "type": "identifier",
                                "data": {
                                    "name": last_stage.get("leftSide").unwrap().as_str().unwrap().to_string()
                                }
                            },
                            "rightSide": current_reg
                        }
                    }));
                    state_num = 0;
                }
                continue;
            }
        }
        let token = tokens[p].clone();
        if state_num == 0 {
            if token == "def" {
                p += 1;
                state_num = 1;
                stack.push(HashMap::new());
                stack
                    .last_mut()
                    .unwrap()
                    .insert("type".to_string(), json!("definition"));
                continue;
            } else {
                p += 1;
                reserved_identifier = token.clone();
                state_num = 3;
            }
        } else if state_num == 1 {
            p += 1;
            stack
                .last_mut()
                .unwrap()
                .insert("leftSide".to_string(), json!(token.clone()));
            state_num = 2;
            continue;
        } else if state_num == 2 {
            if token == "=" {
                p += 1;
                state_num = 100;
                continue;
            }
        } else if state_num == 3 {
            if token == "=" {
                p += 1;
                stack.push(HashMap::new());
                stack
                    .last_mut()
                    .unwrap()
                    .insert("type".to_string(), json!("assignment"));
                stack
                    .last_mut()
                    .unwrap()
                    .insert("leftSide".to_string(), json!(reserved_identifier.clone()));
                reserved_identifier = "".to_string();
                state_num = 100;
                continue;
            } else if token == "(" {
                p += 1;
                stack.push(HashMap::new());
                stack
                    .last_mut()
                    .unwrap()
                    .insert("type".to_string(), json!("functionCall"));
                stack.last_mut().unwrap().insert(
                    "callee".to_string(),
                    json!({
                        "type": "identifier",
                        "data": {
                            "name": reserved_identifier.clone(),
                        }
                    }),
                );
                stack
                    .last_mut()
                    .unwrap()
                    .insert("args".to_string(), json!(vec![] as Vec<Value>));
                reserved_identifier = "".to_string();
                state_num = 100;
                continue;
            }
        } else if state_num == 100 {
            if token == "{" {
                stack.push(HashMap::new());
                stack
                    .last_mut()
                    .unwrap()
                    .insert("objectData".to_string(), json!({}));
                stack
                    .last_mut()
                    .unwrap()
                    .insert("type".to_string(), json!("objectExpr"));
                p += 1;
                state_num = 102;
                continue;
            }
            if token == "(" {
                stack.push(HashMap::new());
                stack
                    .last_mut()
                    .unwrap()
                    .insert("type".to_string(), json!("paren"));
                p += 1;
                continue;
            }
            let parse_res_i16 = token.parse::<i16>();
            if parse_res_i16.is_ok() {
                current_reg = json!({
                    "type": "i16",
                    "data": { "value": parse_res_i16.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            let parse_res_i32 = token.parse::<i32>();
            if parse_res_i32.is_ok() {
                current_reg = json!({
                    "type": "i32",
                    "data": { "value": parse_res_i32.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            let parse_res_i64 = token.parse::<i64>();
            if parse_res_i64.is_ok() {
                current_reg = json!({
                    "type": "i64",
                    "data": { "value": parse_res_i64.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            let parse_res_f32 = token.parse::<f32>();
            if parse_res_f32.is_ok() {
                current_reg = json!({
                    "type": "f32",
                    "data": { "value": parse_res_f32.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            let parse_res_f64 = token.parse::<f64>();
            if parse_res_f64.is_ok() {
                current_reg = json!({
                    "type": "f64",
                    "data": { "value": parse_res_f64.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            let parse_res_bool = token.parse::<bool>();
            if parse_res_bool.is_ok() {
                current_reg = json!({
                    "type": "bool",
                    "data": { "value": parse_res_bool.unwrap() }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
                current_reg = json!({
                    "type": "string",
                    "data": { "value": token[1..token.len()-1] }
                });
                p += 1;
                state_num = 101;
                continue;
            }
            current_reg = json!({
                "type": "identifier",
                "data": { "name": token }
            });
            p += 1;
            state_num = 101;
            continue;
        } else if state_num == 101 {
            if stack.last().unwrap().get("type").unwrap() == "objectExpr"
                && stack.last().unwrap().contains_key("currentKey")
            {
                let key = stack
                    .last_mut()
                    .unwrap()
                    .remove("currentKey")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string();
                stack
                    .last_mut()
                    .unwrap()
                    .get_mut("objectData")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert(key, current_reg.clone());
                state_num = 103;
                continue;
            } else if stack
                .last()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
                == "arithmetic"
            {
                let last_stage = stack.last().unwrap().clone();
                stack.pop();
                current_reg = json!({
                    "type": "arithmetic",
                    "data": {
                        "operation": last_stage.get("operation").unwrap().clone(),
                        "operand1": last_stage.get("operand1").unwrap().clone(),
                        "operand2": current_reg
                    }
                });
                continue;
            } else if stack
                .last()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
                == "definition"
            {
                let last_stage = stack.last().unwrap().clone();
                stack.pop();
                stack.last_mut().unwrap().get_mut("body").unwrap().as_array_mut().unwrap().push(json!({
                        "type": "definition",
                        "data": {
                            "leftSide": {
                                "type": "identifier",
                                "data": {
                                    "name": last_stage.get("leftSide").unwrap().as_str().unwrap().to_string()
                                }
                            },
                            "rightSide": current_reg
                        }
                    }));
                state_num = 0;
                continue;
            } else if stack
                .last()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
                == "assignment"
            {
                let last_stage = stack.last().unwrap().clone();
                stack.pop();
                stack.last_mut().unwrap().get_mut("body").unwrap().as_array_mut().unwrap().push(json!({
                        "type": "assignment",
                        "data": {
                            "leftSide": {
                                "type": "identifier",
                                "data": {
                                    "name": last_stage.get("leftSide").unwrap().as_str().unwrap().to_string()
                                }
                            },
                            "rightSide": current_reg
                        }
                    }));
                state_num = 0;
                continue;
            } else {
                if token == "}" {
                    p += 1;
                    if stack
                        .last()
                        .unwrap()
                        .get("type")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                        == "objPropValue"
                    {
                        stack.pop();
                        let last_stage = stack.last_mut().unwrap();
                        let ck = last_stage
                            .get("currentKey")
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_string();
                        last_stage
                            .get_mut("objectData")
                            .unwrap()
                            .as_object_mut()
                            .unwrap()
                            .insert(ck, current_reg.clone());
                    }
                    let last_stage = stack.last().unwrap().clone();
                    stack.pop();
                    if last_stage
                        .get("type")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                        == "objectExpr"
                    {
                        current_reg = json!({
                            "type": "object",
                            "data": {
                                "value": last_stage.get("objectData").unwrap().clone(),
                            }
                        });
                    }
                    continue;
                } else if token == ")" {
                    if stack
                        .last()
                        .unwrap()
                        .get("type")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                        == "paren"
                    {
                        p += 1;
                        stack.pop();
                        continue;
                    } else if stack
                        .last()
                        .unwrap()
                        .get("type")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                        == "functionCall"
                    {
                        p += 1;
                        let mut last_sage = stack.pop().unwrap();
                        last_sage
                            .get_mut("args")
                            .unwrap()
                            .as_array_mut()
                            .unwrap()
                            .push(current_reg.clone());
                        current_reg = json!({
                            "type": "functionCall",
                            "data": {
                                "callee": last_sage.get("callee").unwrap().clone(),
                                "args": last_sage.get("args").unwrap().clone(),
                            }
                        });
                        continue;
                    }
                } else if vec!["+", "-", "/", "*", "^", "%"]
                    .iter()
                    .any(|op| op.to_string() == token)
                {
                    stack.push(HashMap::new());
                    stack
                        .last_mut()
                        .unwrap()
                        .insert("type".to_string(), json!("arithmetic"));
                    stack
                        .last_mut()
                        .unwrap()
                        .insert("operand1".to_string(), current_reg.clone());
                    stack
                        .last_mut()
                        .unwrap()
                        .insert("operation".to_string(), json!(token.clone()));
                    p += 1;
                    state_num = 100;
                    continue;
                } else if token == "," {
                    if stack
                        .last()
                        .unwrap()
                        .get("type")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                        == "functionCall"
                    {
                        p += 1;
                        stack
                            .last_mut()
                            .unwrap()
                            .get_mut("args")
                            .unwrap()
                            .as_array_mut()
                            .unwrap()
                            .push(current_reg.clone());
                        state_num = 100;
                        continue;
                    }
                }
            }
            if !stack.last().unwrap().get("body").is_none() {
                if current_reg
                    .get("type")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
                    == "functionCall"
                {
                    stack
                        .last_mut()
                        .unwrap()
                        .get_mut("body")
                        .unwrap()
                        .as_array_mut()
                        .unwrap()
                        .push(current_reg.clone());
                    current_reg = json!({});
                }
                state_num = 0;
                continue;
            }
        } else if state_num == 102 {
            stack.last_mut().unwrap().insert(
                "currentKey".to_string(),
                json!(token[1..token.len() - 1].to_string()),
            );
            stack.push(HashMap::new());
            stack
                .last_mut()
                .unwrap()
                .insert("type".to_string(), json!("objPropValue".to_string()));
            p += 1;
            state_num = 104;
            continue;
        } else if state_num == 103 {
            if token == "," {
                state_num = 102;
                p += 1;
                continue;
            } else if token == "}" {
                state_num = 101;
                continue;
            }
        } else if state_num == 104 {
            if token == ":" {
                p += 1;
                state_num = 100;
            }
        }
    }
    result
}

#[derive(Clone, Debug)]
struct Path {
    id: i32,
    prefix: String,
    nexts: Vec<Rc<RefCell<Path>>>,
}

pub fn compile_code(p: String) -> Vec<u8> {
    let program = p;

    let temp_prog = program;
    let mut tokens: Vec<String> = vec![];
    let mut temp_token = "".to_string();
    let mut inside_string = false;
    for c in temp_prog.chars() {
        if c == '"' {
            if inside_string {
                inside_string = false;
                temp_token.push(c);
                tokens.push(temp_token);
                temp_token = "".to_string();
            } else {
                inside_string = true;
                temp_token.push(c);
            }
            continue;
        }
        if inside_string {
            temp_token.push(c);
            continue;
        }
        let c_stred: &str = &c.to_string();
        if c == ' ' || c == '\n' || c == '\t' {
            if temp_token.len() > 0 {
                tokens.push(temp_token);
                temp_token = "".to_string();
            }
            continue;
        } else if vec![
            "=", "+", "-", "*", "/", "^", "%", "==", ">", "<", ">=", "<=", "!=", ".", "(", ")",
            "[", "]", "{", "}", ":", ",",
        ]
        .contains(&c_stred)
        {
            if temp_token.len() > 0 {
                tokens.push(temp_token);
                temp_token = "".to_string();
            }
            tokens.push(c.to_string());
            continue;
        }
        temp_token.push(c);
    }
    if temp_token.len() > 0 {
        tokens.push(temp_token);
    }
    log(&format!("{:?}", tokens));

    let mut stack: Vec<(String, Path, i32, usize, i32)> = vec![];

    let start_path = Rc::new(RefCell::new(Path {
        id: 1,
        prefix: "start".to_string(),
        nexts: vec![],
    }));
    let end_path = Rc::new(RefCell::new(Path {
        id: 2,
        prefix: "end".to_string(),
        nexts: vec![],
    }));
    {
        start_path.borrow_mut().nexts.push(end_path.clone());
    }
    let expr_path = Rc::new(RefCell::new(Path {
        id: 3,
        prefix: "".to_string(),
        nexts: vec![],
    }));
    {
        start_path.borrow_mut().nexts.push(expr_path.clone());
    }
    let expr_2_path = Rc::new(RefCell::new(Path {
        id: 4,
        prefix: "string".to_string(),
        nexts: vec![],
    }));
    {
        expr_path.borrow_mut().nexts.push(expr_2_path.clone());
    }
    {
        expr_path.borrow_mut().nexts.push(end_path.clone());
    }
    let expr_3_path = Rc::new(RefCell::new(Path {
        id: 5,
        prefix: "+".to_string(),
        nexts: vec![],
    }));
    {
        expr_2_path.borrow_mut().nexts.push(expr_3_path.clone());
    }
    let expr_4_path = Rc::new(RefCell::new(Path {
        id: 6,
        prefix: "string".to_string(),
        nexts: vec![],
    }));
    {
        expr_3_path.borrow_mut().nexts.push(expr_4_path.clone());
    }
    {
        expr_4_path.borrow_mut().nexts.push(expr_3_path.clone());
    }
    {
        expr_4_path.borrow_mut().nexts.push(end_path.clone());
    }

    let function_call_path = Rc::new(RefCell::new(Path {
        id: 7,
        prefix: "id".to_string(),
        nexts: vec![],
    }));
    {
        start_path
            .borrow_mut()
            .nexts
            .push(function_call_path.clone());
    }
    let function_call_2_path = Rc::new(RefCell::new(Path {
        id: 8,
        prefix: "(".to_string(),
        nexts: vec![],
    }));
    {
        function_call_path
            .borrow_mut()
            .nexts
            .push(function_call_2_path.clone());
    }
    {
        function_call_2_path
            .borrow_mut()
            .nexts
            .push(expr_path.clone());
    }
    let function_call_4_path = Rc::new(RefCell::new(Path {
        id: 10,
        prefix: ")".to_string(),
        nexts: vec![],
    }));
    {
        expr_4_path
            .borrow_mut()
            .nexts
            .push(function_call_4_path.clone());
        expr_2_path
            .borrow_mut()
            .nexts
            .push(function_call_4_path.clone());
        function_call_4_path
            .borrow_mut()
            .nexts
            .push(end_path.clone());
    }

    let genesis_path = Rc::new(RefCell::new(Path {
        id: 11,
        prefix: "".to_string(),
        nexts: vec![start_path.clone()],
    }));

    stack.push(("".to_string(), genesis_path.borrow_mut().clone(), 0, 0, 0));

    let mut keyword_map: HashMap<String, bool> = HashMap::new();
    keyword_map.insert("start".to_string(), true);
    keyword_map.insert("end".to_string(), true);
    keyword_map.insert("(".to_string(), true);
    keyword_map.insert(")".to_string(), true);
    keyword_map.insert("+".to_string(), true);

    loop {
        let mut found = false;
        let paths = stack.last().unwrap().1.nexts.clone();
        let checkpoint = stack.last().unwrap().2;
        let mut counter = 0;
        let curr_token = tokens[stack.last().unwrap().4 as usize].clone();
        for pa in paths.iter() {
            if counter < checkpoint {
                counter += 1;
                continue;
            }
            let path = pa.borrow().clone();
            if path.prefix == "" {
                let mut prev_exists = false;
                for hist in stack.clone().into_iter().rev() {
                    if hist.1.id == path.id && hist.3 == stack.len() {
                        prev_exists = true;
                        break;
                    }
                }
                if prev_exists {
                    counter += 1;
                    continue;
                }
                println!("trying non-prefix {}", curr_token);
                counter += 1;
                stack.last_mut().unwrap().2 = counter;
                found = true;
                stack.push((
                    curr_token,
                    path.clone(),
                    0,
                    stack.len(),
                    stack.last().unwrap().4,
                ));
                break;
            } else if !keyword_map.contains_key(&curr_token) {
                if curr_token.starts_with("\"")
                    && curr_token.ends_with("\"")
                    && path.prefix == "string"
                {
                    println!("matched string {}", curr_token);
                    counter += 1;
                    stack.last_mut().unwrap().2 = counter;
                    found = true;
                    stack.push((
                        curr_token,
                        path.clone(),
                        0,
                        stack.len(),
                        stack.last().unwrap().4 + 1,
                    ));
                    break;
                } else if path.prefix == "id" {
                    println!("matched identifier {}", curr_token);
                    counter += 1;
                    stack.last_mut().unwrap().2 = counter;
                    found = true;
                    stack.push((
                        curr_token,
                        path.clone(),
                        0,
                        stack.len(),
                        stack.last().unwrap().4 + 1,
                    ));
                    break;
                }
            } else if path.prefix == curr_token {
                println!("matched {}", curr_token);
                counter += 1;
                stack.last_mut().unwrap().2 = counter;
                found = true;
                stack.push((
                    curr_token,
                    path.clone(),
                    0,
                    stack.len(),
                    stack.last().unwrap().4 + 1,
                ));
                break;
            }
            counter += 1;
        }
        if stack.last().unwrap().0 == "end" {
            println!("Finished !");
            break;
        }
        if !found {
            if stack.len() > 0 {
                stack.pop();
            }
        }
        if stack.len() == 0 {
            break;
        }
    }

    vec![]
}

// ============================================================================
// JavaScript front-end
//
// `parse_js` turns a practical subset of JavaScript source into the very same
// Elpian AST JSON that the hand-written test helpers and external front-ends
// emit (see the node shapes consumed by `compile_ast` / `serialize_expr`
// above). It is intentionally self-contained — a tokenizer plus a
// recursive-descent / precedence-climbing parser — so the VM can build an Elpa
// instance straight from JS code without an off-VM toolchain.
//
// The pipeline mirrors the AST path exactly:
//
//     JS source ──parse_js──▶ Elpian AST JSON ──compile_ast──▶ bytecode
//
// i.e. JS is first lowered to the documented AST and then handed to the same
// `from ast` compiler that every other entry point uses.
//
// Supported subset (everything the AST/bytecode actually models):
//   * `let` / `const` / `var` declarations (→ `definition`).
//   * assignment, including `+= -= *= /= %=` and `++` / `--` (→ `assignment`).
//     A simple target (`x`, `a.b`, `a[i]`) uses the native `assignment`; a nested
//     or computed target (`a.b.c`, `a[i].x`, `o.a[i]`) is lowered to a
//     `__setIndex(base, key, value)` builtin call, so deep assignment works.
//   * `function name(params) { ... }` (→ `functionDefinition`).
//   * `return` (→ `returnOperation`).
//   * `if` / `else if` / `else` (→ `ifStmt` chains).
//   * `while` and C-style `for` loops (→ `loopStmt`; `for` is desugared into an
//     init prefix plus a `loopStmt` whose body carries the update step).
//   * `switch` / `case` (→ `switchStmt`; `default` and `break` are accepted but
//     not modelled by the bytecode, so they are dropped).
//   * expressions: numbers, strings, booleans, identifiers, arrays, objects,
//     member access (`a.b` / `a[i]` → `indexer`), calls (→ `functionCall`),
//     the arithmetic/comparison operators the VM understands
//     (`+ - * / % ** == === != !== < <= > >=`, with `**`→`^`,
//     `===`→`==`, `!==`→`!=`) and the `!` / unary `-` prefixes.
//   * `class` declarations with a `constructor`, instance methods, class-field
//     initialisers, single inheritance (`extends`) and `super(...)` constructor
//     chaining; `new C(...)` and a bare `C(...)` both construct. Lowered to a
//     factory function whose methods are closures over a `this` object — no new
//     opcode (see `parse_class`). `this` is an ordinary lexical local.
//   * arrow functions and `function` *expressions* (anonymous closures):
//     `x => e`, `(a, b) => e`, `() => { ... }`, `function (a) { ... }`. The VM
//     has no function-literal expression opcode — a function value only enters
//     scope via the `functionDefinition` *statement* (which captures the
//     enclosing locals as the closure's environment). So each arrow / function
//     expression is **desugared**: it is lifted into a synthetic, uniquely-named
//     `functionDefinition` hoisted just before the statement that uses it, and
//     the expression site is replaced by an `identifier` referencing that name.
//     A concise body `=> e` becomes `{ return e; }`. The lifted definition runs
//     in place, so it closes over exactly the locals lexically in scope there —
//     real per-call closures (e.g. a fresh `let` per loop iteration is captured
//     independently). The VM already supports calling such a value held in any
//     variable or object field (e.g. a widget's `onTap`).

#[derive(Clone, Debug, PartialEq)]
enum JsTok {
    Num(String),
    Str(String),
    Ident(String),
    Punct(String),
    Eof,
}

fn tokenize_js(src: &str) -> Vec<JsTok> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    let mut toks: Vec<JsTok> = vec![];
    // Longest punctuators first so the greedy scan never splits `===` into
    // `==` + `=`, `<=` into `<` + `=`, and so on.
    let puncts: &[&str] = &[
        "===", "!==", "**", "==", "!=", "<=", ">=", "=>", "&&", "||", "++", "--", "+=", "-=", "*=",
        "/=", "%=", "(", ")", "{", "}", "[", "]", ";", ",", ".", ":", "?", "<", ">", "=", "+", "-",
        "*", "/", "%", "!", "^", "&", "|",
    ];
    while i < n {
        let c = chars[i];
        if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            i += 1;
            continue;
        }
        // Comments.
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            i += 2;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // String literals (single or double quoted) with the common escapes.
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            while i < n && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < n {
                    match chars[i + 1] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"' => s.push('"'),
                        '0' => s.push('\0'),
                        other => s.push(other),
                    }
                    i += 2;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            i += 1; // closing quote
            toks.push(JsTok::Str(s));
            continue;
        }
        // Numeric literals (integer, fractional, exponent).
        if c.is_ascii_digit() {
            let start = i;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i < n && chars[i] == '.' {
                i += 1;
                while i < n && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                i += 1;
                if i < n && (chars[i] == '+' || chars[i] == '-') {
                    i += 1;
                }
                while i < n && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            toks.push(JsTok::Num(chars[start..i].iter().collect()));
            continue;
        }
        // Identifiers and keywords.
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            toks.push(JsTok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        // Punctuators, greedily matching the longest spelling.
        let mut matched = false;
        for p in puncts.iter() {
            let pl = p.chars().count();
            if i + pl <= n {
                let slice: String = chars[i..i + pl].iter().collect();
                if &slice == p {
                    toks.push(JsTok::Punct((*p).to_string()));
                    i += pl;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            // Unknown character: skip it rather than abort the whole parse.
            i += 1;
        }
    }
    toks.push(JsTok::Eof);
    toks
}

// ---- AST node builders (exact shapes consumed by `compile_ast`) -------------

fn js_num_literal(s: &str) -> Value {
    if s.contains('.') || s.contains('e') || s.contains('E') {
        json!({ "type": "f64", "data": { "value": s.parse::<f64>().unwrap_or(0.0) } })
    } else {
        match s.parse::<i64>() {
            Ok(v) => json!({ "type": "i64", "data": { "value": v } }),
            Err(_) => json!({ "type": "f64", "data": { "value": s.parse::<f64>().unwrap_or(0.0) } }),
        }
    }
}
fn js_int(n: i64) -> Value {
    json!({ "type": "i64", "data": { "value": n } })
}
fn js_ident(name: &str) -> Value {
    json!({ "type": "identifier", "data": { "name": name } })
}
fn js_string(s: &str) -> Value {
    json!({ "type": "string", "data": { "value": s } })
}
fn js_arith(op: &str, a: Value, b: Value) -> Value {
    json!({ "type": "arithmetic", "data": { "operation": op, "operand1": a, "operand2": b } })
}
fn js_def(name: &str, val: Value) -> Value {
    json!({ "type": "definition", "data": { "leftSide": js_ident(name), "rightSide": val } })
}
/// Build an assignment for any JS lvalue. A bare identifier or a *direct* indexer
/// (`a.b` / `a[i]`, whose base is a named variable) uses the native `assignment`
/// node the bytecode models. A *nested or computed* base (`a.b.c`, `a[i].x`,
/// `o.a[i]`) — where the base is itself an expression — is lowered to a call to
/// the `__setIndex` builtin: the base expression evaluates to a container
/// reference and the builtin stores into it. This keeps deep assignment working
/// without the lvalue having to be a single named variable. Anything else yields
/// `None`, so the caller can drop the meaningless statement.
fn js_assign(target: Value, rhs: Value) -> Option<Value> {
    match target["type"].as_str().unwrap_or("") {
        "identifier" => {
            Some(json!({ "type": "assignment", "data": { "leftSide": target, "rightSide": rhs } }))
        }
        "indexer" => {
            if target["data"]["target"]["type"] == "identifier" {
                Some(json!({ "type": "assignment", "data": { "leftSide": target, "rightSide": rhs } }))
            } else {
                let base = target["data"]["target"].clone();
                let index = target["data"]["index"].clone();
                Some(json!({ "type": "functionCall", "data": {
                    "callee": js_ident("__setIndex"),
                    "args": [base, index, rhs]
                } }))
            }
        }
        _ => None,
    }
}
/// Fold a unary minus into the literal where possible, else lower to `0 - x`.
fn js_negate(v: Value) -> Value {
    if v["type"] == "i64" {
        if let Some(n) = v["data"]["value"].as_i64() {
            return js_int(-n);
        }
    }
    if v["type"] == "f64" {
        if let Some(n) = v["data"]["value"].as_f64() {
            return json!({ "type": "f64", "data": { "value": -n } });
        }
    }
    js_arith("-", js_int(0), v)
}
/// Short-circuiting `&&` / `||`. Modelled as a dedicated node (not an arithmetic
/// op) so the bytecode can evaluate the right operand lazily — `a && b` only
/// touches `b` when `a` is truthy, `a || b` only when `a` is falsy — exactly as
/// JavaScript requires (and as guard idioms like `obj && obj.x` depend on).
fn js_logical(op: &str, a: Value, b: Value) -> Value {
    json!({ "type": "logical", "data": { "operation": op, "operand1": a, "operand2": b } })
}
/// The conditional (ternary) operator `c ? a : b`. Like `&&`/`||` it is lazy:
/// only the taken branch is evaluated.
fn js_ternary(c: Value, a: Value, b: Value) -> Value {
    json!({ "type": "ternary", "data": { "condition": c, "consequent": a, "alternate": b } })
}

struct JsParser {
    toks: Vec<JsTok>,
    pos: usize,
    /// Synthetic `functionDefinition` nodes produced by desugaring arrow /
    /// function expressions, awaiting hoisting in front of the statement
    /// currently being parsed (drained by [`JsParser::parse_statement`]).
    lifted: Vec<Value>,
    /// Counter for unique synthetic closure names (`__anon_N`).
    anon_counter: usize,
    /// While parsing a class method body, the parent class name (if the class
    /// `extends` one) so `super.m(...)` can resolve to the parent's method.
    class_parent: Option<String>,
    /// Class names that declared at least one `static` member, with the per-class
    /// holder object `__static_<Name>`. A `C.member` access where `C` is such a
    /// class is rewritten to read off that holder (see [`JsParser::parse_postfix`]).
    class_statics: HashSet<String>,
    /// The constructor parameter list recorded for each class, so a derived class
    /// with no explicit constructor can synthesise one that forwards those args to
    /// `super` (JS's implicit-constructor behaviour).
    class_ctor_params: HashMap<String, Vec<String>>,
}

impl JsParser {
    fn new(toks: Vec<JsTok>) -> Self {
        JsParser {
            toks,
            pos: 0,
            lifted: Vec::new(),
            anon_counter: 0,
            class_parent: None,
            class_statics: HashSet::new(),
            class_ctor_params: HashMap::new(),
        }
    }
    fn peek(&self) -> &JsTok {
        &self.toks[self.pos]
    }
    fn advance(&mut self) -> JsTok {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek(), JsTok::Eof)
    }
    fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), JsTok::Punct(s) if s == p)
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, p: &str) {
        if !self.eat_punct(p) {
            panic!("js: expected '{}', found {:?}", p, self.peek());
        }
    }
    fn at_ident(&self, name: &str) -> bool {
        matches!(self.peek(), JsTok::Ident(s) if s == name)
    }
    fn eat_ident(&mut self, name: &str) -> bool {
        if self.at_ident(name) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn expect_ident(&mut self, name: &str) {
        if !self.eat_ident(name) {
            panic!("js: expected keyword '{}', found {:?}", name, self.peek());
        }
    }
    fn expect_ident_name(&mut self) -> String {
        match self.advance() {
            JsTok::Ident(s) => s,
            t => panic!("js: expected identifier, found {:?}", t),
        }
    }

    fn parse_program(&mut self) -> Value {
        let mut body: Vec<Value> = vec![];
        while !self.at_eof() {
            body.extend(self.parse_statement());
        }
        json!({ "type": "program", "body": body })
    }

    // ---- Statements ---------------------------------------------------------

    /// Parse one statement, then hoist any arrow / function expressions it
    /// desugared into synthetic `functionDefinition`s *in front of* it, so each
    /// closure is defined (and captures its environment) right where it appears.
    fn parse_statement(&mut self) -> Vec<Value> {
        let mark = self.lifted.len();
        let mut stmts = self.parse_statement_inner();
        if self.lifted.len() > mark {
            let mut hoisted: Vec<Value> = self.lifted.split_off(mark);
            hoisted.append(&mut stmts);
            return hoisted;
        }
        stmts
    }

    fn parse_statement_inner(&mut self) -> Vec<Value> {
        if self.eat_punct(";") {
            return vec![];
        }
        if self.at_ident("function") {
            return vec![self.parse_function_decl()];
        }
        if self.at_ident("class") {
            return self.parse_class();
        }
        if self.at_ident("if") {
            return vec![self.parse_if()];
        }
        if self.at_ident("while") {
            return vec![self.parse_while()];
        }
        if self.at_ident("for") {
            return self.parse_for();
        }
        if self.at_ident("switch") {
            return vec![self.parse_switch()];
        }
        if self.at_ident("return") {
            self.advance();
            let val = if self.at_punct(";") || self.at_punct("}") || self.at_eof() {
                js_int(0)
            } else {
                self.parse_expr()
            };
            self.eat_punct(";");
            return vec![json!({ "type": "returnOperation", "data": { "value": val } })];
        }
        if self.at_ident("break") {
            self.advance();
            self.eat_punct(";");
            return vec![json!({ "type": "breakStmt", "data": {} })];
        }
        if self.at_ident("continue") {
            self.advance();
            self.eat_punct(";");
            return vec![json!({ "type": "continueStmt", "data": {} })];
        }
        if self.at_punct("{") {
            // A bare block: inline its statements (the VM has one flat scope).
            return self.parse_block();
        }
        let s = self.parse_simple();
        self.eat_punct(";");
        s
    }

    /// A block `{ ... }` or, when unbraced, a single statement — returned as the
    /// flat operation list the AST uses for `body` arrays.
    fn parse_block_or_single(&mut self) -> Vec<Value> {
        if self.at_punct("{") {
            self.parse_block()
        } else {
            self.parse_statement()
        }
    }
    fn parse_block(&mut self) -> Vec<Value> {
        self.expect_punct("{");
        let mut out: Vec<Value> = vec![];
        while !self.at_punct("}") && !self.at_eof() {
            out.extend(self.parse_statement());
        }
        self.expect_punct("}");
        out
    }

    fn parse_function_decl(&mut self) -> Value {
        self.expect_ident("function");
        let name = self.expect_ident_name();
        self.expect_punct("(");
        let mut params: Vec<String> = vec![];
        while !self.at_punct(")") && !self.at_eof() {
            params.push(self.expect_ident_name());
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")");
        let body = self.parse_block();
        json!({ "type": "functionDefinition", "data": { "name": name, "params": params, "body": body } })
    }

    // ---- classes (desugared to shared prototype + factory constructor) -------
    //
    // ES6 `class` syntax is lowered, in the front-end, onto plain objects + a
    // shared prototype. For
    //
    //     class C extends P {
    //         field = init;
    //         constructor(a) { super(a); this.x = a; }
    //         greet(n) { return this.x + n; }
    //     }
    //
    // it emits:
    //
    //   * each method as a *shared, top-level* function `__m_C__greet(n)` (defined
    //     once for the whole program, not per instance — so construction allocates
    //     no closures and method calls pay no capture-copy cost). `this` is not a
    //     declared parameter: when a method is read off an object the executor
    //     binds it to the receiver (see `bind_proto_method` / the indexer), so the
    //     body uses `this` as an ordinary local.
    //   * a prototype object `__proto_C = { __parent: __proto_P, greet: __m_C__greet }`
    //     built once, with `__parent` linking the inheritance chain.
    //   * an initialiser `__init_C(this, a)` that chains to `__init_P` (the leading
    //     `super(...)`), applies class-field initialisers, then runs the rest of
    //     the constructor body — `this` is an explicit parameter here.
    //   * a constructor `C(a)` = `let this = { __proto: __proto_C }; __init_C(this, a);
    //     return this;`. `new C(a)` and a bare `C(a)` both run it.
    //
    // The executor change this relies on is small and isolated: on a field miss,
    // the indexer resolves the name through the object's `__proto` chain and
    // returns the method bound to the receiver. No new opcode.
    fn parse_class(&mut self) -> Vec<Value> {
        self.expect_ident("class");
        let name = self.expect_ident_name();
        let parent = if self.eat_ident("extends") {
            Some(self.expect_ident_name())
        } else {
            None
        };
        self.expect_punct("{");

        // Lifted closures from field initialisers / `super(...)` args must stay
        // inside the installer (so they capture `this`), not leak to top level.
        let lifted_mark = self.lifted.len();

        let mut ctor_params: Vec<String> = vec![];
        let mut ctor_body: Vec<Value> = vec![];
        let mut had_ctor = false;
        let mut methods: Vec<(String, Vec<String>, Vec<Value>)> = vec![];
        let mut fields: Vec<(String, Value)> = vec![];
        // `static` members belong to the class itself, not its instances.
        let mut static_methods: Vec<(String, Vec<String>, Vec<Value>)> = vec![];
        let mut static_fields: Vec<(String, Value)> = vec![];

        // Method bodies may use `super.m(...)`; record the parent for the duration
        // of the class body so `parse_postfix` can resolve it (saving/restoring any
        // outer class context to support nested class definitions).
        let prev_parent = self.class_parent.take();
        self.class_parent = parent.clone();

        while !self.at_punct("}") && !self.at_eof() {
            if self.eat_punct(";") {
                continue;
            }
            let is_static = self.eat_ident("static");
            let member = self.expect_ident_name();
            if self.at_punct("(") {
                let params = self.parse_paren_params();
                let body = self.parse_block();
                if member == "constructor" {
                    ctor_params = params;
                    ctor_body = body;
                    had_ctor = true;
                } else if is_static {
                    static_methods.push((member, params, body));
                } else {
                    methods.push((member, params, body));
                }
            } else {
                // Class field: `name = expr;` or bare `name;` (defaults to 0).
                let val = if self.eat_punct("=") { self.parse_expr() } else { js_int(0) };
                self.eat_punct(";");
                if is_static {
                    static_fields.push((member, val));
                } else {
                    fields.push((member, val));
                }
            }
        }
        self.expect_punct("}");
        self.class_parent = prev_parent;

        // A derived class with no explicit constructor implicitly forwards its
        // arguments to `super` (`constructor(...args) { super(...args); }`). The VM
        // front-end has no rest params, so adopt the parent constructor's parameter
        // list (recorded when the parent was parsed) and forward those by name.
        if !had_ctor {
            if let Some(p) = &parent {
                if let Some(pp) = self.class_ctor_params.get(p) {
                    ctor_params = pp.clone();
                }
            }
        }
        self.class_ctor_params.insert(name.clone(), ctor_params.clone());

        // Drain field-initialiser / super-arg closures lifted during member
        // parsing; they belong at the head of the installer body.
        let field_lifted: Vec<Value> = self.lifted.split_off(lifted_mark);

        // Extract a leading `super(...)` call from the constructor body, if any.
        let mut super_args: Vec<Value> = vec![];
        let mut had_super = false;
        let mut ctor_rest: Vec<Value> = vec![];
        for stmt in ctor_body.into_iter() {
            if !had_super
                && stmt["type"] == "functionCall"
                && stmt["data"]["callee"]["type"] == "identifier"
                && stmt["data"]["callee"]["data"]["name"] == "super"
            {
                super_args = stmt["data"]["args"].as_array().cloned().unwrap_or_default();
                had_super = true;
            } else {
                ctor_rest.push(stmt);
            }
        }
        // An implicit constructor forwards its (parent-derived) parameters to
        // `super`, so a subclass that omits the constructor still initialises the
        // base correctly.
        if !had_ctor && parent.is_some() {
            super_args = ctor_params.iter().map(|p| js_ident(p)).collect();
            had_super = true;
        }

        let mut out: Vec<Value> = vec![];

        // 1. Methods as *shared, top-level* functions (defined once, not per
        //    instance). `this` is supplied by the method-dispatch path — when a
        //    method is read off an object it is bound to the receiver via the
        //    closure machinery — so it is not a declared parameter; the body
        //    references it as an ordinary local. A `__proto_<Class>` object maps
        //    each method name to its function, with `__parent` linking the chain.
        let mut proto_map = serde_json::Map::new();
        if let Some(p) = &parent {
            proto_map.insert("__parent".to_string(), js_ident(&format!("__proto_{}", p)));
        } else {
            proto_map.insert("__parent".to_string(), js_int(0));
        }
        for (mname, params, body) in methods.into_iter() {
            let fname = format!("__m_{}__{}", name, mname);
            out.push(json!({ "type": "functionDefinition", "data": {
                "name": fname, "params": params, "body": body } }));
            proto_map.insert(mname.clone(), js_ident(&fname));
        }

        // 2. The per-instance initialiser: chain to the parent initialiser
        //    (`super`), apply class-field initialisers, then run the constructor
        //    body. `this` is an explicit parameter here (the constructor passes the
        //    freshly-made instance). No method closures are created per instance.
        let mut init_body: Vec<Value> = vec![];
        if let Some(p) = &parent {
            let mut args: Vec<Value> = vec![js_ident("this")];
            if had_super {
                args.extend(super_args);
            }
            init_body.push(json!({ "type": "functionCall", "data": {
                "callee": js_ident(&format!("__init_{}", p)), "args": args } }));
        }
        // Class-field initialisers (their lifted closures first, so they capture
        // `this`), applied after `super` per JS field-initialiser semantics.
        init_body.extend(field_lifted);
        for (fname, val) in fields.into_iter() {
            init_body.push(js_assign(
                json!({ "type": "indexer", "data": { "target": js_ident("this"), "index": js_string(&fname) } }),
                val,
            ).unwrap());
        }
        init_body.extend(ctor_rest);
        let mut init_params: Vec<String> = vec!["this".to_string()];
        init_params.extend(ctor_params.iter().cloned());
        out.push(json!({ "type": "functionDefinition", "data": {
            "name": format!("__init_{}", name), "params": init_params, "body": init_body } }));

        // 3. The shared prototype, built once at class-definition time.
        out.push(js_def(&format!("__proto_{}", name),
            json!({ "type": "object", "data": { "value": Value::Object(proto_map) } })));

        // 4. The constructor: a fresh object linked to the prototype, initialised,
        //    and returned. `new C(...)` and a bare `C(...)` both run this.
        let mut this_obj = serde_json::Map::new();
        this_obj.insert("__proto".to_string(), js_ident(&format!("__proto_{}", name)));
        let mut call_args: Vec<Value> = vec![js_ident("this")];
        for p in ctor_params.iter() {
            call_args.push(js_ident(p));
        }
        let ctor_body_out = vec![
            js_def("this", json!({ "type": "object", "data": { "value": Value::Object(this_obj) } })),
            json!({ "type": "functionCall", "data": {
                "callee": js_ident(&format!("__init_{}", name)), "args": call_args } }),
            json!({ "type": "returnOperation", "data": { "value": js_ident("this") } }),
        ];
        out.push(json!({ "type": "functionDefinition", "data": {
            "name": name, "params": ctor_params, "body": ctor_body_out } }));

        // 5. Static members live on the class itself. There is no class object in
        //    the VM (the class name is the constructor function), so collect them
        //    into a companion holder `__static_<Class>`; a `C.member` access where
        //    `C` is a class with statics is rewritten to read off that holder (see
        //    `parse_postfix`). Static methods are shared top-level functions, like
        //    instance methods.
        if !static_methods.is_empty() || !static_fields.is_empty() {
            let mut static_map = serde_json::Map::new();
            // Inherit the parent's static holder so `Child.staticOfParent` resolves.
            if let Some(p) = &parent {
                if self.class_statics.contains(p) {
                    static_map.insert("__parent".to_string(), js_ident(&format!("__static_{}", p)));
                }
            }
            for (mname, params, body) in static_methods.into_iter() {
                let fname = format!("__sm_{}__{}", name, mname);
                out.push(json!({ "type": "functionDefinition", "data": {
                    "name": fname, "params": params, "body": body } }));
                static_map.insert(mname, js_ident(&fname));
            }
            for (fname, val) in static_fields.into_iter() {
                static_map.insert(fname, val);
            }
            out.push(js_def(&format!("__static_{}", name),
                json!({ "type": "object", "data": { "value": Value::Object(static_map) } })));
            self.class_statics.insert(name.clone());
        }

        out
    }

    fn parse_if(&mut self) -> Value {
        self.expect_ident("if");
        self.expect_punct("(");
        let cond = self.parse_expr();
        self.expect_punct(")");
        let body = self.parse_block_or_single();
        let mut data = json!({ "condition": cond, "body": body });
        if self.eat_ident("else") {
            if self.at_ident("if") {
                // `else if` — attach the whole nested `ifStmt` as the elseif
                // chain; `serialize_condition_chain` walks `node["data"]`.
                data["elseifStmt"] = self.parse_if();
            } else {
                let else_body = self.parse_block_or_single();
                data["elseStmt"] = json!({ "data": { "body": else_body } });
            }
        }
        json!({ "type": "ifStmt", "data": data })
    }

    fn parse_while(&mut self) -> Value {
        self.expect_ident("while");
        self.expect_punct("(");
        let cond = self.parse_expr();
        self.expect_punct(")");
        let body = self.parse_block_or_single();
        json!({ "type": "loopStmt", "data": { "condition": cond, "body": body } })
    }

    /// Desugar `for (init; cond; update) body` into the init statement(s)
    /// followed by a `loopStmt` whose body ends with the update step.
    fn parse_for(&mut self) -> Vec<Value> {
        self.expect_ident("for");
        self.expect_punct("(");
        let mut out: Vec<Value> = vec![];
        if !self.at_punct(";") {
            out.extend(self.parse_simple());
        }
        self.expect_punct(";");
        let cond = if self.at_punct(";") {
            json!({ "type": "bool", "data": { "value": true } })
        } else {
            self.parse_expr()
        };
        self.expect_punct(";");
        let update = if self.at_punct(")") {
            vec![]
        } else {
            self.parse_simple()
        };
        self.expect_punct(")");
        let body = self.parse_block_or_single();

        if !Self::body_has_continue(&body) {
            // Fast path: no `continue` in the body, so appending the update step
            // to the end of each iteration is both correct and cheap.
            let mut loop_body = body;
            loop_body.extend(update);
            out.push(json!({ "type": "loopStmt", "data": { "condition": cond, "body": loop_body } }));
            return out;
        }

        // `continue` jumps to the loop head, which would skip an update appended at
        // the tail. Run the update at the *top* of every iteration instead (guarded
        // so the first iteration skips it), then test the condition with `break`.
        // This makes `for (...; ...; update) { ...; continue; }` run `update` on the
        // `continue` path, matching JavaScript.
        self.anon_counter += 1;
        let started = format!("__for_started_{}", self.anon_counter);
        let mut loop_body: Vec<Value> = vec![];
        // if (started) { update } else { started = true }
        loop_body.push(json!({ "type": "ifStmt", "data": {
            "condition": js_ident(&started),
            "body": update,
            "elseStmt": { "data": { "body": [
                js_assign(js_ident(&started), json!({ "type": "bool", "data": { "value": true } })).unwrap()
            ] } }
        } }));
        // if (!(cond)) { break; }
        loop_body.push(json!({ "type": "ifStmt", "data": {
            "condition": json!({ "type": "not", "data": { "value": cond } }),
            "body": [ json!({ "type": "breakStmt", "data": {} }) ]
        } }));
        loop_body.extend(body);
        out.push(js_def(&started, json!({ "type": "bool", "data": { "value": false } })));
        out.push(json!({ "type": "loopStmt", "data": {
            "condition": json!({ "type": "bool", "data": { "value": true } }),
            "body": loop_body
        } }));
        out
    }

    /// Whether a (already-lowered) statement list contains a `continue` that
    /// targets *this* loop — i.e. one not nested inside another loop (which owns
    /// its own `continue`) or a function body.
    fn body_has_continue(body: &[Value]) -> bool {
        body.iter().any(Self::stmt_has_continue)
    }
    fn stmt_has_continue(stmt: &Value) -> bool {
        match stmt["type"].as_str().unwrap_or("") {
            "continueStmt" => true,
            // Nested loops / functions bind their own `continue`; do not descend.
            "loopStmt" | "functionDefinition" => false,
            "ifStmt" => {
                let d = &stmt["data"];
                if d["body"].as_array().map(|b| Self::body_has_continue(b)).unwrap_or(false) {
                    return true;
                }
                if d.get("elseifStmt").map(Self::stmt_has_continue).unwrap_or(false) {
                    return true;
                }
                d.get("elseStmt")
                    .and_then(|e| e["data"]["body"].as_array())
                    .map(|b| Self::body_has_continue(b))
                    .unwrap_or(false)
            }
            "switchStmt" => stmt["data"]["cases"]
                .as_array()
                .map(|cs| {
                    cs.iter().any(|c| {
                        c["body"]["body"].as_array().map(|b| Self::body_has_continue(b)).unwrap_or(false)
                    })
                })
                .unwrap_or(false),
            _ => false,
        }
    }

    fn parse_switch(&mut self) -> Value {
        self.expect_ident("switch");
        self.expect_punct("(");
        let val = self.parse_expr();
        self.expect_punct(")");
        self.expect_punct("{");
        let mut cases: Vec<Value> = vec![];
        while !self.at_punct("}") && !self.at_eof() {
            if self.eat_ident("case") {
                let cv = self.parse_expr();
                self.expect_punct(":");
                let body = self.parse_case_body();
                cases.push(json!({ "value": cv, "body": { "body": body } }));
            } else if self.eat_ident("default") {
                // No default opcode in the bytecode; parse and drop it.
                self.expect_punct(":");
                let _ = self.parse_case_body();
            } else {
                break;
            }
        }
        self.expect_punct("}");
        json!({ "type": "switchStmt", "data": { "value": val, "cases": cases } })
    }
    fn parse_case_body(&mut self) -> Vec<Value> {
        let mut body: Vec<Value> = vec![];
        while !self.at_ident("case")
            && !self.at_ident("default")
            && !self.at_punct("}")
            && !self.at_eof()
        {
            body.extend(self.parse_statement());
        }
        body
    }

    /// A "simple" statement with no trailing `;`: a declaration, an assignment
    /// (including compound and `++`/`--` forms), or a bare call expression.
    /// Used directly for `for` init/update clauses and wrapped by
    /// `parse_statement` for ordinary statements.
    fn parse_simple(&mut self) -> Vec<Value> {
        if self.at_ident("let") || self.at_ident("const") || self.at_ident("var") {
            self.advance();
            let mut out: Vec<Value> = vec![];
            loop {
                let name = self.expect_ident_name();
                let val = if self.eat_punct("=") {
                    self.parse_expr()
                } else {
                    js_int(0)
                };
                out.push(js_def(&name, val));
                if !self.eat_punct(",") {
                    break;
                }
            }
            return out;
        }
        // Prefix increment / decrement.
        if self.eat_punct("++") {
            let t = self.parse_postfix();
            return js_assign(t.clone(), js_arith("+", t, js_int(1)))
                .into_iter()
                .collect();
        }
        if self.eat_punct("--") {
            let t = self.parse_postfix();
            return js_assign(t.clone(), js_arith("-", t, js_int(1)))
                .into_iter()
                .collect();
        }
        let target = self.parse_expr();
        // Postfix increment / decrement.
        if self.eat_punct("++") {
            return js_assign(target.clone(), js_arith("+", target, js_int(1)))
                .into_iter()
                .collect();
        }
        if self.eat_punct("--") {
            return js_assign(target.clone(), js_arith("-", target, js_int(1)))
                .into_iter()
                .collect();
        }
        if self.eat_punct("=") {
            let rhs = self.parse_expr();
            return js_assign(target, rhs).into_iter().collect();
        }
        for (pp, op) in [("+=", "+"), ("-=", "-"), ("*=", "*"), ("/=", "/"), ("%=", "%")] {
            if self.eat_punct(pp) {
                let rhs = self.parse_expr();
                return js_assign(target.clone(), js_arith(op, target, rhs))
                    .into_iter()
                    .collect();
            }
        }
        // A bare expression carries meaning to the bytecode when it can have a
        // side effect: a call (`log(x)`), or a short-circuit / conditional whose
        // taken branch may call (`ready && start()`, `cond ? a() : b()`).
        match target["type"].as_str().unwrap_or("") {
            "functionCall" | "logical" | "ternary" => vec![target],
            _ => vec![],
        }
    }

    // ---- Expressions (precedence climbing) ----------------------------------

    fn parse_expr(&mut self) -> Value {
        self.parse_ternary()
    }

    /// The conditional operator sits below everything else and is
    /// right-associative: `a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`.
    fn parse_ternary(&mut self) -> Value {
        let cond = self.parse_logical_or();
        if self.eat_punct("?") {
            let consequent = self.parse_ternary();
            self.expect_punct(":");
            let alternate = self.parse_ternary();
            return js_ternary(cond, consequent, alternate);
        }
        cond
    }

    /// `||` — lower precedence than `&&`; left-associative.
    fn parse_logical_or(&mut self) -> Value {
        let mut left = self.parse_logical_and();
        while self.at_punct("||") {
            self.advance();
            let right = self.parse_logical_and();
            left = js_logical("||", left, right);
        }
        left
    }

    /// `&&` — binds tighter than `||`, looser than comparison/arithmetic (which
    /// `parse_binary` handles); left-associative.
    fn parse_logical_and(&mut self) -> Value {
        let mut left = self.parse_binary(0);
        while self.at_punct("&&") {
            self.advance();
            let right = self.parse_binary(0);
            left = js_logical("&&", left, right);
        }
        left
    }

    /// Map a punctuator to `(precedence, elpian operator, right-associative)`.
    fn binop(p: &str) -> Option<(u8, &'static str, bool)> {
        match p {
            "**" => Some((7, "^", true)),
            "*" => Some((6, "*", false)),
            "/" => Some((6, "/", false)),
            "%" => Some((6, "%", false)),
            "+" => Some((5, "+", false)),
            "-" => Some((5, "-", false)),
            "<" => Some((4, "<", false)),
            "<=" => Some((4, "<=", false)),
            ">" => Some((4, ">", false)),
            ">=" => Some((4, ">=", false)),
            "==" | "===" => Some((3, "==", false)),
            "!=" | "!==" => Some((3, "!=", false)),
            _ => None,
        }
    }

    fn parse_binary(&mut self, min_prec: u8) -> Value {
        let mut left = self.parse_unary();
        loop {
            let op_punct = match self.peek() {
                JsTok::Punct(p) => p.clone(),
                _ => break,
            };
            let (prec, op, right_assoc) = match Self::binop(&op_punct) {
                Some(x) => x,
                None => break,
            };
            if prec < min_prec {
                break;
            }
            self.advance();
            let next_min = if right_assoc { prec } else { prec + 1 };
            let right = self.parse_binary(next_min);
            left = js_arith(op, left, right);
        }
        left
    }

    fn parse_unary(&mut self) -> Value {
        // `new C(args)` — our class constructors are factory functions, so `new`
        // is sugar: it drops to the constructor call (`C(args)`). `new C` without
        // parentheses still constructs (call with no args).
        if self.at_ident("new") {
            self.advance();
            let e = self.parse_postfix();
            if e["type"] == "functionCall" {
                return e;
            }
            return json!({ "type": "functionCall", "data": { "callee": e, "args": [] } });
        }
        if self.eat_punct("!") {
            return json!({ "type": "not", "data": { "value": self.parse_unary() } });
        }
        if self.at_punct("-") {
            self.advance();
            let v = self.parse_unary();
            return js_negate(v);
        }
        if self.at_punct("+") {
            self.advance();
            return self.parse_unary();
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Value {
        let mut e = self.parse_primary();
        loop {
            if self.eat_punct(".") {
                let name = self.expect_ident_name();
                // `super.m` — resolve `m` on the parent prototype and bind it to the
                // current `this`, so `super.m(args)` dispatches to the overridden
                // base method with the right receiver.
                if e["type"] == "identifier" && e["data"]["name"] == "super" {
                    let parent = self.class_parent.clone().unwrap_or_default();
                    e = json!({ "type": "functionCall", "data": {
                        "callee": js_ident("superMethod"),
                        "args": [
                            js_ident(&format!("__proto_{}", parent)),
                            js_string(&name),
                            js_ident("this"),
                        ],
                    } });
                    continue;
                }
                // `Class.staticMember` — read off the class's static holder rather
                // than treating the constructor function as an object.
                if e["type"] == "identifier"
                    && e["data"]["name"].as_str().map(|n| self.class_statics.contains(n)).unwrap_or(false)
                {
                    let cname = e["data"]["name"].as_str().unwrap().to_string();
                    e = json!({ "type": "indexer", "data": {
                        "target": js_ident(&format!("__static_{}", cname)),
                        "index": js_string(&name),
                    } });
                    continue;
                }
                e = json!({ "type": "indexer", "data": { "target": e, "index": js_string(&name) } });
            } else if self.at_punct("[") {
                self.advance();
                let idx = self.parse_expr();
                self.expect_punct("]");
                e = json!({ "type": "indexer", "data": { "target": e, "index": idx } });
            } else if self.at_punct("(") {
                let args = self.parse_args();
                e = json!({ "type": "functionCall", "data": { "callee": e, "args": args } });
            } else {
                break;
            }
        }
        e
    }

    fn parse_args(&mut self) -> Vec<Value> {
        self.expect_punct("(");
        let mut args: Vec<Value> = vec![];
        while !self.at_punct(")") && !self.at_eof() {
            args.push(self.parse_expr());
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")");
        args
    }

    // ---- arrow / function expressions (desugared to lifted closures) --------

    /// With `self.pos` at a `(`, decide whether it opens an arrow parameter list
    /// by scanning to the matching `)` and checking for a following `=>`.
    fn is_paren_arrow(&self) -> bool {
        let mut depth = 0i32;
        let mut i = self.pos;
        while i < self.toks.len() {
            match &self.toks[i] {
                JsTok::Punct(p) if p == "(" => depth += 1,
                JsTok::Punct(p) if p == ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.toks.get(i + 1), Some(JsTok::Punct(p2)) if p2 == "=>");
                    }
                }
                JsTok::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Parse a parenthesized identifier list `( a, b, ... )` (arrow params or a
    /// `function` expression's params).
    fn parse_paren_params(&mut self) -> Vec<String> {
        self.expect_punct("(");
        let mut params: Vec<String> = vec![];
        while !self.at_punct(")") && !self.at_eof() {
            params.push(self.expect_ident_name());
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")");
        params
    }

    /// Consume `=> body` (concise expression or `{ block }`) and lift the result
    /// into a synthetic named closure, returning a reference to it.
    fn finish_arrow(&mut self, params: Vec<String>) -> Value {
        self.expect_punct("=>");
        let body = if self.at_punct("{") {
            self.parse_block()
        } else {
            // Concise body `=> expr` is `{ return expr; }`.
            let e = self.parse_expr();
            vec![json!({ "type": "returnOperation", "data": { "value": e } })]
        };
        self.make_anon(params, body)
    }

    /// A `function (params) { ... }` (or named `function f(...) {...}`) used in
    /// expression position — lowered like an arrow. Any name is accepted but not
    /// bound (the value is anonymous; reference it through where it is stored).
    fn parse_function_expr(&mut self) -> Value {
        self.expect_ident("function");
        if matches!(self.peek(), JsTok::Ident(_)) {
            self.advance(); // optional name, ignored
        }
        let params = self.parse_paren_params();
        let body = self.parse_block();
        self.make_anon(params, body)
    }

    /// Register a synthetic closure definition to be hoisted before the current
    /// statement and return an `identifier` referencing it.
    fn make_anon(&mut self, params: Vec<String>, body: Vec<Value>) -> Value {
        self.anon_counter += 1;
        let name = format!("__anon_{}", self.anon_counter);
        self.lifted.push(json!({
            "type": "functionDefinition",
            "data": { "name": name, "params": params, "body": body }
        }));
        js_ident(&name)
    }

    fn parse_primary(&mut self) -> Value {
        match self.peek().clone() {
            JsTok::Num(s) => {
                self.advance();
                js_num_literal(&s)
            }
            JsTok::Str(s) => {
                self.advance();
                js_string(&s)
            }
            JsTok::Ident(name) => match name.as_str() {
                "true" => {
                    self.advance();
                    json!({ "type": "bool", "data": { "value": true } })
                }
                "false" => {
                    self.advance();
                    json!({ "type": "bool", "data": { "value": false } })
                }
                // The bytecode has no null literal; model the empty value as 0.
                "null" | "undefined" => {
                    self.advance();
                    js_int(0)
                }
                "function" => self.parse_function_expr(),
                _ => {
                    self.advance();
                    // Single-parameter arrow without parens: `x => body`.
                    if self.at_punct("=>") {
                        return self.finish_arrow(vec![name]);
                    }
                    js_ident(&name)
                }
            },
            JsTok::Punct(p) => match p.as_str() {
                "(" => {
                    // `(a, b) => ...` / `() => ...` is an arrow, not a group.
                    if self.is_paren_arrow() {
                        let params = self.parse_paren_params();
                        return self.finish_arrow(params);
                    }
                    self.advance();
                    let e = self.parse_expr();
                    self.expect_punct(")");
                    e
                }
                "[" => self.parse_array(),
                "{" => self.parse_object(),
                other => panic!("js: unexpected token '{}'", other),
            },
            JsTok::Eof => panic!("js: unexpected end of input"),
        }
    }

    fn parse_array(&mut self) -> Value {
        self.expect_punct("[");
        let mut items: Vec<Value> = vec![];
        while !self.at_punct("]") && !self.at_eof() {
            items.push(self.parse_expr());
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("]");
        json!({ "type": "array", "data": { "value": items } })
    }

    fn parse_object(&mut self) -> Value {
        self.expect_punct("{");
        let mut map = serde_json::Map::new();
        while !self.at_punct("}") && !self.at_eof() {
            let key = match self.advance() {
                JsTok::Ident(s) => s,
                JsTok::Str(s) => s,
                JsTok::Num(s) => s,
                t => panic!("js: invalid object key {:?}", t),
            };
            let val = if self.eat_punct(":") {
                self.parse_expr()
            } else {
                // Shorthand `{ a }` is `{ a: a }`.
                js_ident(&key)
            };
            map.insert(key, val);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("}");
        json!({ "type": "object", "data": { "value": Value::Object(map) } })
    }
}

/// Parse JavaScript source into Elpian AST JSON (a `program` node). Panics on a
/// syntax error in the supported subset; use [`try_parse_js`] for a fallible
/// variant.
pub fn parse_js(src: &str) -> serde_json::Value {
    JsParser::new(tokenize_js(src)).parse_program()
}

/// Parse JavaScript source into Elpian AST JSON, returning an error instead of
/// panicking when the source is outside the supported subset.
pub fn try_parse_js(src: &str) -> Result<serde_json::Value, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_js(src)))
        .map_err(|_| "javascript parse error".to_string())
}

/// Compile JavaScript source straight to bytecode by lowering it to the Elpian
/// AST and feeding that to [`compile_ast`] — the same `from ast` path every
/// other entry point uses.
pub fn compile_js(src: &str) -> Vec<u8> {
    compile_ast(parse_js(src), 0)
}
