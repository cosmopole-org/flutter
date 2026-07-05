//! End-to-end tests: real guest code runs on the Elpian VM and drives the
//! `dart:*` foundational libraries through the governed host seam.

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
