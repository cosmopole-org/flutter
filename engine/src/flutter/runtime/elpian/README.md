# Elpian runtime — embedding the Elpian VM in the Flutter engine

This is the **integration layer** that runs a dynamically-delivered Flutter
bundle on the [Elpian VM](../../../../../elpian/) instead of a Dart isolate, and
lowers the scene the bundle produces into the engine's `DisplayList` so the
existing rasterizer paints it.

```
  bundle (Dart-subset source) ─▶ elpian_init ─┐
  pointer events ─▶ elpian_pointer ────────────┤   Elpian VM (Rust staticlib,
  vsync ─▶ elpian_frame ────────────────────────┘   linked via C ABI; tested)
                                        │  scene-tree JSON
                                        ▼
                ElpianRuntime::RenderFrame ─▶ DisplayListBuilder
                (drawRect/drawCircle/… ─▶ DlPaint/DlColor)  ─▶ compositor
```

## Why this exists

The normal engine path AOT-compiles Dart and runs it in a Dart isolate; updating
app code means an app-store rebuild, and on iOS/web the Dart VM can't load code
dynamically. The Elpian VM is a **no-JIT** interpreter (App-Store-safe, and it
compiles to wasm for web), so a bundle can be shipped and reloaded at runtime.
The bundle's own widget/scene specification and logic decide what is drawn —
different bundles render different widgets with no engine rebuild, which is the
"Miniapp / dynamic Flutter" goal.

## How it slots in

- **`elpian_ffi.h`** — the C ABI of the `elpian-dart` crate. These are the exact
  `#[no_mangle] extern "C"` symbols the crate's test suite and the wasm/browser
  build exercise, so the runtime the engine embeds is the tested one; the C++ is
  only glue.
- **`ElpianRuntime`** — `LoadBundle` (compile+run, defining `onDrawFrame` /
  `onPointerEvent`), `DispatchPointer`, and `RenderFrame` (invoke the guest,
  parse its `dart:ui` scene JSON, and build a `DisplayList`).
- To wire it into a shell: construct an `ElpianRuntime` where a `RuntimeController`
  would own a `DartIsolate`; forward `PlatformDispatcher` pointer packets to
  `DispatchPointer`; and, on `onBeginFrame`/`onDrawFrame`, submit
  `RenderFrame()`'s `DisplayList` to the `LayerTree` the compositor already
  consumes. No rasterizer change is needed — the output is a standard
  `DisplayList`.

## Verification status — read this

- **Tested and proven:** the Elpian VM itself (93 VM tests + 86 runtime tests),
  its C ABI, and the scene-tree contract, which is already rasterized to real
  pixels in a headless browser (`elpian/web-demo`). The native static lib links
  and the same ABI is called there.
- **Not compiled in this checkout:** the C++ here has **not** been built against
  the engine, because a Flutter engine build needs `gclient sync` + `depot_tools`
  + `ninja` (hours, tens of GB) that this environment cannot run. It is written
  against the engine's real APIs (`DisplayListBuilder`, `DlPaint`, `DlColor`,
  `DlRect`/`DlPoint`, `rapidjson`) — i.e. the first-PR integration skeleton — and
  needs a full engine build (and staging `libelpian_dart.a`, see `BUILD.gn`) to
  compile and run in-engine.
- **Follow-ups:** `drawParagraph` → `DlBuilder::DrawText` (needs text layout via
  `ParagraphBuilder`); transform/clip/`SceneBuilder` layers → `Save`/`Restore`/
  `ClipRect`/`Transform`; and hosting `ElpianRuntime` under a real
  `RuntimeController` for the platform-view/lifecycle wiring.

The honest boundary: this makes the *engine-side* integration concrete and
correct against the real APIs, and it links the genuinely-tested VM — but running
the full `package:flutter` framework on it still requires the remaining language/
`dart:ui` conformance tracked in `elpian/README.md`.
