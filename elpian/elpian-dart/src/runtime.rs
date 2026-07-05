//! The Dart runtime driver: owns an Elpian VM instance and services the
//! `dart:*` foundational-library calls the guest makes over the `askHost` seam,
//! applying the two-layer governance on every call.
//!
//! Loop: [`DartRuntime::run`] steps the VM; when it pauses on `askHost` we parse
//! the `{machineId, apiName, payload}` envelope, route `apiName` to a library
//! handler (after the capability + resource checks), and resume the VM with the
//! JSON reply — which becomes the guest call's return value. A thrown Dart error
//! is modelled as a reply object `{ "__dart_error__": "<message>" }` that a Dart
//! front-end lowers back into `throw`.

use serde_json::{json, Value};

use elpian_vm::api::{self, VmExecResult};
use elpian_vm::sdk::capabilities::Capability;

use crate::dart_ui::SceneRecorder;
use crate::governance::{required_capability, DartCapability, DartCapabilitySet, ResourceMeter};
use crate::typed_data::TypedDataStore;

/// Error creating or driving a runtime.
#[derive(Debug)]
pub enum DartError {
    /// The guest source was outside the supported subset (failed to compile).
    Compile,
    /// The VM registry lost the instance.
    VmNotFound,
}

/// A single embedded Dart runtime instance.
pub struct DartRuntime {
    machine_id: String,
    caps: DartCapabilitySet,
    meter: ResourceMeter,
    typed_data: TypedDataStore,
    ui: SceneRecorder,
    emitted: Vec<Value>,
    log: Vec<String>,
    denied: Vec<String>,
}

impl DartRuntime {
    /// Build a runtime from JavaScript-subset source (the interim guest surface
    /// until the Dart→Elpian front-end lands; the host-call ABI is identical
    /// either way). `caps` and `meter` set the governance posture.
    pub fn from_js(
        machine_id: impl Into<String>,
        code: impl Into<String>,
        caps: DartCapabilitySet,
        meter: ResourceMeter,
    ) -> Result<Self, DartError> {
        let machine_id = machine_id.into();
        api::init_vm_system();
        if !api::create_vm_from_js(machine_id.clone(), code.into()) {
            return Err(DartError::Compile);
        }
        Ok(DartRuntime {
            machine_id,
            caps,
            meter,
            typed_data: TypedDataStore::new(),
            ui: SceneRecorder::new(),
            emitted: Vec::new(),
            log: Vec::new(),
            denied: Vec::new(),
        })
    }

    /// Values the guest pushed out via `askHost("test.emit", [v])` — a tiny
    /// egress channel used by tests and the harness.
    pub fn emitted(&self) -> &[Value] {
        &self.emitted
    }

    /// Diagnostic lines from `askHost("log", [...])`.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// api-names denied by the governor (capability off or over limit).
    pub fn denied(&self) -> &[String] {
        &self.denied
    }

    /// Host-call count charged to the meter so far.
    pub fn host_calls(&self) -> u64 {
        self.meter.host_calls()
    }

    /// Mirror a Dart capability decision down to the VM's coarse backstop gate
    /// as well, so a revoked family is enforced at *both* layers.
    pub fn revoke(&mut self, cap: DartCapability) {
        self.caps.revoke(cap);
        // Best-effort: map to the nearest coarse VM family and turn it off too.
        if let Some(vm_cap) = coarse_family(cap) {
            api::set_capability(&self.machine_id, vm_cap, false);
        }
    }

    /// Drive the VM to completion, servicing every host call. Returns the
    /// top-level result value (usually a status string).
    pub fn run(&mut self) -> Result<Value, DartError> {
        let mut res = api::execute_vm(self.machine_id.clone());
        loop {
            if !res.has_host_call {
                return Ok(parse_or_null(&res.result_value));
            }
            let reply = self.service(&res.host_call_data);
            res = api::continue_execution(self.machine_id.clone(), reply.to_string());
        }
    }

    /// Service one `{machineId, apiName, payload}` envelope, returning the JSON
    /// value to resume the guest with.
    fn service(&mut self, envelope_json: &str) -> Value {
        let env: Value = match serde_json::from_str(envelope_json) {
            Ok(v) => v,
            Err(_) => return Value::Null,
        };
        let api_name = env.get("apiName").and_then(|v| v.as_str()).unwrap_or("");
        let args = args_of(env.get("payload"));

        match api_name {
            "log" => {
                self.log.push(stringify_args(&args));
                Value::Null
            }
            "test.emit" => {
                self.emitted.push(args.first().cloned().unwrap_or(Value::Null));
                Value::Null
            }
            name if name.starts_with("dart:") => self.service_dart(&name["dart:".len()..], &args),
            _ => Value::Null,
        }
    }

    /// Route a `dart:<library>/<method>` call through governance to the library.
    fn service_dart(&mut self, lib_and_method: &str, args: &[Value]) -> Value {
        let (library, method) = match lib_and_method.split_once('/') {
            Some(pair) => pair,
            None => return dart_error(&format!("malformed dart api name: {lib_and_method}")),
        };

        // Layer 2 governance: capability gate.
        let cap = required_capability(library);
        if !self.caps.allows(cap) {
            let name = format!("dart:{lib_and_method}");
            self.denied.push(name.clone());
            return dart_error(&format!("capability denied for {name}"));
        }

        // Resource metering: charge one call plus the argument byte weight.
        let bytes = approx_bytes(args);
        if let Err(e) = self.meter.charge(bytes) {
            self.denied.push(format!("dart:{lib_and_method}"));
            return dart_error(&e);
        }

        let result = match library {
            "typed_data" => self.typed_data.dispatch(method, args),
            "ui" => self.ui.dispatch(method, args),
            other => Err(format!("unimplemented library dart:{other} (method {method})")),
        };

        match result {
            Ok(v) => v,
            Err(msg) => dart_error(&msg),
        }
    }
}

/// A reply object modelling a thrown Dart error across the host seam.
fn dart_error(message: &str) -> Value {
    json!({ "__dart_error__": message })
}

/// Normalize a payload into an argument array: an array stays as-is; anything
/// else (or absent) becomes a single-element / empty list.
fn args_of(payload: Option<&Value>) -> Vec<Value> {
    match payload {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    }
}

fn approx_bytes(args: &[Value]) -> u64 {
    args.iter().map(|v| v.to_string().len() as u64).sum()
}

fn stringify_args(args: &[Value]) -> String {
    args.iter()
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_or_null(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// Map a fine-grained Dart capability to the nearest coarse VM family for the
/// backstop gate. Returns `None` for families with no coarse analogue (they are
/// still enforced at the Dart layer).
fn coarse_family(cap: DartCapability) -> Option<Capability> {
    match cap {
        DartCapability::Painting => Some(Capability::Gpu),
        DartCapability::Io => Some(Capability::Storage),
        DartCapability::Isolate => Some(Capability::Other),
        DartCapability::Environment => Some(Capability::Clock),
        DartCapability::TypedData | DartCapability::Ffi => None,
    }
}

// Re-export for the harness/tests: a VmExecResult passthrough type alias.
pub type ExecResult = VmExecResult;
