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

use crate::async_loop::EventLoop;
use crate::binding::{handlers, AppLifecycleState, PointerEvent, RENDER_METHOD};
use crate::core::{Clock, CoreRuntime};
use crate::dart_ui::SceneRecorder;
use crate::governance::{required_capability, DartCapability, DartCapabilitySet, ResourceMeter};
use crate::typed_data::TypedDataStore;

/// Name of the guest entrypoint the runtime invokes to run a scheduled callback.
/// The guest (or generated Dart glue) defines
/// `function __dartDispatch(args) { /* args = [cbId, value] */ }` and routes to
/// its own closure table. This is the single seam through which `Future`/`Timer`
/// continuations re-enter guest code.
const DISPATCH_FN: &str = "__dartDispatch";

/// Guest entrypoint invoked to deliver a `ReceivePort` message: the guest
/// defines `function __portDispatch(args) { /* args = [portId, message] */ }`.
const PORT_DISPATCH_FN: &str = "__portDispatch";

/// Safety bound on a single `run()`'s event-loop pump, so a guest that endlessly
/// reschedules microtasks cannot hang the host. Complements the VM instruction
/// limit and the [`ResourceMeter`].
const DEFAULT_MAX_PUMP_TASKS: u64 = 1_000_000;

/// Error creating or driving a runtime.
#[derive(Debug)]
pub enum DartError {
    /// The guest source was outside the supported subset (failed to compile).
    Compile,
    /// The VM registry lost the instance.
    VmNotFound,
    /// The event-loop pump exceeded its per-run task budget (runaway guest).
    PumpBudgetExceeded,
    /// The Dart front-end failed to compile the source (outside the subset).
    Frontend(String),
}

/// A single embedded Dart runtime instance.
pub struct DartRuntime {
    machine_id: String,
    caps: DartCapabilitySet,
    meter: ResourceMeter,
    typed_data: TypedDataStore,
    ui: SceneRecorder,
    core: CoreRuntime,
    events: EventLoop,
    ports: crate::isolate::PortTable,
    max_pump_tasks: u64,
    /// The scene the guest submitted via `FlutterView.render` this frame.
    current_frame: Option<Value>,
    /// The last fully-rendered frame, retained for diffing.
    last_frame: Option<Value>,
    /// Set when the guest requests a repaint via `dart:ui/scheduleFrame` (e.g.
    /// from `setState`), so the host knows a new frame is due.
    needs_frame: bool,
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
        // The JS front-end now lives in the `js2elpian` crate; it lowers to the
        // Elpian AST and registers the VM through the VM's `from ast` path.
        if !js2elpian::create_vm_from_js(machine_id.clone(), code.into()) {
            return Err(DartError::Compile);
        }
        Ok(DartRuntime {
            machine_id,
            caps,
            meter,
            typed_data: TypedDataStore::new(),
            ui: SceneRecorder::new(),
            core: CoreRuntime::new(Clock::System),
            events: EventLoop::new(),
            ports: crate::isolate::PortTable::new(),
            max_pump_tasks: DEFAULT_MAX_PUMP_TASKS,
            current_frame: None,
            last_frame: None,
            needs_frame: false,
            emitted: Vec::new(),
            log: Vec::new(),
            denied: Vec::new(),
        })
    }

    /// Build a runtime from **Dart-subset source**, compiled to the VM's input
    /// by the Phase 3 front-end (the `dart2elpian` crate). Runtime intrinsics
    /// are still reached via the `dart:*` host bridge.
    pub fn from_dart(
        machine_id: impl Into<String>,
        dart_source: &str,
        caps: DartCapabilitySet,
        meter: ResourceMeter,
    ) -> Result<Self, DartError> {
        // Reified `is`/`as` are handled natively by the VM (the class hierarchy
        // is carried on each instance's prototype chain), so the front-end's
        // declared class list is no longer needed here.
        let (js, _classes) =
            dart2elpian::transpile_program(dart_source).map_err(DartError::Frontend)?;
        Self::from_js(machine_id, js, caps, meter)
    }

    /// Build a runtime from a **Flutter-style widget app**: the user's
    /// `StatelessWidget`/`StatefulWidget` source with a `main()` that calls
    /// `runApp(...)`. The [`crate::widgets`] framework prelude is prepended, then
    /// the whole program takes the same Dart → AST → bytecode → VM path as
    /// [`from_dart`](Self::from_dart). Drive it with [`render_frame`] on each
    /// vsync and [`dispatch_pointer`] for input.
    ///
    /// [`render_frame`]: Self::render_frame
    /// [`dispatch_pointer`]: Self::dispatch_pointer
    pub fn from_widget_app(
        machine_id: impl Into<String>,
        app_source: &str,
        caps: DartCapabilitySet,
        meter: ResourceMeter,
    ) -> Result<Self, DartError> {
        let composed = crate::widgets::compose(app_source);
        Self::from_dart(machine_id, &composed, caps, meter)
    }

    /// Build a runtime from a **Flutter app that imports `flutter.dart`** — the
    /// full idiomatic widget library ([`crate::widgets::FLUTTER_LIB`]). The
    /// library is concatenated ahead of the app (import directives stripped) and
    /// the whole program takes the same Dart → AST → bytecode → VM path. Drive it
    /// with [`render_frame`](Self::render_frame) and
    /// [`dispatch_pointer`](Self::dispatch_pointer).
    pub fn from_flutter_app(
        machine_id: impl Into<String>,
        app_source: &str,
        caps: DartCapabilitySet,
        meter: ResourceMeter,
    ) -> Result<Self, DartError> {
        let composed = crate::widgets::compose_flutter(app_source);
        Self::from_dart(machine_id, &composed, caps, meter)
    }

    /// Whether the guest has requested a repaint since the last
    /// [`clear_needs_frame`](Self::clear_needs_frame) (i.e. a `setState` ran).
    /// A host frame scheduler polls this to coalesce repaints.
    pub fn needs_frame(&self) -> bool {
        self.needs_frame
    }

    /// Clear the pending-repaint flag (call after producing a frame).
    pub fn clear_needs_frame(&mut self) {
        self.needs_frame = false;
    }

    /// The scene the guest most recently submitted via `FlutterView.render`
    /// (e.g. during `main()`), for a host/rasterizer to paint.
    pub fn last_scene(&self) -> Option<Value> {
        self.current_frame.clone()
    }

    /// Pin the clock (and thus `DateTime.now`) for reproducible runs/tests.
    pub fn with_fixed_clock(mut self, millis_since_epoch: i64) -> Self {
        self.core = CoreRuntime::new(Clock::Fixed(millis_since_epoch));
        self
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

    /// Run the guest's top-level program, then pump the event loop until it is
    /// idle — so scheduled microtasks and timers (i.e. `Future`/`Timer`/`async`
    /// continuations) run, exactly as a Dart isolate does before it exits.
    pub fn run(&mut self) -> Result<Value, DartError> {
        let result = self.drive(api::execute_vm(self.machine_id.clone()));
        self.pump()?;
        Ok(result)
    }

    /// Drive a single VM turn to completion, servicing every host call it makes,
    /// and return the turn's result value. Reused by both the top-level run and
    /// each scheduled-callback invocation (so callbacks may themselves make
    /// `dart:*` calls and schedule further work).
    fn drive(&mut self, mut res: VmExecResult) -> Value {
        loop {
            if !res.has_host_call {
                return parse_or_null(&res.result_value);
            }
            let reply = self.service(&res.host_call_data);
            res = api::continue_execution(self.machine_id.clone(), reply.to_string());
        }
    }

    /// Drain the event loop: run every due microtask/timer callback (which may
    /// enqueue more) until quiescent, respecting Dart's ordering. Each callback
    /// re-enters the guest via [`DISPATCH_FN`].
    fn pump(&mut self) -> Result<(), DartError> {
        let mut ran: u64 = 0;
        loop {
            ran += 1;
            if ran > self.max_pump_tasks {
                return Err(DartError::PumpBudgetExceeded);
            }
            // Priority: microtasks/timers (event loop) first, then cooperative
            // isolate spawns, then delivered port messages. Newly-scheduled
            // microtasks always run before the next port message.
            if let Some(task) = self.events.next_task() {
                let input = json!([task.cb, Value::Null]).to_string();
                let res = api::execute_vm_func_with_input(
                    self.machine_id.clone(),
                    DISPATCH_FN.to_string(),
                    input,
                    task.cb as i64,
                );
                let _ = self.drive(res);
            } else if let Some((entry, msg)) = self.ports.pop_spawn() {
                self.invoke_handler(&entry, msg);
            } else if let Some((port, msg)) = self.ports.pop_message() {
                let input = json!([port, msg]).to_string();
                let res = api::execute_vm_func_with_input(
                    self.machine_id.clone(),
                    PORT_DISPATCH_FN.to_string(),
                    input,
                    port as i64,
                );
                let _ = self.drive(res);
            } else {
                return Ok(());
            }
        }
    }

    /// Invoke a named guest handler with a JSON argument, servicing its host
    /// calls and flushing any microtasks it schedules. A missing handler is a
    /// harmless no-op (the VM returns without error).
    fn invoke_handler(&mut self, name: &str, arg: Value) {
        // The guest handler receives `arg` directly as its single parameter,
        // exactly like `onEvent(ev)`.
        let input = arg.to_string();
        let res =
            api::execute_vm_func_with_input(self.machine_id.clone(), name.to_string(), input, 0);
        let _ = self.drive(res);
        let _ = self.pump();
    }

    // ---- framework binding: host -> guest events & the frame pipeline -----

    /// Deliver a pointer (touch/mouse) event to the guest's `onPointerEvent`.
    pub fn dispatch_pointer(&mut self, event: PointerEvent) {
        self.invoke_handler(handlers::POINTER, event.to_json());
    }

    /// Deliver an app-lifecycle transition.
    pub fn dispatch_lifecycle(&mut self, state: AppLifecycleState) {
        self.invoke_handler(handlers::LIFECYCLE, json!(state.as_str()));
    }

    /// Deliver a text-input update.
    pub fn dispatch_text(&mut self, text: &str) {
        self.invoke_handler(handlers::TEXT_INPUT, json!({ "text": text }));
    }

    /// Produce one frame: call `onBeginFrame(t)` then `onDrawFrame()`, and
    /// return the scene tree the guest submitted via `FlutterView.render`
    /// (or `None` if it rendered nothing). This is the vsync tick a real engine
    /// would drive, and the returned scene is what the native rasterizer paints.
    pub fn render_frame(&mut self, frame_time_micros: i64) -> Option<Value> {
        self.current_frame = None;
        self.invoke_handler(handlers::BEGIN_FRAME, json!(frame_time_micros));
        self.invoke_handler(handlers::DRAW_FRAME, Value::Null);
        self.current_frame.take()
    }

    /// Produce one frame and return only the **minimal patch** versus the last
    /// frame (retained diffing), so the host transmits/repaints only what
    /// changed. The first frame returns a single full-set patch.
    pub fn render_frame_patch(&mut self, frame_time_micros: i64) -> Vec<crate::scene_diff::Patch> {
        let new = self.render_frame(frame_time_micros).unwrap_or(Value::Null);
        let patches = match &self.last_frame {
            Some(old) => crate::scene_diff::diff(old, &new),
            None => vec![crate::scene_diff::Patch { path: vec![], value: Some(new.clone()) }],
        };
        self.last_frame = Some(new);
        patches
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

        // The frame-submission call is serviced by the runtime, not the
        // recorder: it captures the scene tree for the host to rasterize.
        if library == "ui" && method == RENDER_METHOD {
            self.current_frame = args.first().cloned();
            return Value::Null;
        }

        // A repaint request (from `setState`/`runApp`) is a runtime signal, not a
        // recorder op: flag it so the host schedules the next frame.
        if library == "ui" && method == "scheduleFrame" {
            self.needs_frame = true;
            return Value::Null;
        }

        let result = match library {
            "typed_data" => self.typed_data.dispatch(method, args),
            "ui" => self.ui.dispatch(method, args),
            "core" | "math" => self.core.dispatch(library, method, args),
            // `dart:convert` (JSON/UTF-8/Base64) is a pure codec now provided
            // natively by the VM stdlib (`jsonParse`/`jsonStringify`/`utf8Encode`/
            // `utf8Decode`/`base64Encode`/`base64Decode`), so it no longer needs a
            // host-bridge service here.
            "async" => self.dispatch_async(method, args),
            "isolate" => self.dispatch_isolate(method, args),
            other => Err(format!("unimplemented library dart:{other} (method {method})")),
        };

        match result {
            Ok(v) => v,
            Err(msg) => dart_error(&msg),
        }
    }

    /// `dart:async` native scheduling hooks. `Future`/`Stream`/`Completer` and
    /// `async`/`await` sit on top of these in Dart source.
    fn dispatch_async(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        match method {
            // scheduleMicrotask(cbId)
            "scheduleMicrotask" => {
                let cb = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("scheduleMicrotask requires a callback id")?;
                self.events.schedule_microtask(cb);
                Ok(Value::Null)
            }
            // Timer(cbId, delayMs) -> timerId
            "Timer" => {
                let cb = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("Timer requires a callback id")?;
                let delay = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(serde_json::json!(self.events.schedule_timer(cb, delay)))
            }
            // Timer.periodic(cbId, intervalMs) -> timerId
            "Timer.periodic" => {
                let cb = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("Timer.periodic requires a callback id")?;
                let interval = args.get(1).and_then(|v| v.as_u64()).unwrap_or(1);
                Ok(serde_json::json!(self.events.schedule_periodic(cb, interval)))
            }
            // Timer.cancel(timerId) -> bool
            "Timer.cancel" => {
                let id = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("Timer.cancel requires a timer id")?;
                Ok(serde_json::json!(self.events.cancel_timer(id)))
            }
            other => Err(format!("NoSuchMethodError: dart:async/{other}")),
        }
    }

    /// `dart:isolate` native hooks: ports and cooperative spawn.
    fn dispatch_isolate(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        match method {
            // ReceivePort() -> portId
            "ReceivePort" => Ok(json!(self.ports.new_receive_port())),
            // SendPort.send(portId, message)
            "SendPort.send" => {
                let port = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("SendPort.send requires a port id")?;
                let msg = args.get(1).cloned().unwrap_or(Value::Null);
                self.ports.send(port as u32, msg)?;
                Ok(Value::Null)
            }
            // ReceivePort.close(portId)
            "ReceivePort.close" => {
                let port = args
                    .first()
                    .and_then(|v| v.as_u64())
                    .ok_or("close requires a port id")?;
                Ok(json!(self.ports.close(port as u32)))
            }
            // Isolate.spawn(entryFnName, message)
            "Isolate.spawn" => {
                let entry = args
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or("Isolate.spawn requires an entry name")?
                    .to_string();
                let msg = args.get(1).cloned().unwrap_or(Value::Null);
                self.ports.spawn(entry, msg);
                Ok(Value::Null)
            }
            other => Err(format!("NoSuchMethodError: dart:isolate/{other}")),
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
