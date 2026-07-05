# Elpian — a no-JIT execution layer for dynamic Flutter code

This directory embeds the **Elpian VM** and builds a **Dart/Flutter runtime
layer** on top of it, so a release Flutter app can load and run application code
**dynamically at runtime with no ahead-of-time compilation and no JIT** — which
is what makes the approach valid on the iOS App Store and on the web, the two
targets the Dart VM cannot serve dynamically.

- **`elpian-vm/`** — the Elpian AST/bytecode VM, vendored unmodified from
  [`cosmopole-org/elpis`](https://github.com/cosmopole-org/elpis) (`crates/elpian-vm`).
  A *pausing bytecode interpreter*: it compiles a JS-subset (or a pre-built AST)
  to bytecode and executes it, suspending on `askHost(apiName, payload)` to hand
  host calls back to the embedder. **It never generates machine code** → no W^X
  violation (App-Store-legal) and it compiles to `wasm32`. It already ships a
  first-class capability + resource-limit governor.
- **`elpian-dart/`** — new. The Dart runtime layer: it drives an Elpian VM and
  services the `dart:*` **foundational ("group 3") libraries** — the native
  surfaces the Flutter framework depends on — as governed host-bridge calls.

Both crates build and test on native **and** `wasm32-unknown-unknown`:

```sh
cd elpian
cargo test                                   # 20 tests, all green
cargo build -p elpian-dart --target wasm32-unknown-unknown --release   # web/iOS-shape build
```

## Why this architecture (and not "run Dart on Elpian directly")

Two hard facts shape everything here:

1. **iOS forbids JIT** (no writable-executable memory; App Store Guideline
   2.5.2). **Web has no Dart VM** (Dart → JS/WASM). So the Dart VM cannot be the
   runtime for *dynamically delivered* code on those platforms — only a
   no-codegen interpreter can. Elpian is exactly that.
2. The Flutter framework is inseparable from the Dart runtime's **foundational
   libraries** (`dart:ui`, `dart:typed_data`, `dart:isolate`, `dart:io`,
   `dart:ffi`). In stock Flutter these are **native C++ functions** bound into
   the Dart isolate by the engine (tonic / `Dart_SetNativeResolver`) — *not* Dart
   source. So "running Flutter on a different VM" is fundamentally about
   **re-providing those native surfaces to the guest**, not about parsing Dart.

This layer therefore re-expresses each foundational library as a **host-bridge
service** over Elpian's `askHost` seam, governed per-call. `dart:ui`'s `Canvas`
calls are *recorded* into a serializable scene tree that the real, native
(AOT-compiled, iOS-legal) engine rasterizes — the guest never touches the GPU or
generates code.

## Governance (the controlling mechanisms) — two layers

Every `dart:*` call passes through both:

1. **VM layer (backstop)** — Elpian's built-in coarse capability families
   (`Gpu`, `Network`, `Storage`, `Clock`, `Randomness`, `Other`, …) plus its
   instruction / memory / call-depth limits. A disabled family short-circuits a
   call to a typed null before it reaches this crate.
2. **Dart layer** (`elpian-dart/src/governance.rs`) — a finer
   [`DartCapability`] per library (`Painting`, `TypedData`, `Io`, `Isolate`,
   `Ffi`, `Environment`), *fails closed* for unknown libraries, plus a
   [`ResourceMeter`] bounding host-call count and bytes moved across the seam.
   `DartCapabilitySet::sandboxed()` denies io/isolate/ffi by default.

## What is implemented and verified today

| Area | Status | Where |
|---|---|---|
| VM embed + `askHost` driver loop | ✅ built, e2e-tested | `runtime.rs` |
| Two-layer capability + resource governor | ✅ built, tested | `governance.rs`, `runtime.rs` |
| Dart **numeric tower** (`int` vs `double`, `~/`, `/`→double, wrapping, `is int`) | ✅ built, tested | `value.rs` |
| `dart:typed_data` — `ByteData` alloc/get/set (Uint8/Int32/Float64) + endianness + `RangeError` | ✅ built, tested | `typed_data.rs` |
| `dart:ui` — `PictureRecorder`/`Canvas`/`Picture` → scene tree (drawRect/Circle/Paragraph) | ✅ built, tested | `dart_ui.rs` |
| native + `wasm32` compilation | ✅ verified | — |

The 4 integration tests run **real guest programs on the real VM** driving these
libraries end-to-end, including a capability-denial and a resource-limit case.

> A finding that de-risks the language work: Elpian's value model **already
> represents integers and floats with separate tags** (`typ` 1/2/3 = i16/i32/i64,
> `typ` 4/5 = f32/f64). Dart's `int`/`double` split — usually the first thing a
> JS-based VM gets wrong — maps onto this natively; `value.rs` supplies the
> Dart-correct *semantics* over that representation.

## Roadmap — remaining work to run real Flutter logic

Ordered, each phase standalone and testable. This is the honest path from the
foundation above to running Flutter app code; it is a large program, not a
weekend.

**Phase 1 — foundational libraries (in progress).**
Finish the `dart:ui` surface (`Path`, `Paint` state, `ParagraphBuilder`,
`SceneBuilder`/layers, `Image`, transforms/clips), extend `dart:typed_data`
(all `TypedList` views, `ByteBuffer` sharing), and add `dart:core`/`dart:math`
native helpers (string/num formatting, `DateTime`, `Random`).

**Phase 2 — the async & concurrency model.**
`Future`/`Stream`, `async`/`await`, `async*`/`sync*`, and Dart's *exact*
microtask-vs-event-queue ordering and `Zone`s. Then `dart:isolate`
(`SendPort`/`ReceivePort`, `Isolate.spawn`) mapped onto Elpian's worker-task
pool. Correct ordering here is what makes framework code behave.

**Phase 3 — the Dart→Elpian AST front-end.**
A compiler from Dart source (and/or Dart **kernel** `.dill`) to the Elpian AST
the VM already ingests. Start with the app-logic subset (classes, mixins,
generics *erased*, null-safety, pattern matching) and grow. This is what lets
app code be written in Dart with "no change," within a documented subset.

**Phase 4 — reified types & full semantic conformance.**
Runtime type representation for reified generics (`x is List<int>`), `as`/`is`
soundness, `const` canonicalization, `identical`/`hashCode`, `noSuchMethod`,
exact exception semantics. This is the "make Elpian fully Dart-compatible at the
VM layer" work; it is where a JS-model VM and Dart genuinely diverge, and it is
large.

**Phase 5 — the framework binding & host integration.**
Wire the recorded `dart:ui` scene tree to a native Flutter rasterizer (engine
embedder / platform view), route pointer/lifecycle/text-input events back into
the VM, and stand up the signed code-delivery pipeline. At this point a
Dart-subset Flutter app updates live in a release build on iOS, web, Android,
desktop.

## Honest scope statement

This foundation is real, compiles, and is tested. It does **not** yet run the
unmodified Flutter framework kernel — that requires Phases 2–5, most weightily
the async/type-system conformance (Phase 4) and the framework binding (Phase 5).
The "no limitations / no changes to app code" end state is the target the
roadmap drives toward; every phase here is a concrete, verifiable step on that
path rather than a claim that it is already reached.
