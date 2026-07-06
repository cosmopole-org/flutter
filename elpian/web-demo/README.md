# web-demo — Elpian VM running dynamic Dart in a headless browser

End-to-end proof that a **dynamically-delivered Dart miniapp runs on the Elpian
VM inside a real browser** and produces **actual rendered pixels**, verified
headlessly with Playwright.

Pipeline: `app.dart` → Elpian VM (compiled to `wasm32`, no wasm-bindgen) →
`dart:ui` scene tree → HTML canvas rasterizer → Playwright pixel assertions.

```
 app.dart ──▶ dart.wasm (VM) ──askHost("dart:ui/...")──▶ scene tree JSON
                                                                      │
                          Playwright (headless Chromium) ◀── canvas ◀─┘
                          asserts red/green/blue swatches + await-driven circle
```

## What the miniapp exercises

`app.dart` uses classes, arrow-body methods, list indexing, a `for` loop, and
**`async`/`await`** (the circle's colour is delivered through an awaited
`Future`, so the frame is only complete after the microtask loop runs). It then
paints via the `dart:ui` bridge.

## Run it

```sh
# 1. Build the VM to wasm and copy it here
cd elpian
cargo build -p dart --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/dart.wasm web-demo/

# 2. Run the headless end-to-end test (serves the dir, drives Chromium, asserts pixels)
cd web-demo
node test.mjs      # -> "E2E PASSED", writes rendered.png
```

The test fails the process (non-zero exit) if any swatch/circle pixel is wrong
or the canvas is blank, so it doubles as CI.

### Interactive variant

`counter.dart` + `interactive.html` + `interactive_test.mjs` demonstrate the
**event loop**: a tappable button whose real browser clicks (`page.mouse.click`)
run the VM's `onPointerEvent` handler, mutate a counter, and re-render. The
persistent-runtime wasm API (`elpian_init` / `elpian_pointer` / `elpian_frame`)
keeps VM state across frames.

```sh
node interactive_test.mjs    # clicks the button, asserts the bar grows -> INTERACTIVE E2E PASSED
```

### Real widget-code variant

`widgets_app.dart` + `widgets.html` + `widgets_test.mjs` run an app authored as
**actual Flutter-style widgets** — no raw `dart:ui` calls, just
`StatelessWidget`/`StatefulWidget`, `build()`, nested children, and a
`GestureDetector`. The widget framework (prepended via the `elpian_init_widgets`
export) builds, lays out, and paints the tree into the same scene the canvas
rasterizes; real clicks run `onTap → setState` and the next frame reflects the
new state.

```sh
node widgets_test.mjs        # taps the widget button, asserts the bar tracks state -> WIDGETS E2E PASSED
```

### Full `flutter.dart` app variant

`flutter.html` + `flutter_test.mjs` run [`demo_app.dart`](../dart/flutter/demo_app.dart)
— a realistic app that `import 'flutter.dart'` (the full idiomatic widget
library) and builds a `MaterialApp` → `Scaffold` → `AppBar` with a counter
`Card`, `+`/`-` `ElevatedButton`s, a progress bar, and stat chips. The library
is baked into the wasm and prepended by the `elpian_init_flutter` export; real
clicks drive `setState` and the Material UI repaints.

```sh
node flutter_test.mjs        # taps +/-, asserts the counter/derived stats update -> FLUTTER APP E2E PASSED
```

### Real Skia via CanvasKit — `canvaskit_bridge.js`

The demos above rasterize with a tiny 2D-canvas stand-in. This variant instead
drives **real Skia** through **CanvasKit** (Skia compiled to WASM — the exact
renderer Flutter web uses). [`canvaskit_bridge.js`](canvaskit_bridge.js) is a
**reflective interpreter**: it can construct any CanvasKit object, call any
static factory or instance method, resolve any enum/constant, and marshal every
Skia argument shape — all *by name* — so it covers the **entire Skia API with no
exceptions** (including future symbols). `auditCoverage()` walks the loaded
`CanvasKit` and asserts every symbol is reachable.

Two entry points: `paintScene(scene)` replays an Elpian widget scene on Skia
(with **real text layout** via the Paragraph API), and `runProgram(program)`
executes a raw "Skia program" — the full-power path a guest emits.

- `canvaskit.html` + `canvaskit_test.mjs` — the `flutter.dart` app rasterized by
  real Skia (interactive), a reflective full-API showcase (gradients, Bézier
  paths, mask/image-filter blur, `saveLayer`, shaped text), and the coverage
  audit.
- `skia_vm.html` + `skia_vm_test.mjs` — **guest bytecode drives Skia directly**:
  [`skia_guest.dart`](skia_guest.dart) (no widget framework) emits a reflective
  Skia program over `dart:ui`, and CanvasKit rasterizes it; taps mutate guest
  state and re-emit.

CanvasKit's runtime (`canvaskit.js` + `canvaskit.wasm`) and a font are not
checked in; fetch them once:

```sh
mkdir -p canvaskit && cd canvaskit
V=0.39.1
curl -sSL -o canvaskit.js   "https://unpkg.com/canvaskit-wasm@$V/bin/canvaskit.js"
curl -sSL -o canvaskit.wasm "https://unpkg.com/canvaskit-wasm@$V/bin/canvaskit.wasm"
cp /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf font.ttf   # any TTF works
cd ..
node canvaskit_test.mjs      # -> CANVASKIT E2E PASSED  (writes canvaskit.png)
node skia_vm_test.mjs        # -> SKIA-VM E2E PASSED    (writes skia_vm.png)
```

## Honest scope

The 2D-canvas demos are a **stand-in** rasterizer; the CanvasKit variant is the
**real** one — genuine Skia, the same engine Flutter web ships, driven by the
Elpian VM's output (and, in `skia_vm`, by raw guest ops). What this is *not* is
Google's `dart2js`/`dart2wasm` Flutter-web build; here the Elpian VM (not the
Dart VM) runs the app, and the bridge — not Flutter's engine glue — connects it
to CanvasKit. On native (iOS/Android/desktop) the same scene/program contract
would target native Skia/Impeller via the engine C++ layer instead.
