// use wasm_bindgen::prelude::wasm_bindgen;

use crate::sdk::{
    capabilities::CapabilitySet,
    context::Context,
    data::{Array, Function, Object, Payload, Val, ValGroup, ValMap},
    lifecycle::ExecControl,
    limits::{Governor, ResourceLimits},
    program::{DecodedProgram, UnitKind},
    stdlib,
};
use core::panic;
use std::{cell::RefCell, collections::HashMap, fmt, i16, rc::Rc};

use std::vec;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OperationTypes {
    DefineVar,
    AssignVar,
    CallFunc,
    ReturnVal,
    IfStmt,
    LoopStmt,
    SwitchStmt,
    Arithmetic,
    Indexer,
    NotVal,
    ObjExpr,
    ArrExpr,
    CondBrch,
    CastOprt,
    Logical,
    Conditional,
    Dummy,
}

impl fmt::Display for OperationTypes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExecStates {
    AssignVarExtractName,
    AssignVarExtractIndex,
    AssignVarExtractValue,
    DefineVarExtractName,
    DefineVarExtractValue,
    CallFuncStarted,
    CallFuncExtractFunc,
    CallFuncExtractParam,
    CallFuncFinished,
    ReturnValStarted,
    ReturnValFinished,
    IfStmtIsConditioned,
    IfStmtFinished,
    LoopStmtStarted,
    LoopStmtFinished,
    SwitchStmtStarted,
    SwitchStmtExtractVal,
    SwitchStmtExtractCase,
    SwitchStmtFinished,
    ArithmeticStarted,
    ArithmeticExtractOp,
    ArithmeticExtractArg1,
    ArithmeticExtractArg2,
    IndexerStarted,
    IndexerExtractVarName,
    IndexerExtractIndex,
    NotValStarted,
    NotValFinished,
    ObjExprStarted,
    ObjExprExtractInfo,
    ObjExprExtractProp,
    ObjExprFinished,
    ArrExprStarted,
    ArrExprExtractInfo,
    ArrExprExtractItem,
    ArrExprFinished,
    CondBranchStarted,
    CondBranchFinished,
    CastOprtStarted,
    CastOprtFinished,
    LogicalExtractOp1,
    LogicalExtractOp2,
    CondExprExtractCond,
    CondExprExtractValue,
    Dummy,
}

impl fmt::Display for ExecStates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// The payload handed to [`Operation::set_state`] on a state transition.
///
/// This used to be a `Box<dyn Any>`, which heap-allocated (and then dynamically
/// downcast) on *every* operation step — the arithmetic operand flow alone does
/// this hundreds of thousands of times per frame. A closed enum of the handful
/// of shapes the executor actually feeds keeps the payload on the stack: no
/// allocation, no vtable, no downcast. Each `(operation, state)` pair fixes the
/// shape, so the matching extractor is unambiguous (and a mismatch is a bug, not
/// a recoverable case — hence `unreachable!`).
pub enum StateData {
    Empty,
    Val(Val),
    I16(i16),
    I32(i32),
    Str(String),
    StrI16(String, i16),
    I64I32(i64, i32),
}

impl StateData {
    #[inline]
    fn val(self) -> Val {
        match self {
            StateData::Val(v) => v,
            _ => unreachable!("StateData::val on a non-Val payload"),
        }
    }
    #[inline]
    fn i16v(self) -> i16 {
        match self {
            StateData::I16(v) => v,
            _ => unreachable!("StateData::i16v on a non-I16 payload"),
        }
    }
    #[inline]
    fn i32v(self) -> i32 {
        match self {
            StateData::I32(v) => v,
            _ => unreachable!("StateData::i32v on a non-I32 payload"),
        }
    }
    #[inline]
    fn string(self) -> String {
        match self {
            StateData::Str(v) => v,
            _ => unreachable!("StateData::string on a non-Str payload"),
        }
    }
    #[inline]
    fn str_i16(self) -> (String, i16) {
        match self {
            StateData::StrI16(s, n) => (s, n),
            _ => unreachable!("StateData::str_i16 on a non-StrI16 payload"),
        }
    }
    #[inline]
    fn i64_i32(self) -> (i64, i32) {
        match self {
            StateData::I64I32(a, b) => (a, b),
            _ => unreachable!("StateData::i64_i32 on a non-I64I32 payload"),
        }
    }
}

pub trait Operation {
    fn get_type(&self) -> OperationTypes;
    fn get_state(&self) -> ExecStates;
    fn set_state(&mut self, state: ExecStates, data: StateData);
    fn get_data(&self) -> Vec<Val>;
    /// For a [`SwitchStmt`] mid-collection: the `(body_start, body_end)` unit
    /// range of the *next* case about to be collected. The run loop reads the
    /// end to skip the case body once its value has been evaluated. Other
    /// operations never collect cases, so the default is unused.
    fn next_case_bounds(&self) -> (usize, usize) {
        (0, 0)
    }
}

impl fmt::Debug for dyn Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Operation{{{} {}}}", self.get_type(), self.get_state())
    }
}

struct DefineVariable {
    typ: OperationTypes,
    state: ExecStates,
    pub var_name: Option<String>,
    pub var_value: Option<Val>,
}

impl DefineVariable {
    pub fn new() -> Self {
        DefineVariable {
            typ: OperationTypes::DefineVar,
            state: ExecStates::DefineVarExtractName,
            var_name: None,
            var_value: None,
        }
    }
}

impl Operation for DefineVariable {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::DefineVarExtractName {
            self.var_name = Some(data.string());
        } else if state == ExecStates::DefineVarExtractValue {
            self.var_value = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val {
                typ: 7,
                data: Payload::from(self.var_name.clone().unwrap()),
            },
            self.var_value.clone().unwrap(),
        ]
    }
}

struct AssignVariable {
    typ: OperationTypes,
    state: ExecStates,
    pub var_name: Option<String>,
    pub assign_target_type: i16,
    pub index: Option<Val>,
    pub var_value: Option<Val>,
}

impl AssignVariable {
    pub fn new() -> Self {
        AssignVariable {
            typ: OperationTypes::AssignVar,
            state: ExecStates::AssignVarExtractName,
            var_name: None,
            assign_target_type: 0,
            index: None,
            var_value: None,
        }
    }
}

impl Operation for AssignVariable {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::AssignVarExtractName {
            let (var_name, assign_target_type) = data.str_i16();
            self.var_name = Some(var_name.clone());
            self.assign_target_type = assign_target_type;
        } else if state == ExecStates::AssignVarExtractIndex {
            if self.assign_target_type == 2 {
                self.index = Some(data.val());
            } else {
                panic!("elpian error: wrong state set to assignment operation");
            }
        } else if state == ExecStates::AssignVarExtractValue {
            self.var_value = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        if self.assign_target_type == 2 {
            // The index is only known after `AssignVarExtractIndex`; callers that
            // read `get_data` earlier (e.g. to inspect the target type while still
            // in `AssignVarExtractName`) get a typed-null placeholder for it.
            let index = self
                .index
                .clone()
                .unwrap_or_else(|| Val { typ: 0, data: Payload::Null });
            if self.var_value.is_none() {
                return vec![
                    Val {
                        typ: 7,
                        data: Payload::from(self.var_name.clone().unwrap()),
                    },
                    Val {
                        typ: 6,
                        data: Payload::from(self.assign_target_type),
                    },
                    index,
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                ];
            } else {
                return vec![
                    Val {
                        typ: 7,
                        data: Payload::from(self.var_name.clone().unwrap()),
                    },
                    Val {
                        typ: 6,
                        data: Payload::from(self.assign_target_type),
                    },
                    index,
                    self.var_value.clone().unwrap(),
                ];
            }
        } else {
            if self.var_value.is_none() {
                return vec![
                    Val {
                        typ: 7,
                        data: Payload::from(self.var_name.clone().unwrap()),
                    },
                    Val {
                        typ: 6,
                        data: Payload::from(self.assign_target_type),
                    },
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                ];
            } else {
                return vec![
                    Val {
                        typ: 7,
                        data: Payload::from(self.var_name.clone().unwrap()),
                    },
                    Val {
                        typ: 6,
                        data: Payload::from(self.assign_target_type),
                    },
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                    self.var_value.clone().unwrap(),
                ];
            }
        }
    }
}

struct CallFunction {
    typ: OperationTypes,
    state: ExecStates,
    pub func: Option<Rc<RefCell<Function>>>,
    pub is_native: bool,
    pub param_count: i32,
    pub params: Vec<Val>,
}

impl CallFunction {
    /// `param_count` is the number of arguments the *call site* provides, folded
    /// into the `Call` unit at decode time (so it no longer trails the callee in
    /// the instruction stream).
    pub fn new(param_count: i32) -> Self {
        CallFunction {
            typ: OperationTypes::CallFunc,
            state: ExecStates::CallFuncStarted,
            func: None,
            param_count,
            is_native: false,
            params: vec![],
        }
    }
}

impl Operation for CallFunction {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::CallFuncExtractFunc {
            // The callee value just evaluated; the argument count is already known
            // (folded into the `Call` unit, stored as `param_count` at creation).
            let callee = data.val();
            if callee.typ == 10 {
                self.func = Some(callee.as_func());
                self.is_native = false;
            } else if callee.typ == 255 {
                self.func = Some(Rc::new(RefCell::new(Function::new(
                    "".to_string(),
                    0,
                    0,
                    vec!["apiName".to_string(), "input".to_string()],
                ))));
                self.param_count = 2;
                self.is_native = true;
            } else if callee.typ == 252 {
                // Native standard-library builtin. The finish check gates on the
                // call-site argument count (`param_count`), and the builtin reads
                // its arguments positionally from the provided-args array — the
                // formal parameter *names* are never consulted. So we skip building
                // the `arg0..argN` name list entirely (it allocated one `String`
                // per argument on every native call, the hottest path in the VM).
                let name = callee.as_string();
                self.func = Some(Rc::new(RefCell::new(Function::new(name, 0, 0, Vec::new()))));
                self.is_native = true;
            } else {
                panic!("elpian error: the specified data is not runnable");
            }
        } else if state == ExecStates::CallFuncExtractParam {
            self.params.push(data.val());
        }
        if self.func.is_some() {
            // Collect exactly as many argument values as the *call site* provided
            // (`param_count`), not as many as the function declares. JavaScript
            // calls are arity-flexible: extra arguments are ignored and missing
            // ones bind to `undefined` (done when the frame is built). Gating on
            // the declared param count desynced the arg stream whenever a function
            // was called with fewer arguments than it declares.
            if self.params.len() >= self.param_count.max(0) as usize {
                self.state = ExecStates::CallFuncFinished;
            }
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val {
                typ: 10,
                data: Payload::from(self.func.clone().unwrap()),
            },
            Val {
                typ: 6,
                data: Payload::from(self.is_native),
            },
            Val {
                typ: 2,
                data: Payload::from(self.param_count),
            },
            Val {
                typ: 9,
                data: Payload::from(Rc::new(RefCell::new(Array::new(
                    self.params.clone(),
                )))),
            },
        ]
    }
}

struct ReturnValue {
    typ: OperationTypes,
    state: ExecStates,
    pub value: Option<Val>,
}

impl ReturnValue {
    pub fn new() -> Self {
        ReturnValue {
            typ: OperationTypes::ReturnVal,
            state: ExecStates::ReturnValStarted,
            value: None,
        }
    }
}

impl Operation for ReturnValue {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::ReturnValFinished {
            self.value = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![self.value.clone().unwrap()]
    }
}

struct IfStmt {
    typ: OperationTypes,
    state: ExecStates,
    pub has_condition: bool,
    pub condition: Option<Val>,
    // Branch targets, as unit indices, folded into the `IfHead` unit at decode.
    body_start: usize,
    body_end: usize,
    next: usize,
    branch_after: usize,
}

impl IfStmt {
    pub fn new(
        has_condition: bool,
        body_start: usize,
        body_end: usize,
        next: usize,
        branch_after: usize,
    ) -> Self {
        IfStmt {
            typ: OperationTypes::IfStmt,
            // A conditioned arm waits for its condition to evaluate; an
            // unconditional `else` is already decided (it always runs).
            state: if has_condition {
                ExecStates::IfStmtIsConditioned
            } else {
                ExecStates::IfStmtFinished
            },
            has_condition,
            condition: if has_condition {
                None
            } else {
                Some(Val { typ: 6, data: Payload::from(true) })
            },
            body_start,
            body_end,
            next,
            branch_after,
        }
    }
}

impl Operation for IfStmt {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::IfStmtFinished {
            self.condition = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val { typ: 6, data: Payload::from(self.has_condition) },
            self.condition.clone().unwrap(),
            Val { typ: 3, data: Payload::from(self.body_start as i64) },
            Val { typ: 3, data: Payload::from(self.body_end as i64) },
            Val { typ: 3, data: Payload::from(self.next as i64) },
            Val { typ: 3, data: Payload::from(self.branch_after as i64) },
        ]
    }
}

struct LoopStmt {
    typ: OperationTypes,
    state: ExecStates,
    pub condition: Option<Val>,
    // Loop bounds, as unit indices, folded into the `Loop` unit at decode.
    body_start: usize,
    body_end: usize,
    branch_after: usize,
}

impl LoopStmt {
    pub fn new(body_start: usize, body_end: usize, branch_after: usize) -> Self {
        LoopStmt {
            typ: OperationTypes::LoopStmt,
            state: ExecStates::LoopStmtStarted,
            condition: None,
            body_start,
            body_end,
            branch_after,
        }
    }
}

impl Operation for LoopStmt {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::LoopStmtFinished {
            self.condition = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            self.condition.clone().unwrap(),
            Val { typ: 3, data: Payload::from(self.body_start as i64) },
            Val { typ: 3, data: Payload::from(self.body_end as i64) },
            Val { typ: 3, data: Payload::from(self.branch_after as i64) },
        ]
    }
}

struct SwitchStmt {
    typ: OperationTypes,
    state: ExecStates,
    pub comparing_value: Option<Val>,
    pub branch_after_start: usize,
    pub case_count: usize,
    pub cases: Vec<(Val, usize, usize)>,
    /// The `(body_start, body_end)` unit range of each case, in order, folded
    /// into the `Switch` unit at decode. Each case value is still an expression
    /// evaluated at run time; as it arrives it is paired with the next entry.
    cases_bounds: std::rc::Rc<Vec<(usize, usize)>>,
}

impl SwitchStmt {
    pub fn new(branch_after: usize, cases_bounds: std::rc::Rc<Vec<(usize, usize)>>) -> Self {
        SwitchStmt {
            typ: OperationTypes::SwitchStmt,
            state: ExecStates::SwitchStmtStarted,
            comparing_value: None,
            branch_after_start: branch_after,
            case_count: cases_bounds.len(),
            cases: vec![],
            cases_bounds,
        }
    }
}

impl Operation for SwitchStmt {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn next_case_bounds(&self) -> (usize, usize) {
        self.cases_bounds.get(self.cases.len()).copied().unwrap_or((0, 0))
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::SwitchStmtExtractVal {
            // The switch value just evaluated; the case table is already known.
            self.comparing_value = Some(data.val());
        } else if state == ExecStates::SwitchStmtExtractCase {
            // A case value just evaluated; pair it with the next case's body range.
            let value = data.val();
            let (start, end) = self.next_case_bounds();
            self.cases.push((value, start, end));
        }
        if self.case_count == self.cases.len() {
            self.state = ExecStates::SwitchStmtFinished;
        }
    }

    fn get_data(&self) -> Vec<Val> {
        let case_items: Vec<Val> = self
            .cases
            .iter()
            .map(|item| {
                let mut case_info = ValMap::default();
                case_info.insert("val".to_string(), item.0.clone());
                case_info.insert(
                    "start".to_string(),
                    Val {
                        typ: 3,
                        data: Payload::from(item.1 as i64),
                    },
                );
                case_info.insert(
                    "end".to_string(),
                    Val {
                        typ: 3,
                        data: Payload::from(item.2 as i64),
                    },
                );
                Val {
                    typ: 8,
                    data: Payload::from(Rc::new(RefCell::new(Object::new(
                        -1,
                        ValGroup::new(case_info),
                    )))),
                }
            })
            .collect();
        vec![
            self.comparing_value.clone().unwrap(),
            Val {
                typ: 3,
                data: Payload::from(self.branch_after_start as i64),
            },
            Val {
                typ: 3,
                data: Payload::from(self.case_count as i64),
            },
            Val {
                typ: 9,
                data: Payload::from(Rc::new(RefCell::new(Array::new(
                    case_items,
                )))),
            },
        ]
    }
}

struct Arithmetic {
    typ: OperationTypes,
    state: ExecStates,
    pub arg1: Option<Val>,
    pub arg2: Option<Val>,
    pub op: i16,
}

impl Arithmetic {
    pub fn new() -> Self {
        Arithmetic {
            typ: OperationTypes::Arithmetic,
            state: ExecStates::ArithmeticStarted,
            arg1: None,
            arg2: None,
            op: 0,
        }
    }
}

impl Operation for Arithmetic {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::ArithmeticExtractOp {
            self.op = data.i16v();
        } else if state == ExecStates::ArithmeticExtractArg1 {
            self.arg1 = Some(data.val());
        } else if state == ExecStates::ArithmeticExtractArg2 {
            self.arg2 = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val {
                typ: 1,
                data: Payload::from(self.op),
            },
            self.arg1.clone().unwrap(),
            self.arg2.clone().unwrap(),
        ]
    }
}

struct IndexerValue {
    typ: OperationTypes,
    state: ExecStates,
    pub var: Option<Val>,
    pub index: Option<Val>,
}

impl IndexerValue {
    pub fn new() -> Self {
        IndexerValue {
            typ: OperationTypes::Indexer,
            state: ExecStates::IndexerStarted,
            var: None,
            index: None,
        }
    }
}

impl Operation for IndexerValue {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::IndexerExtractVarName {
            self.var = Some(data.val());
        } else if state == ExecStates::IndexerExtractIndex {
            self.index = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![self.var.clone().unwrap(), self.index.clone().unwrap()]
    }
}

struct NotValue {
    typ: OperationTypes,
    state: ExecStates,
    pub value: Option<Val>,
}

impl NotValue {
    pub fn new() -> Self {
        NotValue {
            typ: OperationTypes::NotVal,
            state: ExecStates::NotValStarted,
            value: None,
        }
    }
}

impl Operation for NotValue {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::NotValFinished {
            self.value = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![self.value.clone().unwrap()]
    }
}

struct ObjectExpr {
    typ: OperationTypes,
    state: ExecStates,
    pub object_typ_id: i64,
    pub prop_count: i32,
    pub props: Vec<Val>,
}

impl ObjectExpr {
    pub fn new() -> Self {
        ObjectExpr {
            typ: OperationTypes::ObjExpr,
            state: ExecStates::ObjExprStarted,
            object_typ_id: 0,
            prop_count: 0,
            props: vec![],
        }
    }
}

impl Operation for ObjectExpr {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::ObjExprExtractInfo {
            let val = data.i64_i32();
            self.object_typ_id = val.0;
            self.prop_count = val.1;
        } else if state == ExecStates::ObjExprExtractProp {
            let val = data.val();
            self.props.push(val.clone());
        }
        if (self.prop_count as usize) == (self.props.len() / 2) {
            self.state = ExecStates::ObjExprFinished;
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val {
                typ: 3,
                data: Payload::from(self.object_typ_id),
            },
            Val {
                typ: 2,
                data: Payload::from(self.prop_count),
            },
            Val {
                typ: 9,
                data: Payload::from(Rc::new(RefCell::new(Array::new(
                    self.props.clone(),
                )))),
            },
        ]
    }
}

struct ArrayExpr {
    typ: OperationTypes,
    state: ExecStates,
    pub item_count: i32,
    pub items: Vec<Val>,
}

impl ArrayExpr {
    pub fn new() -> Self {
        ArrayExpr {
            typ: OperationTypes::ArrExpr,
            state: ExecStates::ArrExprStarted,
            item_count: 0,
            items: vec![],
        }
    }
}

impl Operation for ArrayExpr {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::ArrExprExtractInfo {
            self.item_count = data.i32v();
        } else if state == ExecStates::ArrExprExtractItem {
            let val = data.val();
            self.items.push(val.clone());
        }
        if (self.item_count as usize) == self.items.len() {
            self.state = ExecStates::ArrExprFinished;
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            Val {
                typ: 2,
                data: Payload::from(self.item_count),
            },
            Val {
                typ: 9,
                data: Payload::from(Rc::new(RefCell::new(Array::new(
                    self.items.clone(),
                )))),
            },
        ]
    }
}

struct CondBranch {
    typ: OperationTypes,
    state: ExecStates,
    pub condition: Option<Val>,
    pub true_branch: i64,
    pub false_branch: i64,
}

impl CondBranch {
    /// `true_branch`/`false_branch` are unit indices folded into the `CondBranch`
    /// unit at decode.
    pub fn new(true_branch: usize, false_branch: usize) -> Self {
        CondBranch {
            typ: OperationTypes::CondBrch,
            state: ExecStates::CondBranchStarted,
            condition: None,
            true_branch: true_branch as i64,
            false_branch: false_branch as i64,
        }
    }
}

impl Operation for CondBranch {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::CondBranchFinished {
            // The condition just evaluated; both targets are already known.
            self.condition = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            self.condition.clone().unwrap(),
            Val {
                typ: 3,
                data: Payload::from(self.true_branch),
            },
            Val {
                typ: 3,
                data: Payload::from(self.false_branch),
            },
        ]
    }
}

struct CastOp {
    typ: OperationTypes,
    state: ExecStates,
    pub data: Option<Val>,
    pub target_type: String,
}

impl CastOp {
    /// `target_type` is folded into the `Cast` unit at decode.
    pub fn new(target_type: String) -> Self {
        CastOp {
            typ: OperationTypes::CastOprt,
            state: ExecStates::CastOprtStarted,
            data: None,
            target_type,
        }
    }
}

impl Operation for CastOp {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, data: StateData) {
        self.state = state;
        if state == ExecStates::CastOprtFinished {
            // The value just evaluated; the target type is already known.
            self.data = Some(data.val());
        }
    }

    fn get_data(&self) -> Vec<Val> {
        vec![
            self.data.clone().unwrap(),
            Val {
                typ: 7,
                data: Payload::from(self.target_type.clone()),
            },
        ]
    }
}

/// Short-circuiting `&&` / `||`. The left operand is evaluated first; the right
/// is only evaluated when the result is not already decided (`&&` with a truthy
/// left, `||` with a falsy left). On short-circuit the dispatch loop reuses the
/// left value as the result and jumps the program counter to `op2_end`, skipping
/// the right operand's units entirely. No double evaluation, exact JS semantics.
struct LogicalOp {
    typ: OperationTypes,
    state: ExecStates,
    is_or: bool,
    op2_end: usize,
}

impl LogicalOp {
    pub fn new(is_or: bool, op2_end: usize) -> Self {
        LogicalOp {
            typ: OperationTypes::Logical,
            state: ExecStates::LogicalExtractOp1,
            is_or,
            op2_end,
        }
    }
}

impl Operation for LogicalOp {
    fn get_state(&self) -> ExecStates {
        self.state
    }
    fn get_type(&self) -> OperationTypes {
        self.typ
    }
    fn set_state(&mut self, state: ExecStates, _data: StateData) {
        // Operands are consumed straight from `main_reg` in the dispatch loop; the
        // op only tracks which operand is awaited and carries its skip target.
        self.state = state;
    }
    fn get_data(&self) -> Vec<Val> {
        vec![
            Val { typ: 6, data: Payload::from(self.is_or) },
            Val { typ: 3, data: Payload::from(self.op2_end as i64) },
        ]
    }
}

/// The conditional (ternary) operator `c ? a : b`. The condition is evaluated
/// first; the dispatch loop then either lets execution fall into the consequent
/// or jumps to `alt_start`, and after the taken branch's value is produced jumps
/// to `end` so the other branch's units are skipped.
struct ConditionalOp {
    typ: OperationTypes,
    state: ExecStates,
    alt_start: usize,
    end: usize,
}

impl ConditionalOp {
    pub fn new(alt_start: usize, end: usize) -> Self {
        ConditionalOp {
            typ: OperationTypes::Conditional,
            state: ExecStates::CondExprExtractCond,
            alt_start,
            end,
        }
    }
}

impl Operation for ConditionalOp {
    fn get_state(&self) -> ExecStates {
        self.state
    }
    fn get_type(&self) -> OperationTypes {
        self.typ
    }
    fn set_state(&mut self, state: ExecStates, _data: StateData) {
        self.state = state;
    }
    fn get_data(&self) -> Vec<Val> {
        vec![
            Val { typ: 3, data: Payload::from(self.alt_start as i64) },
            Val { typ: 3, data: Payload::from(self.end as i64) },
        ]
    }
}

struct DummyOp {
    typ: OperationTypes,
    state: ExecStates,
}

impl DummyOp {
    pub fn new() -> Self {
        DummyOp {
            typ: OperationTypes::Dummy,
            state: ExecStates::Dummy,
        }
    }
}

impl Operation for DummyOp {
    fn get_state(&self) -> ExecStates {
        self.state
    }

    fn get_type(&self) -> OperationTypes {
        self.typ
    }

    fn set_state(&mut self, state: ExecStates, _data: StateData) {
        self.state = state;
    }

    fn get_data(&self) -> Vec<Val> {
        vec![]
    }
}

pub struct Executor {
    executor_id: i16,
    /// Program counter: an index into [`DecodedProgram::units`] (not a byte
    /// offset). The interpreter advances it one unit at a time and branches by
    /// assigning a target unit index directly.
    pointer: usize,
    /// One past the last unit of the range currently executing (the top-level
    /// program, or a function/control body). The step loop stops when
    /// `pointer == end_at`.
    end_at: usize,
    ctx: Context,
    /// The program decoded once into an in-memory list of operation objects,
    /// with all branch targets pre-translated to unit indices. The raw bytecode
    /// is not retained — the interpreter traverses these units directly. See
    /// `program.rs`.
    prog: DecodedProgram,
    cb_counter: i64,
    pending_func_result_value: Val,
    registers: Vec<Box<dyn Operation>>,
    _allowed_api: HashMap<String, bool>,
    run_cb_id: i64,
    exec_globally: bool,
    reserved_host_call: Option<(u8, i64, Val)>,
    pub processing: bool,
    /// Resource governor (instruction / memory / storage / call-depth budgets).
    governor: Governor,
    /// Host-togglable capabilities gating every `askHost` side effect.
    capabilities: CapabilitySet,
    /// Host-driven pause / resume / terminate control.
    control: ExecControl,
    /// Set when `run_from` suspended this turn because of a host pause request,
    /// so `single_thread_operation` reports the instance as paused (not done).
    paused_out: bool,
    /// A fatal trap (limit overrun or builtin error) that ended the instance.
    /// Once set, the instance is terminated and reports this reason to the host.
    trap: Option<String>,
}

impl Executor {
    pub fn create_in_single_thread(
        program: Vec<u8>,
        exec_id: i16,
        func_group: Vec<String>,
    ) -> Self {
        let mut allowed_api: HashMap<String, bool> = HashMap::new();
        for api_name in func_group.iter() {
            allowed_api.insert(api_name.clone(), true);
        }
        // Decode the bytecode once into the in-memory unit list; the raw bytes
        // are not kept past this point.
        let prog = DecodedProgram::decode(&program);
        let end_at = prog.units.len();
        Executor {
            _allowed_api: allowed_api,
            executor_id: exec_id,
            pointer: 0,
            end_at,
            ctx: Context::new(),
            prog,
            cb_counter: 0,
            pending_func_result_value: Val::new(254, Payload::Null),
            registers: vec![],
            run_cb_id: 0,
            exec_globally: false,
            reserved_host_call: None,
            processing: false,
            governor: Governor::new(ResourceLimits::unlimited()),
            capabilities: CapabilitySet::allow_all(),
            control: ExecControl::new(),
            paused_out: false,
            trap: None,
        }
    }

    // ---- Host-facing instance management (limits, capabilities, lifecycle) --

    /// Replace the active resource-limit policy. Usage already accrued is kept.
    pub fn set_limits(&mut self, limits: ResourceLimits) {
        self.governor.set_limits(limits);
    }
    /// Current resource limits.
    pub fn limits(&self) -> ResourceLimits {
        self.governor.limits()
    }
    /// Live resource usage tally.
    pub fn usage(&self) -> crate::sdk::limits::ResourceUsage {
        self.governor.usage()
    }
    /// Mutable access to the capability set (host toggles network / storage / …).
    pub fn capabilities_mut(&mut self) -> &mut CapabilitySet {
        &mut self.capabilities
    }
    /// Snapshot of the capability set.
    pub fn capabilities(&self) -> CapabilitySet {
        self.capabilities.clone()
    }
    /// Replace the capability set wholesale.
    pub fn set_capabilities(&mut self, caps: CapabilitySet) {
        self.capabilities = caps;
    }
    /// Host: request the instance pause at the next step boundary.
    pub fn request_pause(&mut self) {
        self.control.request_pause();
    }
    /// Host: resume a paused instance.
    pub fn resume_control(&mut self) {
        self.control.resume();
    }
    /// Host: request the instance terminate. If it is idle (between turns) the
    /// termination is confirmed immediately; if it is mid-flight (e.g. servicing
    /// a host call) the request is observed and confirmed at the next step
    /// boundary by the run loop.
    pub fn request_terminate(&mut self) {
        self.control.request_terminate();
        if !self.processing {
            self.control.confirm_terminated();
            self.registers.clear();
        }
    }
    /// Current run state.
    pub fn run_state(&self) -> crate::sdk::lifecycle::RunState {
        self.control.state()
    }
    /// Whether the instance suspended on a host pause this turn.
    pub fn was_paused(&self) -> bool {
        self.paused_out
    }
    /// The fatal trap reason, if the instance was stopped by a limit or error.
    pub fn trap_reason(&self) -> Option<String> {
        self.trap.clone()
    }
    /// Charge the storage governor on behalf of the host filesystem; returns the
    /// limit error string if the storage cap would be exceeded.
    pub fn charge_storage(&mut self, delta: i64) -> Result<(), String> {
        self.governor.charge_storage(delta).map_err(|e| e.to_string())
    }
    /// Reconcile the absolute storage figure with the host filesystem total.
    pub fn set_storage_bytes(&mut self, bytes: u64) -> Result<(), String> {
        self.governor.set_storage_bytes(bytes).map_err(|e| e.to_string())
    }
    /// After `run_from` returns, surface a host-driven stop (trap / terminate /
    /// pause) as the operation result, short-circuiting the normal
    /// done/host-call detection. Returns `None` when execution stopped for an
    /// ordinary reason (completion or a pending host call).
    fn control_status(&mut self, cb_id: i64) -> Option<(u8, i64, Val)> {
        if self.trap.is_some() || self.control.is_terminated() {
            self.processing = false;
            // Status 0x06 = terminated/trapped; payload is the reason string
            // (empty for a clean host-ordered terminate).
            let msg = self.trap.clone().unwrap_or_default();
            return Some((0x06, cb_id, Val::new(7, Payload::from(msg))));
        }
        if self.paused_out {
            self.processing = false;
            // Status 0x05 = paused; the continuation is preserved for `resume`.
            return Some((0x05, cb_id, Val::new(253, Payload::Null)));
        }
        None
    }
    pub fn single_thread_operation(
        &mut self,
        op_code: u8,
        cb_id: i64,
        payload: Val,
    ) -> (u8, i64, Val) {
        match op_code {
            0x01 => {
                // println!("executor: run_func called");
                self.run_cb_id = cb_id;
                self.governor.begin_turn();
                self.paused_out = false;
                // A fresh top-level turn carries no pending return value. Clear
                // any sentinel a previous call may have left behind so a function
                // that falls off its end without an explicit `return` yields "no
                // value" instead of leaking the last returned result.
                self.pending_func_result_value = Val::new(254, Payload::Null);
                if self.control.is_terminated() {
                    return (0x06, cb_id, Val::new(7, Payload::from(
                        self.trap.clone().unwrap_or_default(),
                    )));
                }
                if payload.typ != 9 {
                    self.exec_globally = true;
                    self.processing = true;
                    let result = self.run_from(
                        0,
                        self.prog.units.len(),
                        false,
                        Val {
                            typ: 0,
                            data: Payload::Null,
                        },
                        false,
                    );
                    if let Some(status) = self.control_status(cb_id) {
                        return status;
                    }
                    if self.reserved_host_call.is_some() {
                        let host_call_data = self.reserved_host_call.clone().unwrap();
                        self.reserved_host_call = None;
                        return host_call_data;
                    } else if self.pointer == self.ctx.memory.get(0).unwrap().borrow().frozen_end {
                        self.processing = false;
                        return (0x01, cb_id, result);
                    } else {
                        self.processing = false;
                        return (
                            0x00,
                            0,
                            Val {
                                typ: 0,
                                data: Payload::Null,
                            },
                        );
                    }
                } else {
                    self.exec_globally = false;
                    self.processing = true;
                    let arr = payload.as_array();
                    let func_name = arr.borrow().data[0].as_string();
                    let input = arr.borrow().data[1].clone();
                    let val = self.ctx.find_val_in_first_scope(&func_name);
                    if !val.is_empty() {
                        let func = val.as_func();
                        let mut m = ValMap::default();
                        if !func.borrow().params.is_empty() {
                            m.insert(func.borrow().params[0].clone(), input);
                        }
                        self.ctx.push_scope_with_args(
                            "funcBody".to_string(),
                            func.borrow().start,
                            func.borrow().start,
                            func.borrow().end,
                            m,
                        );
                        let result = self.run_from(
                            func.borrow().start,
                            func.borrow().end,
                            false,
                            Val {
                                typ: 0,
                                data: Payload::Null,
                            },
                            true,
                        );
                        if let Some(status) = self.control_status(cb_id) {
                            return status;
                        }
                        if self.reserved_host_call.is_some() {
                            let host_call_data = self.reserved_host_call.clone().unwrap();
                            self.reserved_host_call = None;
                            return host_call_data;
                        } else if self.ctx.memory.len() == 1 {
                            self.processing = false;
                            return (0x01, cb_id, result);
                        } else {
                            self.processing = false;
                            return (
                                0x00,
                                0,
                                Val {
                                    typ: 0,
                                    data: Payload::Null,
                                },
                            );
                        }
                    } else {
                        // The host may invoke an *optional* lifecycle handler the
                        // app didn't define (e.g. `onEvent`, `onResize`, `onFrame`,
                        // `onHostMessage`). Per the documented contract this is a
                        // harmless no-op — so complete the turn with no value rather
                        // than panicking. Panicking here poisons the VM mutex, after
                        // which every subsequent call fails ("cannot recursively
                        // acquire mutex"), silently freezing a host that simply drove
                        // a handler the app chose not to implement.
                        self.processing = false;
                        return (
                            0x01,
                            cb_id,
                            Val {
                                typ: 0,
                                data: Payload::Null,
                            },
                        );
                    }
                }
            }
            0x02 => {
                // println!("executor: print_memory called");
                self.ctx.memory.iter().for_each(|scope| {
                    scope
                        .borrow()
                        .memory
                        .borrow()
                        .data
                        .iter()
                        .for_each(|(key, val)| {
                            println!("{{ key: {}, val: {} }}", key, val.stringify());
                        });
                });
                return (
                    0x00,
                    0,
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                );
            }
            0x03 | 0x04 => {
                // 0x03 resumes after a host call (injecting `payload` as the
                // call's return value). 0x04 resumes after a host-ordered pause
                // (no value injected — `payload` is the typ-254 "no value"
                // marker), continuing exactly where the step loop suspended.
                self.governor.begin_turn();
                self.paused_out = false;
                if self.control.is_terminated() {
                    return (0x06, cb_id, Val::new(7, Payload::from(
                        self.trap.clone().unwrap_or_default(),
                    )));
                }
                self.processing = true;
                let result = self.run_from(
                    self.pointer,
                    self.end_at,
                    true,
                    payload,
                    !self.exec_globally,
                );
                if let Some(status) = self.control_status(cb_id) {
                    return status;
                }
                if !self.ctx.memory.is_empty() {
                    if self.exec_globally {
                        if self.reserved_host_call.is_some() {
                            let host_call_data = self.reserved_host_call.clone().unwrap();
                            self.reserved_host_call = None;
                            return host_call_data;
                        } else if self.pointer
                            == self.ctx.memory.get(0).unwrap().borrow().frozen_end
                        {
                            self.processing = false;
                            return (0x01, cb_id, result);
                        } else {
                            self.processing = false;
                            return (
                                0x00,
                                0,
                                Val {
                                    typ: 0,
                                    data: Payload::Null,
                                },
                            );
                        }
                    } else {
                        if self.reserved_host_call.is_some() {
                            let host_call_data = self.reserved_host_call.clone().unwrap();
                            self.reserved_host_call = None;
                            return host_call_data;
                        } else if self.ctx.memory.len() == 1 {
                            self.processing = false;
                            return (0x01, cb_id, result);
                        } else {
                            self.processing = false;
                            return (
                                0x00,
                                0,
                                Val {
                                    typ: 0,
                                    data: Payload::Null,
                                },
                            );
                        }
                    }
                } else {
                    self.processing = false;
                    return (
                        0x00,
                        0,
                        Val {
                            typ: 0,
                            data: Payload::Null,
                        },
                    );
                }
            }
            _ => {
                self.processing = false;
                return (
                    0x00,
                    0,
                    Val {
                        typ: 0,
                        data: Payload::Null,
                    },
                );
            }
        }
    }
    /// Resolve an identifier reference to a value: a scope-chain binding shadows
    /// everything; otherwise `askHost` is the host-call seam (typ 255) and a
    /// known standard-library builtin resolves to its native handle (typ 252).
    fn resolve_ident(&mut self, id: &str) -> Val {
        if id == "askHost" {
            return Val { typ: 255, data: Payload::Null };
        }
        let bound = self.ctx.find_val_globally(id);
        if !bound.is_empty() {
            return bound;
        }
        if stdlib::is_builtin(id) {
            return Val { typ: 252, data: Payload::from(id.to_string()) };
        }
        bound
    }
    fn check_float_range(&self, num: f64) -> Val {
        if num < f32::MAX.into() {
            return Val {
                typ: 4,
                data: Payload::from(num as f32),
            };
        } else {
            return Val {
                typ: 5,
                data: Payload::from(num),
            };
        }
    }
    fn check_int_range(&self, num: i64) -> Val {
        if num < i16::MAX.into() {
            return Val {
                typ: 1,
                data: Payload::from(num as i16),
            };
        } else if num < i32::MAX.into() {
            return Val {
                typ: 2,
                data: Payload::from(num as i32),
            };
        } else {
            return Val {
                typ: 3,
                data: Payload::from(num),
            };
        }
    }
    fn operate_sum(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as i64,
                    2 => arg1.as_i32() as i64,
                    3 => arg1.as_i64() as i64,
                    _ => 0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as i64;
                        self.check_int_range(val1 + val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as i64;
                        self.check_int_range(val1 + val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as i64;
                        self.check_int_range(val1 + val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp + val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp + val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and integer can not be summed");
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let val1_temp = val1.to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        panic!("elpian error: object and integer can not be summed");
                    }
                    9 => {
                        let mut val2 = arg2.as_array().borrow().clone_arr();
                        val2.data.insert(0, arg1);
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(val2))),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and integer can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and integer can not be summed");
                    }
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64() as f64,
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as f64;
                        self.check_float_range(val1 + val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as f64;
                        self.check_float_range(val1 + val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as f64;
                        self.check_float_range(val1 + val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        self.check_float_range(val1 + val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        self.check_float_range(val1 + val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and float can not be summed");
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let val1_temp = val1.to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        panic!("elpian error: object and float can not be summed");
                    }
                    9 => {
                        let mut val2 = arg2.as_array().borrow().clone_arr();
                        val2.data.insert(0, arg1);
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(val2))),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and float can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and float can not be summed");
                    }
                }
            }
            6 => {
                let val1 = arg1.as_bool();
                match arg2.typ {
                    1 => {
                        panic!("elpian error: bool and integer can not be summed");
                    }
                    2 => {
                        panic!("elpian error: bool and integer can not be summed");
                    }
                    3 => {
                        panic!("elpian error: objeboolt and integer can not be summed");
                    }
                    4 => {
                        panic!("elpian error: bool and float can not be summed");
                    }
                    5 => {
                        panic!("elpian error: bool and float can not be summed");
                    }
                    6 => {
                        let val2 = arg2.as_bool();
                        Val {
                            typ: 7,
                            data: Payload::from(val1 ^ val2),
                        }
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let val1_temp = val1.to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        panic!("elpian error: object and bool can not be summed");
                    }
                    9 => {
                        let mut val2 = arg2.as_array().borrow().clone_arr();
                        val2.data.insert(0, arg1);
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(val2))),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and bool can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and bool can not be summed");
                    }
                }
            }
            7 => {
                let val1 = arg1.as_string();
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    2 => {
                        let val2 = arg2.as_i32().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    3 => {
                        let val2 = arg2.as_i64().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    4 => {
                        let val2 = arg2.as_f32().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    5 => {
                        let val2 = arg2.as_f64().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    6 => {
                        let val2 = arg2.as_bool().to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    8 => {
                        let val2 = arg2.as_object().borrow().stringify();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    9 => {
                        let val2 = arg2.as_array().borrow().stringify();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1, val2)),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and string can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and string can not be summed");
                    }
                }
            }
            8 => {
                let val1 = arg1.as_object();
                match arg2.typ {
                    1 => {
                        panic!("elpian error: object and integer can not be summed");
                    }
                    2 => {
                        panic!("elpian error: object and integer can not be summed");
                    }
                    3 => {
                        panic!("elpian error: object and integer can not be summed");
                    }
                    4 => {
                        panic!("elpian error: object and float can not be summed");
                    }
                    5 => {
                        panic!("elpian error: object and float can not be summed");
                    }
                    6 => {
                        panic!("elpian error: object and bool can not be summed");
                    }
                    7 => {
                        let val1_temp = val1.borrow().stringify();
                        let val2 = arg2.as_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        let val2 = arg2.as_object();
                        val2.borrow().data.data.iter().for_each(|(k, v)| {
                            val1.borrow_mut().data.data.insert(k.clone(), v.clone());
                        });
                        Val {
                            typ: 8,
                            data: Payload::from(val2),
                        }
                    }
                    9 => {
                        let mut val2 = arg2.as_array().borrow().clone_arr();
                        val2.data.insert(0, arg1);
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(val2))),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and object can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and object can not be summed");
                    }
                }
            }
            9 => {
                let val1 = arg1.as_array();
                match arg2.typ {
                    1 | 2 | 3 | 4 | 5 | 6 | 8 | 10 => {
                        val1.borrow_mut().data.push(arg2);
                        Val {
                            typ: 9,
                            data: Payload::from(val1),
                        }
                    }
                    7 => {
                        let val1_temp = val1.borrow().stringify();
                        let val2 = arg2.as_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    9 => {
                        let mut val1 = arg2.as_array().borrow().clone_arr();
                        val1.data.append(&mut arg2.as_array().borrow().data.clone());
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(val1))),
                        }
                    }
                    _ => {
                        panic!("elpian error: unknown data type and array can not be summed");
                    }
                }
            }
            10 => {
                panic!("elpian error: function can not be summed with other types");
            }
            _ => {
                panic!("elpian error: unknown type can not be summed with other types");
            }
        }
    }
    fn operate_multiply(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as i64,
                    2 => arg1.as_i32() as i64,
                    3 => arg1.as_i64() as i64,
                    _ => 0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as i64;
                        self.check_int_range(val1 * val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as i64;
                        self.check_int_range(val1 * val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as i64;
                        self.check_int_range(val1 * val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp * val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp * val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and integer can not be multiplied");
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let mut res = "".to_string();
                        for _i in 0..val1 {
                            res.push_str(&val2);
                        }
                        Val {
                            typ: 7,
                            data: Payload::from(res),
                        }
                    }
                    8 => {
                        panic!("elpian error: object and integer can not be multiplied");
                    }
                    9 => {
                        let val2 = arg2.as_array();
                        let mut res: Vec<Val> = vec![];
                        for _i in 0..val1 {
                            res.append(&mut val2.borrow().data.clone());
                        }
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(
                                Array::new(res),
                            ))),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and integer can not be multiplied");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and integer can not be multiplied");
                    }
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64() as f64,
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as f64;
                        self.check_float_range(val1 * val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as f64;
                        self.check_float_range(val1 * val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as f64;
                        self.check_float_range(val1 * val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        self.check_float_range(val1 * val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        self.check_float_range(val1 * val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and float can not be multiplied");
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let val1_temp = val1.to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        panic!("elpian error: object and float can not be multiplied");
                    }
                    9 => {
                        panic!("elpian error: array and float can not be multiplied");
                    }
                    10 => {
                        panic!("elpian error: function and float can not be multiplied");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and float can not be multiplied");
                    }
                }
            }
            6 => {
                let val1 = arg1.as_bool();
                match arg2.typ {
                    1 => {
                        panic!("elpian error: bool and integer can not be multiplied");
                    }
                    2 => {
                        panic!("elpian error: bool and integer can not be multiplied");
                    }
                    3 => {
                        panic!("elpian error: bool and integer can not be multiplied");
                    }
                    4 => {
                        panic!("elpian error: bool and float can not be multiplied");
                    }
                    5 => {
                        panic!("elpian error: bool and float can not be multiplied");
                    }
                    6 => {
                        let val2 = arg2.as_bool();
                        Val {
                            typ: 7,
                            data: Payload::from(val1 & val2),
                        }
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        let val1_temp = val1.to_string();
                        Val {
                            typ: 7,
                            data: Payload::from(format!("{}{}", val1_temp, val2)),
                        }
                    }
                    8 => {
                        if val1 {
                            return arg2.clone();
                        } else {
                            return Val {
                                typ: 8,
                                data: Payload::from(Rc::new(RefCell::new(
                                    Object::new(-2, ValGroup::new_empty()),
                                ))),
                            };
                        }
                    }
                    9 => {
                        if val1 {
                            return arg2.clone();
                        } else {
                            return Val {
                                typ: 9,
                                data: Payload::from(Rc::new(RefCell::new(
                                    Array::new_empty(),
                                ))),
                            };
                        }
                    }
                    10 => {
                        panic!("elpian error: function and bool can not be multiplied");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and bool can not be multiplied");
                    }
                }
            }
            7 => {
                let val1 = arg1.as_string();
                match arg2.typ {
                    1 => {
                        let mut res = "".to_string();
                        for _i in 0..arg2.as_i16() {
                            res.push_str(&val1);
                        }
                        Val {
                            typ: 7,
                            data: Payload::from(res),
                        }
                    }
                    2 => {
                        let mut res = "".to_string();
                        for _i in 0..arg2.as_i32() {
                            res.push_str(&val1);
                        }
                        Val {
                            typ: 7,
                            data: Payload::from(res),
                        }
                    }
                    3 => {
                        let mut res = "".to_string();
                        for _i in 0..arg2.as_i64() {
                            res.push_str(&val1);
                        }
                        Val {
                            typ: 7,
                            data: Payload::from(res),
                        }
                    }
                    4 => {
                        panic!("elpian error: string and float can not be multiplied");
                    }
                    5 => {
                        panic!("elpian error: string and float can not be multiplied");
                    }
                    6 => {
                        panic!("elpian error: string and bool can not be multiplied");
                    }
                    7 => {
                        panic!("elpian error: string and string can not be multiplied");
                    }
                    8 => {
                        panic!("elpian error: string and object can not be multiplied");
                    }
                    9 => {
                        panic!("elpian error: string and array can not be multiplied");
                    }
                    10 => {
                        panic!("elpian error: string and function can not be multiplied");
                    }
                    _ => {
                        panic!("elpian error: string type and unknown data can not be multiplied");
                    }
                }
            }
            8 => {
                panic!("elpian error: object can not be multiplied with other types");
            }
            9 => {
                let val1 = arg1.as_array();
                match arg2.typ {
                    1 => {
                        let mut res: Vec<Val> = vec![];
                        for _i in 0..arg2.as_i16() {
                            res.append(&mut val1.borrow().data.clone());
                        }
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(
                                Array::new(res),
                            ))),
                        }
                    }
                    2 => {
                        let mut res: Vec<Val> = vec![];
                        for _i in 0..arg2.as_i32() {
                            res.append(&mut val1.borrow().data.clone());
                        }
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(
                                Array::new(res),
                            ))),
                        }
                    }
                    3 => {
                        let mut res: Vec<Val> = vec![];
                        for _i in 0..arg2.as_i64() {
                            res.append(&mut val1.borrow().data.clone());
                        }
                        Val {
                            typ: 9,
                            data: Payload::from(Rc::new(RefCell::new(
                                Array::new(res),
                            ))),
                        }
                    }
                    4 | 5 => {
                        panic!("elpian error: array and float can not be multiplied");
                    }
                    6 => {
                        if arg2.as_bool() {
                            return arg1.clone();
                        } else {
                            return Val {
                                typ: 9,
                                data: Payload::from(Rc::new(RefCell::new(
                                    Array::new_empty(),
                                ))),
                            };
                        }
                    }
                    7 => {
                        panic!("elpian error: array and string can not be multiplied");
                    }
                    8 => {
                        panic!("elpian error: array and object can not be multiplied");
                    }
                    10 => {
                        panic!("elpian error: array and function can not be multiplied");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and array can not be multiplied");
                    }
                }
            }
            10 => {
                panic!("elpian error: function can not be multiplied with other types");
            }
            _ => {
                panic!("elpian error: unknown type can not be multiplied with other types");
            }
        }
    }
    fn operate_subtract(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as i64,
                    2 => arg1.as_i32() as i64,
                    3 => arg1.as_i64() as i64,
                    _ => 0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as i64;
                        self.check_int_range(val1 - val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as i64;
                        self.check_int_range(val1 - val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as i64;
                        self.check_int_range(val1 - val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp - val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        let val1_temp = val1 as f64;
                        self.check_float_range(val1_temp - val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and integer can not be subtracted");
                    }
                    7 => {
                        panic!("elpian error: string can not be subtracted from integer");
                    }
                    8 => {
                        panic!("elpian error: object and integer can not be subtracted");
                    }
                    9 => {
                        panic!("elpian error: array can not be subtracted from integer");
                    }
                    10 => {
                        panic!("elpian error: function and integer can not be subtracted");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and integer can not be subtracted");
                    }
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64() as f64,
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as f64;
                        self.check_float_range(val1 - val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as f64;
                        self.check_float_range(val1 - val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as f64;
                        self.check_float_range(val1 - val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        self.check_float_range(val1 - val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        self.check_float_range(val1 - val2)
                    }
                    6 => {
                        panic!("elpian error: boolean and float can not be subtracted");
                    }
                    7 => {
                        panic!("elpian error: string can not be subtracted from float");
                    }
                    8 => {
                        panic!("elpian error: object and float can not be subtracted");
                    }
                    9 => {
                        panic!("elpian error: array can not be subtracted from float");
                    }
                    10 => {
                        panic!("elpian error: function and float can not be subtracted");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and float can not be subtracted");
                    }
                }
            }
            6 => {
                let val1 = arg1.as_bool();
                match arg2.typ {
                    1 => {
                        panic!("elpian error: bool and float can not be subtracted");
                    }
                    2 => {
                        panic!("elpian error: bool and integer can not be subtracted");
                    }
                    3 => {
                        panic!("elpian error: bool and integer can not be subtracted");
                    }
                    4 => {
                        panic!("elpian error: bool and float can not be subtracted");
                    }
                    5 => {
                        panic!("elpian error: bool and float can not be subtracted");
                    }
                    6 => {
                        let val2 = arg2.as_bool();
                        Val {
                            typ: 7,
                            data: Payload::from(val1 ^ val2),
                        }
                    }
                    7 => {
                        panic!("elpian error: bool and string can not be subtracted");
                    }
                    8 => {
                        panic!("elpian error: bool and object can not be subtracted");
                    }
                    9 => {
                        let val2 = arg2.as_array();
                        val2.borrow_mut().data.insert(0, arg1);
                        Val {
                            typ: 9,
                            data: Payload::from(val2),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and bool can not be subtracted");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and bool can not be subtracted");
                    }
                }
            }
            7 => {
                let mut val1 = arg1.as_string();
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    2 => {
                        let val2 = arg2.as_i32().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    3 => {
                        let val2 = arg2.as_i64().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    4 => {
                        let val2 = arg2.as_f32().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    5 => {
                        let val2 = arg2.as_f64().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    6 => {
                        let val2 = arg2.as_bool().to_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    7 => {
                        let val2 = arg2.as_string();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    8 => {
                        let val2 = arg2.as_object().borrow().stringify();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    9 => {
                        let val2 = arg2.as_array().borrow().stringify();
                        val1 = val1.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1),
                        }
                    }
                    10 => {
                        panic!("elpian error: function and string can not be subtracted");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and string can not be subtracted");
                    }
                }
            }
            8 => {
                let val1 = arg1.as_object();
                match arg2.typ {
                    1 => {
                        panic!("elpian error: object and integer can not be subtracted");
                    }
                    2 => {
                        panic!("elpian error: object and integer can not be subtracted");
                    }
                    3 => {
                        panic!("elpian error: object and integer can not be subtracted");
                    }
                    4 => {
                        panic!("elpian error: object and float can not be subtracted");
                    }
                    5 => {
                        panic!("elpian error: object and float can not be subtracted");
                    }
                    6 => {
                        panic!("elpian error: object and bool can not be subtracted");
                    }
                    7 => {
                        let mut val1_temp = val1.borrow().stringify();
                        let val2 = arg2.as_string();
                        val1_temp = val1_temp.replace(&val2, "");
                        Val {
                            typ: 7,
                            data: Payload::from(val1_temp),
                        }
                    }
                    8 => {
                        let val2 = arg2.as_object();
                        let mut deleted: Vec<String> = vec![];
                        val2.borrow().data.data.iter().for_each(|(k, v)| {
                            if val1.borrow().data.data.contains_key(k) {
                                let val1_data = &val1.borrow().data.data;
                                let v2 = val1_data.get(k).unwrap();
                                if self.is_eq(v.clone(), v2.clone()) {
                                    deleted.push(k.clone());
                                }
                            }
                        });
                        deleted.iter().for_each(|k| {
                            val1.borrow_mut().data.data.remove(&k.clone());
                        });
                        Val {
                            typ: 8,
                            data: Payload::from(val2),
                        }
                    }
                    9 => {
                        panic!("elpian error: array can not be subtracted from object");
                    }
                    10 => {
                        panic!("elpian error: function and integer can not be summed");
                    }
                    _ => {
                        panic!("elpian error: unknown data type and integer can not be summed");
                    }
                }
            }
            9 => {
                let val1 = arg1.as_array();
                match arg2.typ {
                    1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 10 => {
                        val1.borrow_mut().data = val1
                            .borrow()
                            .data
                            .iter()
                            .filter_map(|item| {
                                if self.is_eq(item.clone(), arg2.clone()) {
                                    return None;
                                } else {
                                    return Some(item.clone());
                                }
                            })
                            .collect();
                        Val {
                            typ: 9,
                            data: Payload::from(val1),
                        }
                    }
                    9 => {
                        let val2 = arg2.as_array();
                        val1.borrow_mut().data = val1
                            .borrow()
                            .data
                            .iter()
                            .filter_map(|item| {
                                for item2 in val2.borrow().data.iter() {
                                    if self.is_eq(item.clone(), item2.clone()) {
                                        return None;
                                    }
                                }
                                return Some(item.clone());
                            })
                            .collect();
                        Val {
                            typ: 9,
                            data: Payload::from(val1),
                        }
                    }
                    _ => {
                        panic!("elpian error: unknown data type and integer can not be summed");
                    }
                }
            }
            10 => {
                panic!("nothing can be subtracted from function");
            }
            _ => {
                panic!("can not subtract unknown type with anything");
            }
        }
    }
    fn operate_division(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as f64,
                    2 => arg1.as_i32() as f64,
                    3 => arg1.as_i64() as f64,
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    6 => {
                        panic!("elpian error: integer and boolean can not be divisioned");
                    }
                    7 => {
                        panic!("elpian error: integer and boolean can not be divisioned");
                    }
                    8 => {
                        panic!("elpian error: integer and object can not be divisioned");
                    }
                    9 => {
                        panic!("elpian error: integer and array can not be divisioned");
                    }
                    10 => {
                        panic!("elpian error: integer and function can not be divisioned");
                    }
                    _ => {
                        panic!("elpian error: integer and unknown data type can not be divisioned");
                    }
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64() as f64,
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => {
                        let val2 = arg2.as_i16() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    2 => {
                        let val2 = arg2.as_i32() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    3 => {
                        let val2 = arg2.as_i64() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    4 => {
                        let val2 = arg2.as_f32() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    5 => {
                        let val2 = arg2.as_f64() as f64;
                        self.check_float_range(val1 / val2)
                    }
                    6 => {
                        panic!("elpian error: float and boolean can not be divisioned");
                    }
                    7 => {
                        panic!("elpian error: float and string can not be divisioned");
                    }
                    8 => {
                        panic!("elpian error: float and object can not be divisioned");
                    }
                    9 => {
                        panic!("elpian error: float and array can not be divisioned");
                    }
                    10 => {
                        panic!("elpian error: float and function can not be divisioned");
                    }
                    _ => {
                        panic!("elpian error: float and unknown data type can not be divisioned");
                    }
                }
            }
            6 => {
                panic!("elpian error: bool can not be divisioned with other types");
            }
            7 => {
                panic!("elpian error: bool can not be divisioned with other types");
            }
            8 => {
                panic!("elpian error: object can not be divisioned with other types");
            }
            9 => {
                panic!("elpian error: array can not be divisioned with other types");
            }
            10 => {
                panic!("elpian error: function can not be divisioned with other types");
            }
            _ => {
                panic!("elpian error: unknown type can not be divisioned with other types");
            }
        }
    }
    fn operate_modulo(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            // Integer dividend: keep an integer remainder for integer divisors,
            // promote to float when the divisor is a float (matching the rest of
            // the arithmetic ops, e.g. `operate_subtract`).
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as i64,
                    2 => arg1.as_i32() as i64,
                    3 => arg1.as_i64(),
                    _ => 0,
                };
                match arg2.typ {
                    1 => self.check_int_range(val1 % arg2.as_i16() as i64),
                    2 => self.check_int_range(val1 % arg2.as_i32() as i64),
                    3 => self.check_int_range(val1 % arg2.as_i64()),
                    4 => self.check_float_range(val1 as f64 % arg2.as_f32() as f64),
                    5 => self.check_float_range(val1 as f64 % arg2.as_f64()),
                    6 => panic!("elpian error: integer and boolean can not be modulo'd"),
                    7 => panic!("elpian error: integer and string can not be modulo'd"),
                    8 => panic!("elpian error: integer and object can not be modulo'd"),
                    9 => panic!("elpian error: integer and array can not be modulo'd"),
                    10 => panic!("elpian error: integer and function can not be modulo'd"),
                    _ => panic!("elpian error: integer and unknown data type can not be modulo'd"),
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64(),
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => self.check_float_range(val1 % arg2.as_i16() as f64),
                    2 => self.check_float_range(val1 % arg2.as_i32() as f64),
                    3 => self.check_float_range(val1 % arg2.as_i64() as f64),
                    4 => self.check_float_range(val1 % arg2.as_f32() as f64),
                    5 => self.check_float_range(val1 % arg2.as_f64()),
                    6 => panic!("elpian error: float and boolean can not be modulo'd"),
                    7 => panic!("elpian error: float and string can not be modulo'd"),
                    8 => panic!("elpian error: float and object can not be modulo'd"),
                    9 => panic!("elpian error: float and array can not be modulo'd"),
                    10 => panic!("elpian error: float and function can not be modulo'd"),
                    _ => panic!("elpian error: float and unknown data type can not be modulo'd"),
                }
            }
            6 => panic!("elpian error: bool can not be modulo'd with other types"),
            7 => panic!("elpian error: string can not be modulo'd with other types"),
            8 => panic!("elpian error: object can not be modulo'd with other types"),
            9 => panic!("elpian error: array can not be modulo'd with other types"),
            10 => panic!("elpian error: function can not be modulo'd with other types"),
            _ => panic!("elpian error: unknown type can not be modulo'd with other types"),
        }
    }
    fn operate_power(&self, arg1: Val, arg2: Val) -> Val {
        match arg1.typ {
            // Integer base raised to a non-negative integer exponent stays an
            // integer (falling back to float on overflow); any float operand or
            // negative exponent yields a float, like the other arithmetic ops.
            1 | 2 | 3 => {
                let val1 = match arg1.typ {
                    1 => arg1.as_i16() as i64,
                    2 => arg1.as_i32() as i64,
                    3 => arg1.as_i64(),
                    _ => 0,
                };
                let int_pow = |exp: i64| -> Val {
                    if (0..=u32::MAX as i64).contains(&exp) {
                        match val1.checked_pow(exp as u32) {
                            Some(r) => self.check_int_range(r),
                            None => self.check_float_range((val1 as f64).powf(exp as f64)),
                        }
                    } else {
                        self.check_float_range((val1 as f64).powf(exp as f64))
                    }
                };
                match arg2.typ {
                    1 => int_pow(arg2.as_i16() as i64),
                    2 => int_pow(arg2.as_i32() as i64),
                    3 => int_pow(arg2.as_i64()),
                    4 => self.check_float_range((val1 as f64).powf(arg2.as_f32() as f64)),
                    5 => self.check_float_range((val1 as f64).powf(arg2.as_f64())),
                    6 => panic!("elpian error: integer and boolean can not be exponentiated"),
                    7 => panic!("elpian error: integer and string can not be exponentiated"),
                    8 => panic!("elpian error: integer and object can not be exponentiated"),
                    9 => panic!("elpian error: integer and array can not be exponentiated"),
                    10 => panic!("elpian error: integer and function can not be exponentiated"),
                    _ => panic!("elpian error: integer and unknown data type can not be exponentiated"),
                }
            }
            4 | 5 => {
                let val1 = match arg1.typ {
                    4 => arg1.as_f32() as f64,
                    5 => arg1.as_f64(),
                    _ => 0.0,
                };
                match arg2.typ {
                    1 => self.check_float_range(val1.powf(arg2.as_i16() as f64)),
                    2 => self.check_float_range(val1.powf(arg2.as_i32() as f64)),
                    3 => self.check_float_range(val1.powf(arg2.as_i64() as f64)),
                    4 => self.check_float_range(val1.powf(arg2.as_f32() as f64)),
                    5 => self.check_float_range(val1.powf(arg2.as_f64())),
                    6 => panic!("elpian error: float and boolean can not be exponentiated"),
                    7 => panic!("elpian error: float and string can not be exponentiated"),
                    8 => panic!("elpian error: float and object can not be exponentiated"),
                    9 => panic!("elpian error: float and array can not be exponentiated"),
                    10 => panic!("elpian error: float and function can not be exponentiated"),
                    _ => panic!("elpian error: float and unknown data type can not be exponentiated"),
                }
            }
            6 => panic!("elpian error: bool can not be exponentiated with other types"),
            7 => panic!("elpian error: string can not be exponentiated with other types"),
            8 => panic!("elpian error: object can not be exponentiated with other types"),
            9 => panic!("elpian error: array can not be exponentiated with other types"),
            10 => panic!("elpian error: function can not be exponentiated with other types"),
            _ => panic!("elpian error: unknown type can not be exponentiated with other types"),
        }
    }
    fn is_eq(&self, v: Val, v2: Val) -> bool {
        return match v.typ {
            1 | 2 | 3 => {
                let v_val = match v.typ {
                    1 => v.as_i16() as i64,
                    2 => v.as_i32() as i64,
                    3 => v.as_i64() as i64,
                    _ => 0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as i64,
                            2 => v2.as_i32() as i64,
                            3 => v2.as_i64() as i64,
                            _ => 0,
                        };
                        v_val == v2_val
                    }
                    4 | 5 => {
                        let v_val_temp = v_val as f64;
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val_temp == v2_val
                    }
                    _ => false,
                }
            }
            4 | 5 => {
                let v_val = match v.typ {
                    4 => v.as_f32() as f64,
                    5 => v.as_f64() as f64,
                    _ => 0.0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as f64,
                            2 => v2.as_i32() as f64,
                            3 => v2.as_i64() as f64,
                            _ => 0.0,
                        };
                        v_val == v2_val
                    }
                    4 | 5 => {
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val == v2_val
                    }
                    _ => false,
                }
            }
            6 => {
                let v_val = v.as_bool();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_bool();
                        v_val == v2_val
                    }
                    _ => false,
                }
            }
            7 => {
                let v_val = v.as_string();
                match v2.typ {
                    7 => {
                        let v2_val = v2.as_string();
                        v_val == v2_val
                    }
                    _ => false,
                }
            }
            8 => {
                let v_val = v.as_object();
                match v2.typ {
                    8 => {
                        let v2_val = v2.as_object();
                        if v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) && v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) {
                            return v_val.borrow().data.data.iter().all(|(k, d)| {
                                self.is_eq(
                                    d.clone(),
                                    v2_val.borrow().data.data.get(&k.clone()).unwrap().clone(),
                                )
                            });
                        }
                        false
                    }
                    _ => false,
                }
            }
            9 => {
                let v_val = v.as_array();
                match v2.typ {
                    9 => {
                        let v2_val = v2.as_array();
                        if v_val.borrow().data.len() != v2_val.borrow().data.len() {
                            return false;
                        }
                        let mut counter: usize = 0;
                        return v_val.borrow().data.iter().all(|d| {
                            if self.is_eq(
                                d.clone(),
                                v2_val.borrow().data.get(counter).unwrap().clone(),
                            ) {
                                counter += 1;
                                return true;
                            } else {
                                return false;
                            }
                        });
                    }
                    _ => false,
                }
            }
            10 => {
                let v_val = v.as_func();
                match v2.typ {
                    10 => {
                        let v2_val = v2.as_func();
                        v_val.borrow().start == v2_val.borrow().start
                            && v_val.borrow().end == v2_val.borrow().end
                    }
                    _ => false,
                }
            }
            _ => false,
        };
    }
    fn is_ge(&self, v: Val, v2: Val) -> bool {
        return match v.typ {
            1 | 2 | 3 => {
                let v_val = match v.typ {
                    1 => v.as_i16() as i64,
                    2 => v.as_i32() as i64,
                    3 => v.as_i64() as i64,
                    _ => 0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as i64,
                            2 => v2.as_i32() as i64,
                            3 => v2.as_i64() as i64,
                            _ => 0,
                        };
                        v_val > v2_val
                    }
                    4 | 5 => {
                        let v_val_temp = v_val as f64;
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val_temp > v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            4 | 5 => {
                let v_val = match v.typ {
                    4 => v.as_f32() as f64,
                    5 => v.as_f64() as f64,
                    _ => 0.0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as f64,
                            2 => v2.as_i32() as f64,
                            3 => v2.as_i64() as f64,
                            _ => 0.0,
                        };
                        v_val > v2_val
                    }
                    4 | 5 => {
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val > v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            6 => {
                let v_val = v.as_bool();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_bool();
                        v_val > v2_val
                    }
                    _ => panic!(
                        "elpian error: boolean and non boolean values are not comparable unless it is just equality check"
                    ),
                }
            }
            7 => {
                let v_val = v.as_string();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_string();
                        v_val > v2_val
                    }
                    _ => panic!(
                        "elpian error: string and non string values are not comparable unless it is just equality check"
                    ),
                }
            }
            8 => {
                let v_val = v.as_object();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_object();
                        if v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) && v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) {
                            let mut counter1 = 0;
                            let mut counter2 = 0;
                            v_val.borrow().data.data.iter().for_each(|(k, d)| {
                                if self.is_ge(
                                    d.clone(),
                                    v2_val.borrow().data.data.get(&k.clone()).unwrap().clone(),
                                ) {
                                    counter1 += 1;
                                } else {
                                    counter2 += 1;
                                }
                            });
                            return counter1 > counter2;
                        }
                        false
                    }
                    _ => panic!(
                        "elpian error: object and non object values are not comparable unless it is just equality check"
                    ),
                }
            }
            9 => {
                let v_val = v.as_array();
                match v2.typ {
                    9 => {
                        let v2_val = v2.as_array();
                        if v_val.borrow().data.len() != v2_val.borrow().data.len() {
                            return false;
                        }
                        let mut counter1 = 0;
                        let mut counter2 = 0;
                        let mut counter = 0;
                        v_val.borrow().data.iter().for_each(|d| {
                            if self.is_ge(
                                d.clone(),
                                v2_val.borrow().data.get(counter).unwrap().clone(),
                            ) {
                                counter1 += 1;
                            } else {
                                counter2 += 1;
                            }
                            counter += 1;
                        });
                        return counter1 > counter2;
                    }
                    _ => panic!(
                        "elpian error: array and non array values are not comparable unless it is just equality check"
                    ),
                }
            }
            10 => panic!(
                "elpian error: function types are not comparable unless it is just equality check"
            ),
            _ => panic!("elpian error: unknown types are not comparable"),
        };
    }
    fn is_gee(&self, v: Val, v2: Val) -> bool {
        return match v.typ {
            1 | 2 | 3 => {
                let v_val = match v.typ {
                    1 => v.as_i16() as i64,
                    2 => v.as_i32() as i64,
                    3 => v.as_i64() as i64,
                    _ => 0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as i64,
                            2 => v2.as_i32() as i64,
                            3 => v2.as_i64() as i64,
                            _ => 0,
                        };
                        v_val >= v2_val
                    }
                    4 | 5 => {
                        let v_val_temp = v_val as f64;
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val_temp >= v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            4 | 5 => {
                let v_val = match v.typ {
                    4 => v.as_f32() as f64,
                    5 => v.as_f64() as f64,
                    _ => 0.0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as f64,
                            2 => v2.as_i32() as f64,
                            3 => v2.as_i64() as f64,
                            _ => 0.0,
                        };
                        v_val >= v2_val
                    }
                    4 | 5 => {
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val >= v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            6 => {
                let v_val = v.as_bool();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_bool();
                        v_val >= v2_val
                    }
                    _ => panic!(
                        "elpian error: boolean and non boolean values are not comparable unless it is just equality check"
                    ),
                }
            }
            7 => {
                let v_val = v.as_string();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_string();
                        v_val >= v2_val
                    }
                    _ => panic!(
                        "elpian error: string and non string values are not comparable unless it is just equality check"
                    ),
                }
            }
            8 => {
                let v_val = v.as_object();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_object();
                        if v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) && v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) {
                            let mut counter1 = 0;
                            let mut counter2 = 0;
                            v_val.borrow().data.data.iter().for_each(|(k, d)| {
                                if self.is_gee(
                                    d.clone(),
                                    v2_val.borrow().data.data.get(&k.clone()).unwrap().clone(),
                                ) {
                                    counter1 += 1;
                                } else {
                                    counter2 += 1;
                                }
                            });
                            return counter1 >= counter2;
                        }
                        false
                    }
                    _ => panic!(
                        "elpian error: object and non object values are not comparable unless it is just equality check"
                    ),
                }
            }
            9 => {
                let v_val = v.as_array();
                match v2.typ {
                    9 => {
                        let v2_val = v2.as_array();
                        if v_val.borrow().data.len() != v2_val.borrow().data.len() {
                            return false;
                        }
                        let mut counter1 = 0;
                        let mut counter2 = 0;
                        let mut counter = 0;
                        v_val.borrow().data.iter().for_each(|d| {
                            if self.is_gee(
                                d.clone(),
                                v2_val.borrow().data.get(counter).unwrap().clone(),
                            ) {
                                counter1 += 1;
                            } else {
                                counter2 += 1;
                            }
                            counter += 1;
                        });
                        return counter1 >= counter2;
                    }
                    _ => panic!(
                        "elpian error: array and non array values are not comparable unless it is just equality check"
                    ),
                }
            }
            10 => panic!(
                "elpian error: function types are not comparable unless it is just equality check"
            ),
            _ => panic!("elpian error: unknown types are not comparable"),
        };
    }
    fn is_le(&self, v: Val, v2: Val) -> bool {
        return match v.typ {
            1 | 2 | 3 => {
                let v_val = match v.typ {
                    1 => v.as_i16() as i64,
                    2 => v.as_i32() as i64,
                    3 => v.as_i64() as i64,
                    _ => 0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as i64,
                            2 => v2.as_i32() as i64,
                            3 => v2.as_i64() as i64,
                            _ => 0,
                        };
                        v_val < v2_val
                    }
                    4 | 5 => {
                        let v_val_temp = v_val as f64;
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val_temp < v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            4 | 5 => {
                let v_val = match v.typ {
                    4 => v.as_f32() as f64,
                    5 => v.as_f64() as f64,
                    _ => 0.0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as f64,
                            2 => v2.as_i32() as f64,
                            3 => v2.as_i64() as f64,
                            _ => 0.0,
                        };
                        v_val < v2_val
                    }
                    4 | 5 => {
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val < v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            6 => {
                let v_val = v.as_bool();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_bool();
                        v_val < v2_val
                    }
                    _ => panic!(
                        "elpian error: boolean and non boolean values are not comparable unless it is just equality check"
                    ),
                }
            }
            7 => {
                let v_val = v.as_string();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_string();
                        v_val < v2_val
                    }
                    _ => panic!(
                        "elpian error: string and non string values are not comparable unless it is just equality check"
                    ),
                }
            }
            8 => {
                let v_val = v.as_object();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_object();
                        if v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) && v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) {
                            let mut counter1 = 0;
                            let mut counter2 = 0;
                            v_val.borrow().data.data.iter().for_each(|(k, d)| {
                                if self.is_le(
                                    d.clone(),
                                    v2_val.borrow().data.data.get(&k.clone()).unwrap().clone(),
                                ) {
                                    counter1 += 1;
                                } else {
                                    counter2 += 1;
                                }
                            });
                            return counter1 < counter2;
                        }
                        false
                    }
                    _ => panic!(
                        "elpian error: object and non object values are not comparable unless it is just equality check"
                    ),
                }
            }
            9 => {
                let v_val = v.as_array();
                match v2.typ {
                    9 => {
                        let v2_val = v2.as_array();
                        if v_val.borrow().data.len() != v2_val.borrow().data.len() {
                            return false;
                        }
                        let mut counter1 = 0;
                        let mut counter2 = 0;
                        let mut counter = 0;
                        v_val.borrow().data.iter().for_each(|d| {
                            if self.is_le(
                                d.clone(),
                                v2_val.borrow().data.get(counter).unwrap().clone(),
                            ) {
                                counter1 += 1;
                            } else {
                                counter2 += 1;
                            }
                            counter += 1;
                        });
                        return counter1 < counter2;
                    }
                    _ => panic!(
                        "elpian error: array and non array values are not comparable unless it is just equality check"
                    ),
                }
            }
            10 => panic!(
                "elpian error: function types are not comparable unless it is just equality check"
            ),
            _ => panic!("elpian error: unknown types are not comparable"),
        };
    }
    fn is_lee(&self, v: Val, v2: Val) -> bool {
        return match v.typ {
            1 | 2 | 3 => {
                let v_val = match v.typ {
                    1 => v.as_i16() as i64,
                    2 => v.as_i32() as i64,
                    3 => v.as_i64() as i64,
                    _ => 0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as i64,
                            2 => v2.as_i32() as i64,
                            3 => v2.as_i64() as i64,
                            _ => 0,
                        };
                        v_val <= v2_val
                    }
                    4 | 5 => {
                        let v_val_temp = v_val as f64;
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val_temp <= v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            4 | 5 => {
                let v_val = match v.typ {
                    4 => v.as_f32() as f64,
                    5 => v.as_f64() as f64,
                    _ => 0.0,
                };
                match v2.typ {
                    1 | 2 | 3 => {
                        let v2_val = match v2.typ {
                            1 => v2.as_i16() as f64,
                            2 => v2.as_i32() as f64,
                            3 => v2.as_i64() as f64,
                            _ => 0.0,
                        };
                        v_val <= v2_val
                    }
                    4 | 5 => {
                        let v2_val = match v2.typ {
                            4 => v2.as_f32() as f64,
                            5 => v2.as_f64() as f64,
                            _ => 0.0,
                        };
                        v_val <= v2_val
                    }
                    _ => panic!(
                        "elpian error: numerical and non numerical values are not comparable unless it is just equality check"
                    ),
                }
            }
            6 => {
                let v_val = v.as_bool();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_bool();
                        v_val <= v2_val
                    }
                    _ => panic!(
                        "elpian error: boolean and non boolean values are not comparable unless it is just equality check"
                    ),
                }
            }
            7 => {
                let v_val = v.as_string();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_string();
                        v_val <= v2_val
                    }
                    _ => panic!(
                        "elpian error: string and non string values are not comparable unless it is just equality check"
                    ),
                }
            }
            8 => {
                let v_val = v.as_object();
                match v2.typ {
                    6 => {
                        let v2_val = v2.as_object();
                        if v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) && v_val.borrow().data.data.iter().all(|(k, _d)| {
                            if !v2_val.borrow().data.data.contains_key(&k.clone()) {
                                return false;
                            }
                            true
                        }) {
                            let mut counter1 = 0;
                            let mut counter2 = 0;
                            v_val.borrow().data.data.iter().for_each(|(k, d)| {
                                if self.is_lee(
                                    d.clone(),
                                    v2_val.borrow().data.data.get(&k.clone()).unwrap().clone(),
                                ) {
                                    counter1 += 1;
                                } else {
                                    counter2 += 1;
                                }
                            });
                            return counter1 <= counter2;
                        }
                        false
                    }
                    _ => panic!(
                        "elpian error: object and non object values are not comparable unless it is just equality check"
                    ),
                }
            }
            9 => {
                let v_val = v.as_array();
                match v2.typ {
                    9 => {
                        let v2_val = v2.as_array();
                        if v_val.borrow().data.len() != v2_val.borrow().data.len() {
                            return false;
                        }
                        let mut counter1 = 0;
                        let mut counter2 = 0;
                        let mut counter = 0;
                        v_val.borrow().data.iter().for_each(|d| {
                            if self.is_lee(
                                d.clone(),
                                v2_val.borrow().data.get(counter).unwrap().clone(),
                            ) {
                                counter1 += 1;
                            } else {
                                counter2 += 1;
                            }
                            counter += 1;
                        });
                        return counter1 <= counter2;
                    }
                    _ => panic!(
                        "elpian error: array and non array values are not comparable unless it is just equality check"
                    ),
                }
            }
            10 => panic!(
                "elpian error: function types are not comparable unless it is just equality check"
            ),
            _ => panic!("elpian error: unknown types are not comparable"),
        };
    }
    fn define(&mut self, id_name: String, val: Val) {
        if let Err(e) = self.governor.charge_memory(val.approx_size()) {
            self.trap = Some(e.to_string());
        }
        self.ctx.define_val_globally(id_name, val);
    }
    fn assign(&mut self, id_name: String, val: Val) {
        if let Err(e) = self.governor.charge_memory(val.approx_size()) {
            self.trap = Some(e.to_string());
        }
        self.ctx.update_val_globally(id_name, val);
    }
    /// Pop the innermost scope, crediting the governor with the value-memory it
    /// held. This is the release half of the executor's approximate live-heap
    /// accounting: values are charged when bound (`define`/`assign`) and freed
    /// when their owning scope is torn down, so the tally tracks what the guest
    /// currently holds rather than everything it has ever allocated.
    fn pop_scope_governed(&mut self) {
        if let Some(scope) = self.ctx.memory.last() {
            let (bytes, is_func) = {
                let s = scope.borrow();
                let bytes: u64 = s.memory.borrow().data.values().map(|v| v.approx_size()).sum();
                (bytes, s.tag == "funcBody")
            };
            self.governor.release_memory(bytes);
            if is_func {
                self.governor.leave_call();
            }
        }
        self.ctx.pop_scope();
    }
    /// Snapshot the enclosing (non-global) locals as a closure's captured
    /// environment. Returns `None` at top level (nothing to close over), so
    /// plain functions pay no capture cost. Values are shared by `Rc`, so the
    /// closure keeps exactly its upvalues alive for as long as it lives.
    fn capture_env(&self) -> Option<Rc<RefCell<ValGroup>>> {
        if self.ctx.memory.len() <= 1 {
            return None;
        }
        let mut map: ValMap = ValMap::default();
        for scope in self.ctx.memory[1..].iter() {
            for (k, v) in scope.borrow().memory.borrow().data.iter() {
                map.insert(k.clone(), v.clone());
            }
        }
        if map.is_empty() {
            None
        } else {
            Some(Rc::new(RefCell::new(ValGroup::new(map))))
        }
    }
    /// Capture only the closure's *free variables* (computed by the compiler)
    /// from the enclosing non-global scopes — the innermost binding of each name
    /// wins, matching lexical resolution. This replaces snapshotting the entire
    /// scope chain: a closure pays only for the upvalues it actually uses, both
    /// to create and to seed on each call. Names not found in an enclosing scope
    /// (globals, or a closure's own not-yet-declared locals) are simply omitted
    /// and resolve normally at run time.
    fn capture_named(&self, names: &[String]) -> Option<Rc<RefCell<ValGroup>>> {
        if self.ctx.memory.len() <= 1 || names.is_empty() {
            return None;
        }
        let mut map: ValMap = ValMap::default();
        for name in names {
            for scope in self.ctx.memory[1..].iter().rev() {
                let found = scope.borrow().memory.borrow().data.get(name).cloned();
                if let Some(v) = found {
                    map.insert(name.clone(), v);
                    break;
                }
            }
        }
        if map.is_empty() {
            None
        } else {
            Some(Rc::new(RefCell::new(ValGroup::new(map))))
        }
    }
    /// Resolve a class method for `receiver.key` through the object's `__proto`
    /// chain (set by a `class` constructor), returning the method *bound* to the
    /// receiver. Binding reuses the closure mechanism: the shared top-level method
    /// function is cloned with a one-entry captured env `{ this: receiver }`, so
    /// the existing call path seeds `this` into the frame at no extra machinery —
    /// and, crucially, the method itself is never installed per instance. Returns
    /// `None` when `key` is not a method anywhere on the chain.
    fn bind_proto_method(&self, receiver: &Val, key: &str) -> Option<Val> {
        let mut proto = receiver
            .as_object()
            .borrow()
            .data
            .data
            .get("__proto")
            .cloned();
        while let Some(p) = proto {
            if p.typ != 8 {
                break;
            }
            let (entry, parent) = {
                let pb = p.as_object();
                let b = pb.borrow();
                (b.data.data.get(key).cloned(), b.data.data.get("__parent").cloned())
            };
            if let Some(m) = entry {
                if m.typ == 10 {
                    let bound = m.as_func().borrow().bind(receiver.clone());
                    return Some(Val { typ: 10, data: Payload::from(Rc::new(RefCell::new(bound))) });
                }
                return Some(m);
            }
            proto = parent;
        }
        None
    }
    pub fn run_from(
        &mut self,
        start: usize,
        end: usize,
        continue_exec: bool,
        host_call_result: Val,
        is_partial_exec: bool,
    ) -> Val {
        if !continue_exec {
            if !is_partial_exec {
                self.ctx
                    .push_scope("funcBody".to_string(), start, start, end);
            }
            self.pointer = start;
            self.end_at = end;
        } else {
            self.pending_func_result_value = host_call_result.clone();
        }
        let mut main_reg: Option<Val> = None;
        let mut is_reg_state_final = false;
        if continue_exec {
            if self.pending_func_result_value.typ != 254 {
                let returned_val = self.pending_func_result_value.clone();
                self.pending_func_result_value = Val {
                    typ: 254,
                    data: Payload::Null,
                };
                if !self.registers.is_empty() {
                    main_reg = Some(returned_val);
                    is_reg_state_final = false;
                }
            }
        }
        loop {
            // --- Host-driven lifecycle + resource governance (per step) ------
            // Checked at every step boundary so the host can pause, resume, or
            // terminate an instance, and so runaway work/memory is trapped long
            // before it can exhaust the real process.
            if self.trap.is_some() {
                self.control.confirm_terminated();
                self.registers.clear();
                break;
            }
            if self.control.should_suspend() {
                if self.control.is_terminating() {
                    self.control.confirm_terminated();
                    self.registers.clear();
                    break;
                } else {
                    self.control.confirm_paused();
                    self.paused_out = true;
                    break;
                }
            }
            if let Err(e) = self.governor.charge_instruction() {
                self.trap = Some(e.to_string());
                self.control.confirm_terminated();
                self.registers.clear();
                break;
            }
            if main_reg.is_some() {
                if !self.registers.is_empty() {
                    let op_type = self.registers.last().unwrap().get_type();
                    if op_type == OperationTypes::Dummy {
                        // A `DummyOp` is a called-function frame marker. A bare
                        // expression statement inside that body bubbles its value
                        // up to here; statement values are discarded (only an
                        // explicit `return` propagates), so drop it. Without this
                        // the stale value would be picked up by the next
                        // operation (e.g. a following `return`).
                        main_reg = None;
                        continue;
                    }
                    if op_type == OperationTypes::ArrExpr
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::ArrExprExtractInfo
                            || self.registers.last().unwrap().get_state()
                                == ExecStates::ArrExprExtractItem
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::ArrExprExtractItem,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::ArrExprFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::ObjExpr
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::ObjExprExtractInfo
                            || self.registers.last().unwrap().get_state()
                                == ExecStates::ObjExprExtractProp
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::ObjExprExtractProp,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::ObjExprFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::CallFunc
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::CallFuncStarted
                        {
                            // The callee just evaluated; the argument count is
                            // already stored in the operation (folded into the
                            // `Call` unit at decode).
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::CallFuncExtractFunc,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::CallFuncFinished;
                            continue;
                        } else if self.registers.last().unwrap().get_state()
                            == ExecStates::CallFuncExtractFunc
                            || self.registers.last().unwrap().get_state()
                                == ExecStates::CallFuncExtractParam
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::CallFuncExtractParam,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::CallFuncFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::ReturnVal
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::ReturnValStarted
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::ReturnValFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::ReturnValFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::DefineVar
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::DefineVarExtractName
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::DefineVarExtractValue,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::DefineVarExtractValue;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::AssignVar
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::AssignVarExtractName
                        {
                            if self.registers.last().unwrap().get_data()[1].as_i16() == 1 {
                                self.registers.last_mut().unwrap().set_state(
                                    ExecStates::AssignVarExtractValue,
                                    StateData::Val(main_reg.take().unwrap()),
                                );
                                main_reg = None;
                                is_reg_state_final =
                                    self.registers.last().unwrap().get_state()
                                        == ExecStates::AssignVarExtractValue;
                                continue;
                            } else if self.registers.last().unwrap().get_data()[1].as_i16()
                                == 2
                            {
                                self.registers.last_mut().unwrap().set_state(
                                    ExecStates::AssignVarExtractIndex,
                                    StateData::Val(main_reg.take().unwrap()),
                                );
                                main_reg = None;
                                is_reg_state_final =
                                    self.registers.last().unwrap().get_state()
                                        == ExecStates::AssignVarExtractValue;
                                continue;
                            }
                        } else if self.registers.last().unwrap().get_state()
                            == ExecStates::AssignVarExtractIndex
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::AssignVarExtractValue,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::AssignVarExtractValue;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::IfStmt
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::IfStmtIsConditioned
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::IfStmtFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::IfStmtFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::LoopStmt
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::LoopStmtStarted
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::LoopStmtFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::LoopStmtFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::SwitchStmt
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::SwitchStmtStarted
                        {
                            // The switch value just evaluated; the branch-after and
                            // case table are already stored in the operation
                            // (folded into the `Switch` unit at decode).
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::SwitchStmtExtractVal,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::SwitchStmtFinished;
                            continue;
                        } else if self.registers.last().unwrap().get_state()
                            == ExecStates::SwitchStmtExtractVal
                            || self.registers.last().unwrap().get_state()
                                == ExecStates::SwitchStmtExtractCase
                        {
                            // A case value just evaluated. Its body range is the
                            // next entry in the operation's case table; read the
                            // end before recording the case so we can skip the body.
                            let (_, branch_true_end) =
                                self.registers.last().unwrap().next_case_bounds();
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::SwitchStmtExtractCase,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::SwitchStmtFinished;
                            // Skip past this case's body to the next case's value
                            // expression. Without this the scan would fall into
                            // the body and execute it while still collecting
                            // cases. Once every case is collected the dispatch
                            // (SwitchStmtFinished) sets the pointer itself, so the
                            // value parked here is only used between cases.
                            self.pointer = branch_true_end;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::Arithmetic
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::ArithmeticExtractOp
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::ArithmeticExtractArg1,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::ArithmeticExtractArg2;
                            continue;
                        } else if self.registers.last().unwrap().get_state()
                            == ExecStates::ArithmeticExtractArg1
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::ArithmeticExtractArg2,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::ArithmeticExtractArg2;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::Indexer
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::IndexerStarted
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::IndexerExtractVarName,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::IndexerExtractIndex;
                            continue;
                        } else if self.registers.last().unwrap().get_state()
                            == ExecStates::IndexerExtractVarName
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::IndexerExtractIndex,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::IndexerExtractIndex;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::NotVal
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::NotValStarted
                        {
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::NotValFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::NotValFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::CondBrch
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::CondBranchStarted
                        {
                            // The condition just evaluated; both targets are
                            // already stored in the operation (folded into the
                            // `CondBranch` unit at decode).
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::CondBranchFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::CondBranchFinished;
                            continue;
                        }
                    } else if op_type
                        == OperationTypes::CastOprt
                    {
                        if self.registers.last().unwrap().get_state()
                            == ExecStates::CastOprtStarted
                        {
                            // The value just evaluated; the target type is already
                            // stored in the operation (folded into the `Cast` unit).
                            self.registers.last_mut().unwrap().set_state(
                                ExecStates::CastOprtFinished,
                                StateData::Val(main_reg.take().unwrap()),
                            );
                            main_reg = None;
                            is_reg_state_final =
                                self.registers.last().unwrap().get_state()
                                    == ExecStates::CastOprtFinished;
                            continue;
                        }
                    } else if op_type == OperationTypes::Logical {
                        let state = self.registers.last().unwrap().get_state();
                        if state == ExecStates::LogicalExtractOp1 {
                            // The left operand just evaluated. Decide whether the
                            // result is settled (`&&` falsy / `||` truthy → reuse it
                            // and skip the right operand) or the right operand must
                            // be evaluated (`&&` truthy / `||` falsy).
                            let data = self.registers.last().unwrap().get_data();
                            let is_or = data[0].as_bool();
                            let op2_end = data[1].as_i64() as usize;
                            let left = main_reg.take().unwrap();
                            let evaluate_right = if is_or { !left.truthy() } else { left.truthy() };
                            if evaluate_right {
                                self.registers
                                    .last_mut()
                                    .unwrap()
                                    .set_state(ExecStates::LogicalExtractOp2, StateData::Empty);
                                main_reg = None;
                                is_reg_state_final = false;
                                // Fall through into the right operand's units.
                                continue;
                            } else {
                                self.registers.pop();
                                self.pointer = op2_end; // skip the right operand
                                main_reg = Some(left);
                                is_reg_state_final = false;
                                continue;
                            }
                        } else if state == ExecStates::LogicalExtractOp2 {
                            // The right operand just evaluated and is the result.
                            let right = main_reg.take().unwrap();
                            self.registers.pop();
                            main_reg = Some(right);
                            is_reg_state_final = false;
                            continue;
                        }
                    } else if op_type == OperationTypes::Conditional {
                        let state = self.registers.last().unwrap().get_state();
                        if state == ExecStates::CondExprExtractCond {
                            // The condition just evaluated. A truthy condition lets
                            // execution fall into the consequent (which immediately
                            // follows); otherwise jump to the alternate.
                            let data = self.registers.last().unwrap().get_data();
                            let alt_start = data[0].as_i64() as usize;
                            let cond = main_reg.take().unwrap();
                            if !cond.truthy() {
                                self.pointer = alt_start;
                            }
                            self.registers
                                .last_mut()
                                .unwrap()
                                .set_state(ExecStates::CondExprExtractValue, StateData::Empty);
                            main_reg = None;
                            is_reg_state_final = false;
                            continue;
                        } else if state == ExecStates::CondExprExtractValue {
                            // The taken branch's value is the result; skip past the
                            // other branch.
                            let data = self.registers.last().unwrap().get_data();
                            let end = data[1].as_i64() as usize;
                            let val = main_reg.take().unwrap();
                            self.registers.pop();
                            self.pointer = end;
                            main_reg = Some(val);
                            is_reg_state_final = false;
                            continue;
                        }
                    }
                } else {
                    main_reg = None;
                }
            } else if is_reg_state_final {
                if !self.registers.is_empty() {
                    if self.registers.last().unwrap().get_state()
                        == ExecStates::ArrExprFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let items_vec = regs[1].clone();
                        self.registers.pop();
                        main_reg = Some(items_vec);
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::ObjExprFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let typ_id = regs[0].as_i64();
                        let props_vec = regs[2].as_array();
                        let mut props_map = ValMap::default();
                        for i in (0..props_vec.borrow().data.len()).step_by(2) {
                            props_map.insert(
                                props_vec.borrow().data[i].as_string(),
                                props_vec.borrow().data[i + 1].clone(),
                            );
                        }
                        let result = Val {
                            typ: 8,
                            data: Payload::from(Rc::new(RefCell::new(
                                Object::new(typ_id, ValGroup::new(props_map)),
                            ))),
                        };
                        self.registers.pop();
                        main_reg = Some(result);
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::CallFuncFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let is_native = regs[1].as_bool();
                        if !is_native {
                            let func = regs[0].as_func().clone();
                            // Guard native-stack exhaustion via the call-depth
                            // budget before entering the new frame.
                            if let Err(e) = self.governor.enter_call() {
                                self.trap = Some(e.to_string());
                                continue;
                            }
                            let expected_params = func.borrow().params.clone();
                            let provided_args = regs[3].as_array().borrow().data.clone();
                            let mut args = ValMap::default();
                            // Seed the frame with the closure's captured upvalues
                            // first, so explicit parameters override them.
                            if let Some(captured) = func.borrow().captured.clone() {
                                for (k, v) in captured.borrow().data.iter() {
                                    args.insert(k.clone(), v.clone());
                                }
                            }
                            // A bound method receives its receiver as `this`.
                            if let Some(receiver) = func.borrow().this_arg.clone() {
                                args.insert("this".to_string(), receiver);
                            }
                            for (i, param_name) in expected_params.iter().enumerate() {
                                let arg = provided_args.get(i).cloned().unwrap_or_else(|| {
                                    Val::new(0, Payload::Null)
                                });
                                args.insert(param_name.clone(), arg);
                            }
                            self.ctx
                                .memory
                                .last()
                                .unwrap()
                                .borrow_mut()
                                .update_frozen_pointer(self.pointer);
                            self.ctx.push_scope_with_args(
                                "funcBody".to_string(),
                                func.borrow().start,
                                func.borrow().start,
                                func.borrow().end,
                                args,
                            );
                            self.pointer = func.borrow().start;
                            self.end_at = func.borrow().end;
                            self.registers.pop();
                            self.registers
                                .push(Box::new(DummyOp::new()));
                            is_reg_state_final = false;
                            continue;
                        } else {
                            // A native call: either a standard-library builtin
                            // (named function, typ 252) or the `askHost` seam
                            // (unnamed, typ 255). The two construction sites are the
                            // only producers of a native function value, so a
                            // non-empty name unambiguously means "builtin" — no need
                            // to re-scan the builtin table here (it ran already at
                            // resolve time). We dispatch straight off the borrowed
                            // name and the borrowed argument slice, cloning neither
                            // the name `String` nor the argument `Vec` on this path.
                            let func = regs[0].as_func();
                            let is_builtin_call = !func.borrow().name.is_empty();
                            if is_builtin_call {
                                let func_ref = func.borrow();
                                let arg_arr = regs[3].as_array();
                                let arg_ref = arg_arr.borrow();
                                let outcome = stdlib::invoke(&func_ref.name, &arg_ref.data);
                                match outcome {
                                    Ok(result) => {
                                        drop(arg_ref);
                                        drop(func_ref);
                                        self.registers.pop();
                                        let _ = self.governor.charge_memory(result.approx_size());
                                        main_reg = Some(result);
                                        is_reg_state_final = false;
                                        continue;
                                    }
                                    Err(e) => {
                                        self.trap = Some(format!("{}: {e}", func_ref.name));
                                        continue;
                                    }
                                }
                            }
                            let arg1 = regs[3].as_array().borrow().data[0].clone();
                            let api_name = arg1.as_string();
                            // Capability gate: if the host has switched this
                            // interface off, the call does not reach the host —
                            // it short-circuits to a typed null so the guest
                            // keeps running deterministically.
                            if !self.capabilities.allows_api(&api_name) {
                                self.registers.pop();
                                main_reg = Some(Val::new(0, Payload::Null));
                                is_reg_state_final = false;
                                continue;
                            }
                            let arg2 = regs[3].as_array().borrow().data[1].clone();
                            self.cb_counter += 1;
                            let cb_id = self.cb_counter;
                            self.registers.pop();
                            self.reserved_host_call = Some((
                                0x02,
                                cb_id,
                                Val {
                                    typ: 9,
                                    data: Payload::from(Rc::new(RefCell::new(
                                        Array::new(vec![
                                            arg1,
                                            Val {
                                                typ: 1,
                                                data: Payload::from(self.executor_id),
                                            },
                                            arg2,
                                        ]),
                                    ))),
                                },
                            ));
                            break;
                        }
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::ReturnValFinished
                    {
                        let data = self.registers.last().unwrap().get_data();
                        let returned_val = data[0].clone();
                        self.registers.pop();
                        // A `return` exits the whole function, not just the block
                        // (if / loop / switch) it textually sits in. Unwind any
                        // such intervening scopes so the enclosing function-body
                        // frame is innermost, then jump to its end and let the
                        // normal scope teardown deliver the value — making every
                        // return behave like a top-level return. The outermost
                        // scope (the top-level program) is itself tagged
                        // "funcBody" but must never be unwound, so the length
                        // guard keeps it in place (a top-level `return` simply
                        // ends the run).
                        while self.ctx.memory.len() > 1
                            && self.ctx.memory.last().unwrap().borrow().tag != "funcBody"
                        {
                            self.pop_scope_governed();
                        }
                        let func_end = self.ctx.memory.last().unwrap().borrow().frozen_end;
                        self.pointer = func_end;
                        self.end_at = func_end;
                        self.pending_func_result_value = returned_val;
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::DefineVarExtractValue
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let var_name = regs[0].as_string();
                        let var_value = regs[1].clone();
                        self.registers.pop();
                        self.define(var_name.clone(), var_value.clone());
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::AssignVarExtractValue
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let var_name = regs[0].as_string();
                        let assign_target_type = regs[1].as_i16();
                        let data = regs[3].clone();
                        if assign_target_type == 1 {
                            self.assign(var_name.clone(), data);
                        } else if assign_target_type == 2 {
                            let index = regs[2].clone();
                            let indexed = self.ctx.find_val_globally(&var_name);
                            if index.typ == 7 {
                                if indexed.typ == 8 {
                                    let obj = indexed.as_object();
                                    obj.borrow_mut().data.data.insert(index.as_string(), data);
                                } else {
                                    panic!(
                                    "elpian error: non object value can not be indexed by string"
                                );
                                }
                            } else if index.typ >= 1 && index.typ <= 3 {
                                if indexed.typ == 9 {
                                    let sidx = match index.typ {
                                        1 => index.as_i16() as i64,
                                        2 => index.as_i32() as i64,
                                        _ => index.as_i64(),
                                    };
                                    if sidx < 0 {
                                        panic!("elpian error: negative array index");
                                    }
                                    let idx = sidx as usize;
                                    let arr = indexed.as_array();
                                    let mut b = arr.borrow_mut();
                                    // JS semantics: assigning at or past the end grows
                                    // the array, filling the gap with null (e.g.
                                    // `var out = []; out[i] = v;`).
                                    if idx >= b.data.len() {
                                        b.data.resize(idx + 1, Val { typ: 0, data: Payload::Null });
                                    }
                                    b.data[idx] = data;
                                } else {
                                    panic!(
                                    "elpian error: non object value can not be indexed by string"
                                );
                                }
                            } else {
                                panic!(
                                "elpian error: types other than integer and string can not be used to index anything"
                            );
                            }
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::IfStmtFinished
                    {
                        // The branch targets are part of the operation (folded into
                        // the `IfHead` unit at decode): regs = [has_condition,
                        // condition, body_start, body_end, next, branch_after].
                        let regs = self.registers.last().unwrap().get_data();
                        let has_condition = regs[0].as_bool();
                        let cond_val = regs[1].clone();
                        let branch_true_start = regs[2].as_i64() as usize;
                        let branch_true_end = regs[3].as_i64() as usize;
                        let branch_next_start = regs[4].as_i64() as usize;
                        let branch_after_start = regs[5].as_i64() as usize;
                        let mut condition = false;
                        if has_condition {
                            // JS truthiness — any non-falsy value (object, number,
                            // non-empty string, …) takes the branch, not just `true`.
                            condition = cond_val.truthy();
                        }
                        if !has_condition {
                            self.ctx
                                .memory
                                .last()
                                .unwrap()
                                .borrow_mut()
                                .update_frozen_pointer(branch_after_start);
                            self.ctx.push_scope(
                                "ifBody".to_string(),
                                branch_true_start,
                                branch_true_start,
                                branch_true_end,
                            );
                            self.pointer = branch_true_start;
                            self.end_at = branch_true_end;
                        } else {
                            if condition {
                                self.ctx
                                    .memory
                                    .last()
                                    .unwrap()
                                    .borrow_mut()
                                    .update_frozen_pointer(branch_after_start);
                                self.ctx.push_scope(
                                    "ifBody".to_string(),
                                    branch_true_start,
                                    branch_true_start,
                                    branch_true_end,
                                );
                                self.pointer = branch_true_start;
                                self.end_at = branch_true_end;
                            } else {
                                self.pointer = branch_next_start;
                            }
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::LoopStmtFinished
                    {
                        // The loop bounds are part of the operation (folded into the
                        // `Loop` unit at decode): regs = [condition, body_start,
                        // body_end, branch_after].
                        let regs = self.registers.last().unwrap().get_data();
                        let cond_val = regs[0].clone();
                        // JS truthiness for the loop guard (see if-statement above).
                        let condition = cond_val.truthy();
                        let branch_true_start = regs[1].as_i64() as usize;
                        let branch_true_end = regs[2].as_i64() as usize;
                        let branch_after_start = regs[3].as_i64() as usize;
                        if condition {
                            // A loop re-evaluates its `LoopStmt` unit while still
                            // *inside* the previous iteration's body scope: the body's
                            // final instruction jumps back to the loop unit (see the
                            // compiler's `loopStmt` emission), so the spent `loopBody`
                            // scope is still on top here. Reclaim it before opening a
                            // fresh one, so only ever **one** body scope is live and an
                            // N-iteration loop stays O(N) — otherwise one empty scope
                            // leaks per iteration, every variable lookup then walks an
                            // ever-deeper chain (`find_val_globally`/`update_val_globally`),
                            // and the loop degrades to O(N^2) time and O(N) memory
                            // (reclaimed only when the whole function returns). Match by
                            // tag **and** the body-start it was opened at, so nested
                            // loops reclaim only their own bodies and the loop's first
                            // entry (top scope is the enclosing frame, not a matching
                            // `loopBody`) is left untouched. Closures created in the
                            // body keep their captured environment alive through their
                            // own `Rc`, so popping it from the active scope stack does
                            // not disturb per-iteration captures. The exit path
                            // (condition false) deliberately does **not** pre-pop: the
                            // teardown cascade below reclaims the final body scope.
                            let reentered_body = self
                                .ctx
                                .memory
                                .last()
                                .map(|s| {
                                    let s = s.borrow();
                                    s.tag == "loopBody" && s.frozen_start == branch_true_start
                                })
                                .unwrap_or(false);
                            if reentered_body {
                                self.pop_scope_governed();
                            }
                            self.ctx
                                .memory
                                .last()
                                .unwrap()
                                .borrow_mut()
                                .update_frozen_pointer(branch_after_start);
                            self.ctx.push_scope(
                                "loopBody".to_string(),
                                branch_true_start,
                                branch_true_start,
                                branch_true_end,
                            );
                            self.pointer = branch_true_start;
                            self.end_at = branch_true_end;
                        } else {
                            self.pointer = branch_after_start;
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::SwitchStmtFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let comparing_val = regs[0].clone();
                        let branch_after_start = regs[1].as_i64() as usize;
                        let cases = regs[3].as_array();
                        let mut matched = false;
                        for case_info in cases.borrow().data.iter() {
                            let data = case_info.as_object().borrow().data.data.clone();
                            let case_val = data["val"].clone();
                            let branch_true_start = data["start"].as_i64() as usize;
                            let branch_true_end = data["end"].as_i64() as usize;
                            if self.is_eq(comparing_val.clone(), case_val) {
                                matched = true;
                                self.ctx
                                    .memory
                                    .last()
                                    .unwrap()
                                    .borrow_mut()
                                    .update_frozen_pointer(branch_after_start);
                                self.ctx.push_scope(
                                    "switchBody".to_string(),
                                    branch_true_start,
                                    branch_true_start,
                                    branch_true_end,
                                );
                                self.pointer = branch_true_start;
                                self.end_at = branch_true_end;
                            }
                        }
                        if !matched {
                            self.pointer = branch_after_start;
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::ArithmeticExtractArg2
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let op = regs[0].as_i16();
                        let arg1 = regs[1].clone();
                        let arg2 = regs[2].clone();
                        self.registers.pop();
                        match op {
                            1 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(self.is_eq(arg1, arg2)),
                                });
                            }
                            2 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(self.is_ge(arg1, arg2)),
                                });
                            }
                            3 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(self.is_gee(arg1, arg2)),
                                });
                            }
                            4 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(self.is_le(arg1, arg2)),
                                });
                            }
                            5 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(self.is_lee(arg1, arg2)),
                                });
                            }
                            6 => {
                                main_reg = Some(Val {
                                    typ: 6,
                                    data: Payload::from(!self.is_eq(arg1, arg2)),
                                });
                            }
                            7 => {
                                main_reg = Some(self.operate_sum(arg1, arg2));
                            }
                            8 => {
                                main_reg = Some(self.operate_subtract(arg1, arg2));
                            }
                            9 => {
                                main_reg = Some(self.operate_multiply(arg1, arg2));
                            }
                            10 => {
                                main_reg = Some(self.operate_division(arg1, arg2));
                            }
                            11 => {
                                main_reg = Some(self.operate_modulo(arg1, arg2));
                            }
                            12 => {
                                main_reg = Some(self.operate_power(arg1, arg2));
                            }
                            _ => {}
                        }
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::IndexerExtractIndex
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let indexed = regs[0].clone();
                        let index = regs[1].clone();
                        self.registers.pop();
                        if index.typ == 7 {
                            if indexed.typ == 8 {
                                let key = index.as_string();
                                let own = indexed.as_object().borrow().data.data.get(&key).cloned();
                                if let Some(o) = own {
                                    main_reg = Some(o);
                                } else if let Some(bound) = self.bind_proto_method(&indexed, &key) {
                                    // Not an own field: a class method, bound to the
                                    // receiver, so `obj.method(args)` runs with `this`.
                                    main_reg = Some(bound);
                                } else {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            } else {
                                eprintln!(
                                    "elpian error: non object value can not be indexed by string"
                                );
                                main_reg = Some(Val {
                                    typ: 0,
                                    data: Payload::Null,
                                });
                            }
                        } else if index.typ >= 1 && index.typ <= 3 {
                            if indexed.typ == 9 {
                                let arr = indexed.as_array();
                                if index.typ == 1 {
                                    if let Some(o) =
                                        arr.borrow().data.get(index.as_i16() as usize).clone()
                                    {
                                        main_reg = Some(o.clone());
                                    } else {
                                        main_reg = Some(Val {
                                            typ: 0,
                                            data: Payload::Null,
                                        });
                                    }
                                } else if index.typ == 2 {
                                    if let Some(o) =
                                        arr.borrow().data.get(index.as_i32() as usize).clone()
                                    {
                                        main_reg = Some(o.clone());
                                    } else {
                                        main_reg = Some(Val {
                                            typ: 0,
                                            data: Payload::Null,
                                        });
                                    }
                                } else {
                                    if let Some(o) =
                                        arr.borrow().data.get(index.as_i64() as usize).clone()
                                    {
                                        main_reg = Some(o.clone());
                                    } else {
                                        main_reg = Some(Val {
                                            typ: 0,
                                            data: Payload::Null,
                                        });
                                    }
                                }
                            } else {
                                eprintln!(
                                    "elpian error: non object value can not be indexed by string"
                                );
                                main_reg = Some(Val {
                                    typ: 0,
                                    data: Payload::Null,
                                });
                            }
                        } else {
                            eprintln!(
                            "elpian error: types other than integer and string can not be used to index anything"
                        );
                            main_reg = Some(Val {
                                typ: 0,
                                data: Payload::Null,
                            });
                        }
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::NotValFinished
                    {
                        let data = self.registers.last().unwrap().get_data();
                        let val = data[0].clone();
                        self.registers.pop();
                        // `!x` is the boolean negation of JS truthiness, defined for
                        // every value (not just booleans).
                        main_reg = Some(Val {
                            typ: 6,
                            data: Payload::from(!val.truthy()),
                        });
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::CondBranchFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let condition = regs[0].truthy();
                        let branch_true_start = regs[1].as_i64() as usize;
                        let branch_false_start = regs[2].as_i64() as usize;
                        if condition {
                            self.pointer = branch_true_start;
                        } else {
                            self.pointer = branch_false_start;
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    } else if self.registers.last().unwrap().get_state()
                        == ExecStates::CastOprtFinished
                    {
                        let regs = self.registers.last().unwrap().get_data();
                        let data = regs[0].clone();
                        let target_type = regs[1].as_string();
                        if target_type == "i16" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(data.as_i16() as i16),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(data.as_i32() as i16),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(data.as_i64() as i16),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(data.as_f32() as i16),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(data.as_f64() as i16),
                                    });
                                }
                                6 => {
                                    main_reg =
                                        Some(Val {
                                            typ: 1,
                                            data: Payload::from(
                                                if data.as_bool() { 1 } else { 0 } as i16,
                                            ),
                                        });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 1,
                                        data: Payload::from(
                                            data.as_string().parse::<i16>().unwrap(),
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "i32" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(data.as_i16() as i32),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(data.as_i32() as i32),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(data.as_i64() as i32),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(data.as_f32() as i32),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(data.as_f64() as i32),
                                    });
                                }
                                6 => {
                                    main_reg =
                                        Some(Val {
                                            typ: 2,
                                            data: Payload::from(
                                                if data.as_bool() { 1 } else { 0 } as i32,
                                            ),
                                        });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 2,
                                        data: Payload::from(
                                            data.as_string().parse::<i32>().unwrap(),
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "i64" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(data.as_i16() as i64),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(data.as_i32() as i64),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(data.as_i64() as i64),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(data.as_f32() as i64),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(data.as_f64() as i64),
                                    });
                                }
                                6 => {
                                    main_reg =
                                        Some(Val {
                                            typ: 3,
                                            data: Payload::from(
                                                if data.as_bool() { 1 } else { 0 } as i64,
                                            ),
                                        });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 3,
                                        data: Payload::from(
                                            data.as_string().parse::<i64>().unwrap(),
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "f32" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(data.as_i16() as f32),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(data.as_i32() as f32),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(data.as_i64() as f32),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(data.as_f32() as f32),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(data.as_f64() as f32),
                                    });
                                }
                                6 => {
                                    main_reg =
                                        Some(Val {
                                            typ: 4,
                                            data: Payload::from(
                                                if data.as_bool() { 1 } else { 0 } as f32,
                                            ),
                                        });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 4,
                                        data: Payload::from(
                                            data.as_string().parse::<f32>().unwrap(),
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "f64" || target_type == "number" {
                            // `number` is the JavaScript numeric type, aliased onto
                            // the f64 representation.
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(data.as_i16() as f64),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(data.as_i32() as f64),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(data.as_i64() as f64),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(data.as_f32() as f64),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(data.as_f64() as f64),
                                    });
                                }
                                6 => {
                                    main_reg =
                                        Some(Val {
                                            typ: 5,
                                            data: Payload::from(
                                                if data.as_bool() { 1 } else { 0 } as f64,
                                            ),
                                        });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 5,
                                        data: Payload::from(
                                            data.as_string().parse::<f64>().unwrap(),
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "bool" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_i16() > 0),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_i32() > 0),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_i64() > 0),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_f32() > 0.0),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_f64() > 0.0),
                                    });
                                }
                                6 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(data.as_bool()),
                                    });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 6,
                                        data: Payload::from(
                                            data.as_string() == "true",
                                        ),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        } else if target_type == "string" {
                            match data.typ {
                                1 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_i16().to_string(),
                                        ),
                                    });
                                }
                                2 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_i32().to_string(),
                                        ),
                                    });
                                }
                                3 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_i64().to_string(),
                                        ),
                                    });
                                }
                                4 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_f32().to_string(),
                                        ),
                                    });
                                }
                                5 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_f64().to_string(),
                                        ),
                                    });
                                }
                                6 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(
                                            data.as_bool().to_string(),
                                        ),
                                    });
                                }
                                7 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(data.as_string()),
                                    });
                                }
                                8 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(data.stringify()),
                                    });
                                }
                                9 => {
                                    main_reg = Some(Val {
                                        typ: 7,
                                        data: Payload::from(data.stringify()),
                                    });
                                }
                                _ => {
                                    main_reg = Some(Val {
                                        typ: 0,
                                        data: Payload::Null,
                                    });
                                }
                            }
                        }
                        self.registers.pop();
                        is_reg_state_final = false;
                        continue;
                    }
                } else {
                    main_reg = None;
                }
            }
            let mut terminate = false;
            if self.pointer == self.end_at {
                while self.pointer == self.end_at {
                    if self.ctx.memory.len() == 1 {
                        terminate = true;
                        break;
                    }
                    // Only a *function-body* frame owns a `DummyOp` register (pushed
                    // at call dispatch). Control-flow bodies (`ifBody`/`loopBody`/
                    // `switchBody`) are plain scopes with no register of their own, so
                    // their teardown must NOT pop the enclosing function's `DummyOp`
                    // — doing so would unbalance the register stack and let a
                    // statement after the block leak its value into the caller's
                    // awaiting expression (the bug behind "array used as object key"
                    // traps and corrupted returns in larger programs).
                    let popped_tag = self.ctx.memory.last().unwrap().borrow().tag.clone();
                    self.pop_scope_governed();
                    if is_partial_exec && (self.ctx.memory.len() == 1) {
                        return self.pending_func_result_value.clone();
                    }
                    if popped_tag == "funcBody"
                        && !self.registers.is_empty()
                        && self.registers.last().unwrap().get_type()
                            == OperationTypes::Dummy
                    {
                        self.registers.pop();
                    }
                    if !self.ctx.memory.is_empty() {
                        self.pointer = self.ctx.memory.last().unwrap().borrow().frozen_pointer;
                        self.end_at = self.ctx.memory.last().unwrap().borrow().frozen_end;
                        if self.pending_func_result_value.typ != 254 {
                            // A `return` is propagating to a caller. The callee's
                            // own nested scopes were already unwound at the point
                            // of return, so here we only hand the value to the
                            // caller's awaiting expression register (an in-program
                            // call). The caller's scope stack is left untouched —
                            // it may legitimately sit inside its own control block.
                            let returned_val = self.pending_func_result_value.clone();
                            self.pending_func_result_value = Val {
                                typ: 254,
                                data: Payload::Null,
                            };
                            if !self.registers.is_empty() {
                                main_reg = Some(returned_val);
                                is_reg_state_final = false;
                                break;
                            }
                        }
                    } else {
                        terminate = true;
                        break;
                    }
                }
                if terminate {
                    break;
                }
                continue;
            }
            // Fetch the pre-decoded operation at the program counter and advance
            // to the next unit (control-flow arms below override the pointer
            // afterwards). The bytecode is never re-parsed: every operand was
            // decoded once into the unit, and branch targets are unit indices
            // (see `program.rs`).
            let kind = self.prog.units[self.pointer].clone();
            self.pointer += 1;
            match kind {
                // ----------------------------------
                // arithmetic / comparison operators (op id 1..=12)
                UnitKind::Arith(op_id) => {
                    self.registers.push(Box::new(Arithmetic::new()));
                    self.registers
                        .last_mut()
                        .unwrap()
                        .set_state(ExecStates::ArithmeticExtractOp, StateData::I16(op_id));
                }
                // not operator
                UnitKind::Not => {
                    self.registers.push(Box::new(NotValue::new()));
                }
                // short-circuiting logical && / ||
                UnitKind::Logical { is_or, op2_end } => {
                    self.registers.push(Box::new(LogicalOp::new(is_or, op2_end)));
                }
                // conditional / ternary expression
                UnitKind::Conditional { alt_start, end } => {
                    self.registers.push(Box::new(ConditionalOp::new(alt_start, end)));
                }
                // cast operation (target type folded into the unit)
                UnitKind::Cast { target_type } => {
                    self.registers.push(Box::new(CastOp::new(target_type.to_string())));
                }
                // ----------------------------------
                // program operators:
                // data indexer
                UnitKind::Indexer => {
                    self.registers.push(Box::new(IndexerValue::new()));
                }
                // function call (argument count folded into the unit)
                UnitKind::Call { argc } => {
                    self.registers.push(Box::new(CallFunction::new(argc as i32)));
                }
                // definition statement (name pre-decoded; value expression follows)
                UnitKind::DefineVar(name) => {
                    self.registers.push(Box::new(DefineVariable::new()));
                    self.registers.last_mut().unwrap().set_state(
                        ExecStates::DefineVarExtractName,
                        StateData::Str(name.to_string()),
                    );
                }
                // assignment statement (target name + kind pre-decoded)
                UnitKind::AssignVar { name, kind } => {
                    self.registers.push(Box::new(AssignVariable::new()));
                    self.registers.last_mut().unwrap().set_state(
                        ExecStates::AssignVarExtractName,
                        StateData::StrI16(name.to_string(), kind),
                    );
                }
                // if statement (one arm of an if/else chain; targets folded in)
                UnitKind::IfHead { has_condition, body_start, body_end, next, branch_after } => {
                    self.registers.push(Box::new(IfStmt::new(
                        has_condition,
                        body_start,
                        body_end,
                        next,
                        branch_after,
                    )));
                    if !has_condition {
                        // The unconditional `else` arm is already decided (the
                        // operation starts finished); run its finalizer next step.
                        main_reg = None;
                        is_reg_state_final = true;
                        continue;
                    }
                }
                // loop statement (bounds folded into the unit)
                UnitKind::Loop { body_start, body_end, branch_after } => {
                    self.registers
                        .push(Box::new(LoopStmt::new(body_start, body_end, branch_after)));
                }
                // switch case statement (branch-after + case table folded in)
                UnitKind::Switch { branch_after, cases } => {
                    self.registers.push(Box::new(SwitchStmt::new(branch_after, cases)));
                }
                // function definition (header pre-decoded; body skipped here)
                UnitKind::FuncDef { name, params, frees, start, end } => {
                    let mut func =
                        Function::new(name.to_string(), start, end, (*params).clone());
                    // A function defined inside another function closes over the
                    // enclosing locals it uses (e.g. a factory returning a
                    // counter). Capture just those free variables; at top level
                    // there is nothing to capture and this is a no-op.
                    func.captured = self.capture_named(&frees);
                    self.define(
                        name.to_string(),
                        Val {
                            typ: 10,
                            data: Payload::from(Rc::new(RefCell::new(func))),
                        },
                    );
                    self.pointer = end;
                }
                // return command
                UnitKind::Return => {
                    self.registers.push(Box::new(ReturnValue::new()));
                }
                // jump command
                UnitKind::Jump(dest) => {
                    self.pointer = dest;
                }
                // `continue` — unwind any control-flow scopes (if/switch bodies)
                // opened since the loop body, then re-run the loop head. The loop
                // body's last unit is the compiler's back-jump to the condition, so
                // jumping there re-evaluates it (and, for `for`, runs the update,
                // which the desugaring places at the head on the `continue` path).
                UnitKind::Continue => {
                    loop {
                        let tag = self.ctx.memory.last().unwrap().borrow().tag.clone();
                        if tag == "loopBody" {
                            let body_end = self.ctx.memory.last().unwrap().borrow().frozen_end;
                            self.end_at = body_end;
                            self.pointer = body_end - 1; // the back-jump unit
                            break;
                        }
                        // `continue` not inside a loop (e.g. a stray statement, or
                        // only switch/function scopes around it): nothing to do.
                        if tag == "funcBody" || self.ctx.memory.len() == 1 {
                            break;
                        }
                        self.pop_scope_governed();
                    }
                }
                // `break` — unwind to the nearest enclosing loop or switch body and
                // fall through its end so the normal teardown resumes after it.
                UnitKind::Break => {
                    loop {
                        let tag = self.ctx.memory.last().unwrap().borrow().tag.clone();
                        if tag == "loopBody" || tag == "switchBody" {
                            let body_end = self.ctx.memory.last().unwrap().borrow().frozen_end;
                            self.end_at = body_end;
                            self.pointer = body_end;
                            break;
                        }
                        if tag == "funcBody" || self.ctx.memory.len() == 1 {
                            break;
                        }
                        self.pop_scope_governed();
                    }
                }
                // conditional branch (targets folded into the unit)
                UnitKind::CondBranch { true_branch, false_branch } => {
                    self.registers.push(Box::new(CondBranch::new(true_branch, false_branch)));
                }
                // ----------------------------------
                // expressions
                // scalar / string literal
                UnitKind::Lit(val) => {
                    main_reg = Some(val);
                    continue;
                }
                // identifier reference (resolved against scope / builtins / host)
                UnitKind::Ident(name) => {
                    let val = self.resolve_ident(&name);
                    main_reg = Some(val);
                    continue;
                }
                // function literal (closure over the live environment)
                UnitKind::FuncLit { start, end, params } => {
                    let mut func = Function::new(String::new(), start, end, (*params).clone());
                    func.captured = self.capture_env();
                    main_reg = Some(Val {
                        typ: 10,
                        data: Payload::from(Rc::new(RefCell::new(func))),
                    });
                    continue;
                }
                // object expression
                UnitKind::ObjHead { typ, props_len } => {
                    self.registers.push(Box::new(ObjectExpr::new()));
                    self.registers
                        .last_mut()
                        .unwrap()
                        .set_state(ExecStates::ObjExprExtractInfo, StateData::I64I32(typ, props_len));
                    if self.registers.last().unwrap().get_state() == ExecStates::ObjExprFinished {
                        main_reg = None;
                        is_reg_state_final = true;
                        continue;
                    }
                }
                // array expression
                UnitKind::ArrHead { len } => {
                    self.registers.push(Box::new(ArrayExpr::new()));
                    self.registers
                        .last_mut()
                        .unwrap()
                        .set_state(ExecStates::ArrExprExtractInfo, StateData::I32(len));
                    if self.registers.last().unwrap().get_state() == ExecStates::ArrExprFinished {
                        main_reg = None;
                        is_reg_state_final = true;
                        continue;
                    }
                }
                // ----------------------------------
                // Bare immediates (consumed by a state transition, not dispatched
                // here) and no-op padding: nothing to do, exactly like the old
                // fall-through arm.
                UnitKind::Nop => {}
            }
        }
        Val::new(0, Payload::Null)
    }
}
