//! End-to-end tests: real guest code runs on the Elpian VM and drives the
//! `dart:*` foundational libraries through the governed host seam.

use elpian_dart::binding::{PointerEvent, PointerPhase};
use elpian_dart::bundle::{BundleLoader, CodeBundle, HmacSha256Scheme};
use elpian_dart::{DartCapability, DartCapabilitySet, DartRuntime, ResourceMeter};

/// A `dart:typed_data` round-trip driven entirely from guest code: allocate a
/// ByteData, write an Int32, read it back, and emit the result. This exercises
/// the full loop — guest `askHost` → envelope parse → governance → library →
/// resume with reply → guest observes the value.
#[test]
fn guest_drives_typed_data_roundtrip() {
    let code = r#"
        var buf = askHost("dart:typed_data/ByteData.alloc", [8]);
        askHost("dart:typed_data/ByteData.setInt32", [buf, 0, 1234567, true]);
        var v = askHost("dart:typed_data/ByteData.getInt32", [buf, 0, true]);
        askHost("test.emit", [v]);
    "#;
    let mut rt = DartRuntime::from_js(
        "td_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    assert_eq!(rt.emitted(), &[serde_json::json!(1234567)]);
}

/// A `dart:ui` recording driven from guest code, composed into a scene tree.
#[test]
fn guest_records_a_ui_scene() {
    let code = r#"
        askHost("dart:ui/PictureRecorder.beginRecording", []);
        askHost("dart:ui/Canvas.drawRect", [0.0, 0.0, 100.0, 50.0, 4294901760]);
        var pic = askHost("dart:ui/PictureRecorder.endRecording", []);
        var scene = askHost("dart:ui/Picture.toScene", [pic]);
        askHost("test.emit", [scene]);
    "#;
    let mut rt = DartRuntime::from_js(
        "ui_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    let scene = &rt.emitted()[0];
    assert_eq!(scene["root"]["ops"][0]["op"], "drawRect");
    assert_eq!(scene["root"]["ops"][0]["color"], 4294901760u64);
}

/// Guest uses `dart:math` (seeded Random) and `dart:core` (DateTime.now with a
/// pinned clock) end-to-end, proving the core/math wiring and determinism.
#[test]
fn guest_uses_core_and_math() {
    let code = r#"
        var rng = askHost("dart:math/Random", [42]);
        var a = askHost("dart:math/Random.nextInt", [rng, 1000]);
        var now = askHost("dart:core/DateTime.now", []);
        askHost("test.emit", [a]);
        askHost("test.emit", [now]);
    "#;
    let mut rt = DartRuntime::from_js(
        "core_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles")
    .with_fixed_clock(1_700_000_000_000);
    rt.run().expect("runs");
    // Seed 42 is deterministic; assert the value is in range and the clock pinned.
    let a = rt.emitted()[0].as_i64().unwrap();
    assert!((0..1000).contains(&a));
    assert_eq!(rt.emitted()[1], serde_json::json!(1_700_000_000_000i64));
}

/// Phase 3: real **Dart source** compiled by the front-end and executed on the
/// VM end-to-end. Exercises typed locals, a C-style `for` (lowered to `while`),
/// `~/`, a function call, string interpolation, and reaching the host bridge.
#[test]
fn runs_real_dart_source() {
    let dart = r#"
        int sumTo(int n) {
            int total = 0;
            for (int i = 1; i <= n; i = i + 1) {
                total = total + i;
            }
            return total;
        }
        void main() {
            int s = sumTo(10);
            int half = s ~/ 2;
            askHost("test.emit", ["sum=$s half=$half"]);
        }
    "#;
    let mut rt = DartRuntime::from_dart(
        "dart_test",
        dart,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("dart compiles");
    rt.run().expect("runs");
    // 1+..+10 = 55; 55 ~/ 2 = 27.
    assert_eq!(rt.emitted(), &[serde_json::json!("sum=55 half=27")]);
}

/// The async event loop drives real guest callbacks in Dart's exact order:
/// microtasks before timers, with nested scheduling handled correctly. The guest
/// routes scheduled callbacks through `__dartDispatch`, exactly as generated Dart
/// glue would.
#[test]
fn event_loop_runs_callbacks_in_dart_order() {
    let code = r#"
        function __dartDispatch(a) {
            var id = a[0];
            askHost("test.emit", ["cb" + id]);
            // callback 3 (a microtask) schedules a further microtask, cb 4,
            // which must still run before the already-scheduled timer cb 2.
            if (id == 3) { askHost("dart:async/scheduleMicrotask", [4]); }
        }
        askHost("dart:async/scheduleMicrotask", [1]);
        askHost("dart:async/Timer", [2, 10]);
        askHost("dart:async/scheduleMicrotask", [3]);
    "#;
    let mut rt = DartRuntime::from_js(
        "async_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    let order: Vec<String> = rt
        .emitted()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    // 1 and 3 (microtasks) first; 3 schedules 4 which still precedes the timer 2.
    assert_eq!(order, vec!["cb1", "cb3", "cb4", "cb2"]);
}

/// Governance: with the `Painting` capability revoked, a `dart:ui` call is
/// denied by the governor and the guest receives a thrown-error envelope rather
/// than reaching the library.
#[test]
fn revoked_capability_denies_the_call() {
    let code = r#"
        var r = askHost("dart:ui/PictureRecorder.beginRecording", []);
        askHost("test.emit", [r]);
    "#;
    let mut caps = DartCapabilitySet::full();
    caps.revoke(DartCapability::Painting);
    let mut rt = DartRuntime::from_js("denied_test", code, caps, ResourceMeter::unbounded())
        .expect("compiles");
    rt.run().expect("runs");
    let reply = &rt.emitted()[0];
    assert!(
        reply.get("__dart_error__").is_some(),
        "expected a thrown-error envelope, got {reply}"
    );
    assert_eq!(rt.denied().len(), 1);
    assert!(rt.denied()[0].contains("dart:ui"));
}

/// Governance: the resource meter bounds a guest that floods the host, even if
/// it stays within the VM's instruction budget.
#[test]
fn resource_meter_bounds_host_calls() {
    let code = r#"
        var i = 0;
        while (i < 100) {
            askHost("dart:typed_data/ByteData.alloc", [4]);
            i = i + 1;
        }
        askHost("test.emit", ["done"]);
    "#;
    // Cap host calls at 5; the guest tries ~100.
    let meter = ResourceMeter::new(Some(5), None);
    let mut rt = DartRuntime::from_js(
        "meter_test",
        code,
        DartCapabilitySet::full(),
        meter,
    )
    .expect("compiles");
    rt.run().expect("runs");
    // Once the ceiling is hit, subsequent dart: calls are denied.
    assert!(!rt.denied().is_empty(), "meter should have denied calls");
}

/// Phase 5: the framework binding end-to-end. A Dart guest defines pointer and
/// frame handlers; the runtime delivers a tap and drives a frame, and the guest
/// renders a scene the host collects — exactly the engine <-> framework loop.
#[test]
fn binding_delivers_events_and_collects_a_frame() {
    let dart = r#"
        var taps = 0;
        void onPointerEvent(e) {
            taps = taps + 1;
            askHost("test.emit", ["tap$taps"]);
        }
        void onDrawFrame() {
            askHost("dart:ui/PictureRecorder.beginRecording", []);
            askHost("dart:ui/Canvas.drawRect", [0.0, 0.0, 10.0, 10.0, 4278190080]);
            var pic = askHost("dart:ui/PictureRecorder.endRecording", []);
            var scene = askHost("dart:ui/Picture.toScene", [pic]);
            askHost("dart:ui/FlutterView.render", [scene]);
        }
    "#;
    let mut rt = DartRuntime::from_dart(
        "binding_test",
        dart,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("dart compiles");
    rt.run().expect("defines handlers");

    // Deliver two taps.
    rt.dispatch_pointer(PointerEvent { pointer: 1, phase: PointerPhase::Down, x: 5.0, y: 5.0 });
    rt.dispatch_pointer(PointerEvent { pointer: 1, phase: PointerPhase::Up, x: 5.0, y: 5.0 });
    assert_eq!(
        rt.emitted(),
        &[serde_json::json!("tap1"), serde_json::json!("tap2")]
    );

    // Drive a frame; the guest renders a rectangle scene the host collects.
    let frame = rt.render_frame(16_000).expect("guest rendered a frame");
    assert_eq!(frame["root"]["ops"][0]["op"], "drawRect");
}

/// Phase 5: the signed code-delivery path, from signing to a verified run.
#[test]
fn signed_bundle_loads_and_runs_but_tamper_is_rejected() {
    let key = *b"deployment-signing-key";
    let signer = BundleLoader::new(HmacSha256Scheme::new(key));

    let mut bundle = CodeBundle {
        id: "app.counter".into(),
        version: 7,
        entrypoint: "main".into(),
        source: r#"void main() { askHost("test.emit", ["v7"]); }"#.into(),
        signature: Vec::new(),
    };
    signer.sign(&mut bundle);

    // Device side: verify before loading.
    let mut loader = BundleLoader::new(HmacSha256Scheme::new(key));
    let trusted_source = loader.accept(&bundle).expect("valid bundle accepted");
    let mut rt = DartRuntime::from_dart(
        "bundle_run",
        &trusted_source,
        DartCapabilitySet::sandboxed(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    assert_eq!(rt.emitted(), &[serde_json::json!("v7")]);

    // A tampered bundle never yields source, so it can never reach the VM.
    let mut evil = bundle.clone();
    evil.version = 8;
    evil.source = r#"void main() { askHost("test.emit", ["pwned"]); }"#.into();
    let mut loader2 = BundleLoader::new(HmacSha256Scheme::new(key));
    assert!(loader2.accept(&evil).is_err());
}

/// Deepen P2: dart:isolate ports deliver messages to the guest in send order,
/// and a cooperative Isolate.spawn runs a named entry with its message.
#[test]
fn isolate_ports_and_spawn() {
    let code = r#"
        function __portDispatch(a) {
            askHost("test.emit", ["got:" + a[1]]);
        }
        function worker(msg) {
            askHost("test.emit", ["worker:" + msg]);
        }
        var port = askHost("dart:isolate/ReceivePort", []);
        askHost("dart:isolate/SendPort.send", [port, "one"]);
        askHost("dart:isolate/SendPort.send", [port, "two"]);
        askHost("dart:isolate/Isolate.spawn", ["worker", "hi"]);
    "#;
    let mut rt = DartRuntime::from_js(
        "iso_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    let out: Vec<String> = rt.emitted().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    // spawn drains before port messages in the pump priority order.
    assert_eq!(out, vec!["worker:hi", "got:one", "got:two"]);
}

/// Deepen P2: a periodic timer fires repeatedly until the guest cancels it.
#[test]
fn periodic_timer_end_to_end() {
    let code = r#"
        var count = 0;
        var id = 0;
        function __dartDispatch(a) {
            count = count + 1;
            askHost("test.emit", ["tick" + count]);
            if (count == 3) { askHost("dart:async/Timer.cancel", [id]); }
        }
        id = askHost("dart:async/Timer.periodic", [1, 10]);
    "#;
    let mut rt = DartRuntime::from_js(
        "periodic_test",
        code,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    )
    .expect("compiles");
    rt.run().expect("runs");
    let out: Vec<String> = rt.emitted().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(out, vec!["tick1", "tick2", "tick3"]);
}
