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
- **`js2elpian/` · `dart2elpian/`** — the language front-ends. Each compiles its
  source language (JavaScript / a Dart subset) to Elpian bytecode; the VM is the
  single, unified execution target. Dart-specific semantics — the `~/` and `??`
  operators, reified `is`/`as`, the int/double numeric split, and the
  JSON/UTF-8/Base64 codecs — are **native to the VM**, not front-end shims.
  Standard-library **API names are universal**: the VM exposes one flat, neutral
  stdlib surface (`push`, `upper`, `has`, `reversed`, …), and each front-end
  resolves *its* language's spelling (`List.add`, `toUpperCase`, `Array.push`,
  `includes`, …) onto that universal name **at compile time**. The VM carries no
  Dart- or JS-specific method names and does no name translation (no proxying)
  at runtime.
- **`dart/`** — the *optional* Dart/Flutter host surface: it drives an Elpian VM
  and services the `dart:*` **foundational ("group 3") libraries** — the native
  surfaces the Flutter framework depends on (`dart:ui`, `dart:typed_data`,
  `dart:isolate`, …) plus the widget layer — as governed host-bridge calls. It
  is gated behind the `dart` cargo feature (the `--include-dart` switch): a VM
  build that does not want the Dart extras simply omits it.

The crates build and test on native **and** `wasm32-unknown-unknown`:

```sh
cd elpian
cargo test                                             # full suite, all green
cargo build -p dart --target wasm32-unknown-unknown --release              # with Dart extras
cargo build -p dart --no-default-features --target wasm32-unknown-unknown   # VM-only, no Dart
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
2. **Dart layer** (`dart/src/governance.rs`) — a finer
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
| **P1** `dart:typed_data` — `ByteData` + typed-list views + endianness + `setRange` | ✅ built, tested | `typed_data.rs` |
| **P1** `dart:ui` — `Canvas`/`Paint`/`Path`/transform/clip/`PictureRecorder`/`SceneBuilder` → scene tree | ✅ built, tested | `dart_ui.rs` |
| **P1** `dart:core`/`dart:math` — `DateTime.now`, seeded `Random`, math fns, String methods | ✅ built, tested | `core.rs` |
| **P1+** `dart:convert` — JSON / UTF-8 / Base64 codecs | ✅ built, tested | `convert.rs` |
| **P2** async model — microtask/timer event loop with Dart's exact ordering + `Timer.periodic` | ✅ built, tested | `async_loop.rs`, `runtime.rs` |
| **P2+** `dart:isolate` — `ReceivePort`/`SendPort`/`Isolate.spawn` (cooperative) | ✅ built, tested | `isolate.rs` |
| **P3** Dart → Elpian front-end — types, control flow, `~/`, interpolation, ternary, `++`/compound | ✅ built, tested | `dart_frontend.rs` |
| **P3+** Dart **classes** — fields, ctors (`this.x`), methods, `extends`/`super`, `this`, instantiation | ✅ built, tested | `dart_frontend.rs` |
| **P4** reified types, subtyping, function types, generics substitution, `const` canon., `noSuchMethod` | ✅ built, tested | `types.rs` |
| **P4+** reified `is`/`as` end-to-end — class-instance subtype + primitive checks from Dart | ✅ built, tested | `runtime.rs` |
| **P5** framework binding — pointer/lifecycle/text events + vsync frame pump | ✅ built, tested | `binding.rs`, `runtime.rs` |
| **P5+** retained **scene diffing** — minimal per-frame patch (diff + apply) | ✅ built, tested | `scene_diff.rs` |
| **P5** signed code-delivery — SHA-256/HMAC (KAT-verified), verify-before-load, downgrade guard, signed **manifest** with content-hash pinning | ✅ built, tested | `sha256.rs`, `bundle.rs` |
| **P6** **widget layer** — `StatelessWidget`/`StatefulWidget`/`State`, `runApp`, `Container`/`Column`/`Row`/`Center`/`Text`/`GestureDetector`/… ; build → layout → paint → tap → `setState` → repaint | ✅ built, tested | `widgets.rs` |
| **P6** front-end deepening — `for-in` loops + hex int literals (`0xFF2196F3`) | ✅ built, tested | `dart_frontend.rs` |
| **P7** **`flutter.dart` library** — a large, idiomatic Flutter widget/painting library (`Widget`/`State`/`Color`/`Colors`/`EdgeInsets`/`Alignment`/`BoxConstraints`/enums/`RenderFlex`-style layout/`MaterialApp`/`Scaffold`/`AppBar`/`Card`/…), `import`ed by an app | ✅ built, tested | `flutter/flutter.dart` |
| **P7** front-end idioms — annotations, `abstract`, `const`, `enum`, `static` members + named ctors, **getters**, `??`, `void`-arrow bodies | ✅ built, tested | `dart_frontend.rs` |
| **P8** **real Skia via CanvasKit** — a reflective bridge that drives the *entire* Skia API (construct/call/enum/marshal by name; audited 575 symbols, 0 unreachable), replaying Elpian scenes/programs on genuine Skia with real text | ✅ built, browser-tested | `web-demo/canvaskit_bridge.js` |
| native + `wasm32` compilation | ✅ verified | — |

**190 tests pass** (native) and the whole stack builds for `wasm32`. The
integration tests run **real guest programs on the real VM** end-to-end,
including: Dart classes with inheritance, reified `is`/`as`, the async ordering
guarantee, isolate message passing, a capability denial, a resource-limit
cutoff, the pointer-event + frame-render loop with retained diffing, the
signed-bundle accept/tamper-reject path, a **real Flutter-style widget app**
that renders / taps / `setState`s / repaints, and — new — a **realistic app
written against the imported `flutter.dart` library** (`MaterialApp` → `Scaffold`
→ `AppBar` + a `StatefulWidget` counter card with `+`/`-` `ElevatedButton`s,
`Card`, `Row`/`Column`, `Expanded`, and stat chips) whose buttons drive
`setState`. Both apps also run in a **headless browser and rasterize to real
pixels** (`web-demo/widgets_test.mjs`, `web-demo/flutter_test.mjs`).

The `flutter.dart` library is imported exactly as in Flutter:

```dart
import 'flutter.dart';
class MyApp extends StatelessWidget {
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      appBar: AppBar(title: Text('Hi', style: TextStyle(color: Colors.white))),
      body: Center(child: Text('Hello', style: TextStyle(fontSize: 32.0))),
    ),
  );
}
void main() => runApp(MyApp());
```

> A finding that de-risks the language work: Elpian's value model **already
> represents integers and floats with separate tags** (`typ` 1/2/3 = i16/i32/i64,
> `typ` 4/5 = f32/f64). Dart's `int`/`double` split — usually the first thing a
> JS-based VM gets wrong — maps onto this natively; `value.rs` supplies the
> Dart-correct *semantics* over that representation.

## Roadmap status

Phases 1–5 each landed a real, tested vertical slice (see the table above). What
each phase established, and what remains to *deepen* it toward the full framework:

**Phase 1 — foundational libraries ✅ (slice).** `dart:typed_data`, `dart:ui`,
`dart:core`/`dart:math` implemented for their load-bearing subsets. *Deepen:*
`Image`/`ParagraphBuilder` layout, `ByteBuffer` sharing, the rest of `dart:core`.

**Phase 2 — async & concurrency ✅ (slice).** The microtask/timer event loop with
Dart's exact ordering; `Future`/`Stream`/`async`-`await` layer on top in Dart
source. *Deepen:* `dart:isolate` (`SendPort`/`ReceivePort`) on Elpian's worker
pool; `Zone`s; `async*`/`sync*`.

**Phase 3 — Dart → Elpian front-end ✅ (slice).** A real lexer/parser/emitter for
a Dart subset (typed decls, control flow, `~/`, interpolation). *Deepen:*
classes/mixins, generics, pattern matching, and a Dart **kernel** (`.dill`)
front-end for unchanged app code.

**Phase 4 — reified types & conformance ✅ (slice).** Reified types + subtyping,
`is`/`as`, `const` canonicalization, `noSuchMethod` resolution. *Deepen:* wire
the front-end to emit type metadata at allocation/`is`/`as` sites; exact
exception semantics.

**Phase 5 — framework binding & delivery ✅ (slice).** Pointer/lifecycle/text
event routing, the `onBeginFrame`/`onDrawFrame` vsync pump returning a scene
tree, and a **signed** bundle loader (real SHA-256/HMAC, verify-before-load,
downgrade protection). *Deepen:* connect the returned scene tree to a native
Flutter rasterizer (engine embedder / platform view) and add ed25519 signatures.

**Phase 6 — the widget layer ✅ (slice).** A Flutter-shaped widget framework
(`widgets.rs`), written in the Dart subset and compiled through the same
front-end, so **real widget code** — `StatelessWidget`/`StatefulWidget` with
`build()` and nested children — runs on the VM. `runApp` owns the engine binding;
each frame it rebuilds the tree from the root, lays it out under constraints,
and paints it into the `dart:ui` scene; taps are hit-tested to `GestureDetector`s
and `setState` requests the next frame. `State` persists across frames (matched
by build order). The `engine/.../elpian` layer exposes `LoadWidgetApp` /
`elpian_init_widgets` to run such a bundle.

**Phase 7 — the `flutter.dart` library ✅.** A large, **idiomatic** Flutter
widget/painting library authored as ordinary Dart in
[`flutter/flutter.dart`](dart/flutter/flutter.dart) and modelled on the
real framework's public API — `Key`, `Widget`/`StatelessWidget`/`StatefulWidget`/
`State`, `BuildContext`; the painting value types `Color`/`Colors`/`Offset`/
`Size`/`Rect`/`EdgeInsets`/`Alignment`/`BorderRadius`/`BoxDecoration`/`TextStyle`/
`BoxConstraints`; the enums `Axis`/`MainAxisAlignment`/`CrossAxisAlignment`/
`MainAxisSize`/`TextAlign`/`FontWeight`; a real two-phase `layout(constraints)`/
`paint(offset)` protocol with a `RenderFlex`-style flex algorithm; the widgets
`Container`/`DecoratedBox`/`ColoredBox`/`Padding`/`Center`/`Align`/`Column`/`Row`/
`Stack`/`Positioned`/`Expanded`/`Flexible`/`Spacer`/`SizedBox`/`Text`/`Icon`/
`Divider`/`GestureDetector`; and the Material shells `MaterialApp`/`Scaffold`/
`AppBar`/`Card`/`ElevatedButton`/`TextButton`. An app imports it (`import
'flutter.dart';`) and runs on the VM (`LoadFlutterApp` / `elpian_init_flutter`).
Landing it drove the front-end
to accept the idioms the real framework is written in (annotations, `abstract`,
`const`, `enum`, `static` members + named constructors, getters, `??`). *Deepen:*
`Stack` fit/clipping, `ListView`/scrolling, `Theme`/`InheritedWidget`, keyed
reconciliation, animations, and engine-provided text metrics.

**Phase 8 — real Skia via CanvasKit ✅.** A **reflective bridge**
([`web-demo/canvaskit_bridge.js`](web-demo/canvaskit_bridge.js)) drives real Skia
through **CanvasKit** (Skia-in-WASM — the renderer Flutter web uses). Rather than
hand-wrapping Skia's ~1000 methods, it interprets a uniform "Skia program": it
can construct any object, call any static factory or instance method, resolve any
enum/constant, and marshal every Skia argument shape (colors, rects, rrects,
matrices, scalar/point arrays, typed data, handles, nested option dicts) — all
**by name**, so it covers the **entire Skia API with no exceptions**, audited at
runtime against the loaded library (575 symbols, 0 unreachable). It replays both
Elpian widget scenes (with **real text layout** via CanvasKit's Paragraph API)
and raw programs a guest emits over `dart:ui`. Verified headless: the
`flutter.dart` app rasterized by real Skia (interactive), a full-API showcase
(gradients, Bézier paths, mask/image-filter blur, `saveLayer`, shaped text), and
a guest that drives Skia directly from bytecode. *Deepen:* GPU (WebGL/WebGPU)
surface in production, `dart:ui` recorder emitting the program in Rust, and the
matching native Skia/Impeller binding for iOS/Android/desktop.

## Honest scope statement

Phases 1–7 are real, compile (native + wasm32), and are covered by 190 passing
tests including end-to-end runs on the actual VM — now including a realistic app
written against the imported `flutter.dart` library that renders a full Material
screen, takes taps, and repaints from mutated `State`, verified both in the Rust
suite and, rasterized to pixels, in a headless browser.
This is a working **foundation and vertical slice through every layer** of the
architecture — not a complete Dart VM or a drop-in for the unmodified Flutter
framework. The two things that remain genuinely large are (a) breadth — filling
out each `dart:*` library, the language front-end, and the widget set toward the
full `package:flutter` surface, and (b) the final native-rasterizer binding
(compiling the `engine/.../elpian` glue in a real engine build, plus
`drawParagraph` text layout). Every slice here is a concrete, verifiable step
toward the "update app code live in a release build on iOS/web/Android/desktop"
goal, with the security-critical verify-before-execute control already real.
