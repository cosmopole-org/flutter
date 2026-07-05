# web-demo — Elpian VM running dynamic Dart in a headless browser

End-to-end proof that a **dynamically-delivered Dart miniapp runs on the Elpian
VM inside a real browser** and produces **actual rendered pixels**, verified
headlessly with Playwright.

Pipeline: `app.dart` → Elpian VM (compiled to `wasm32`, no wasm-bindgen) →
`dart:ui` scene tree → HTML canvas rasterizer → Playwright pixel assertions.

```
 app.dart ──▶ elpian_dart.wasm (VM) ──askHost("dart:ui/...")──▶ scene tree JSON
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
cargo build -p elpian-dart --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/elpian_dart.wasm web-demo/

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

## Honest scope

This is the **Elpian renderer**, not Flutter's CanvasKit/WebGL engine. It proves
*dynamic Dart → VM-in-browser → pixels* for the subset the VM supports. It is
**not** "the Flutter web build" (that is Google's `dart2js`/`dart2wasm` +
CanvasKit pipeline and is unrelated to this VM). The canvas rasterizer here
stands in for the native engine binding, which is the remaining integration to
run the full framework.
