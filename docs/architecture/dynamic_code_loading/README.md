# Dynamic Dart Code Loading — a "Miniapp" architecture for this Flutter fork

**Goal (as requested):** ship a *release* Flutter app that embeds the Dart VM,
downloads Dart application code as strings at runtime, loads it into that VM, and
runs it — so the app can be updated instantly, in production, without an app-store
rebuild, on **web, mobile, and desktop**.

This document is the result of tracing exactly how this repository decides between
JIT and AOT, and what it would take to build the "Miniapp" system on top of it. It
is grounded in the actual source in this tree (file/line references throughout),
gives an honest per-platform feasibility verdict, and specifies a concrete,
buildable architecture with reference code.

> **TL;DR**
> - "Release build that contains the Dart VM and loads Dart code dynamically" is
>   *already a first-class Flutter build mode*: **`jit_release`**
>   (`packages/flutter_tools/lib/src/build_info.dart:267`). It links the JIT VM
>   (`libdart_jit`) instead of the AOT runtime and loads kernel at runtime. It is a
>   perfectly good production Miniapp host **on Android and desktop**.
> - It **cannot** be your mechanism on **iOS** (Apple forbids JIT / W^X-violating
>   executable memory; this is why Flutter's official *Code Push* was cancelled) or
>   on **web** (there is no Dart VM on web — Dart compiles to JS/WASM).
> - Therefore a *single* "embed the Dart VM everywhere" mechanism is physically
>   impossible. The cross-platform Miniapp system must be a **hybrid**: a common
>   `MiniappHost` interface with a **JIT-kernel loader** on Android/desktop, an
>   **AOT-safe interpreter** on iOS, and a **JS/WASM module loader** on web.

---

## 1. How this repo actually chooses JIT vs AOT

The whole thing hinges on which Dart runtime library the engine links. That is
decided in one place:

**`engine/src/flutter/runtime/BUILD.gn`, `group("libdart")`:**

```gn
# Picks the libdart implementation based on the Flutter runtime mode.
group("libdart") {
  public_deps = []
  if (flutter_runtime_mode == "profile" || flutter_runtime_mode == "release") {
    public_deps += [ "$dart_src/runtime:libdart_aotruntime" ]   # AOT: no JIT, no kernel loader
  } else {
    public_deps += [
      "$dart_src/runtime:libdart_jit",                           # full JIT VM + kernel loader
      "//flutter/lib/snapshot",
    ]
  }
}
```

- `libdart_aotruntime` is the *precompiled runtime*. It contains no compiler and no
  kernel loader. It can only execute machine code that was AOT-compiled ahead of
  time into `libapp.so` / `App.framework`.
- `libdart_jit` is the full VM. It can ingest **Dart kernel** (`.dill`, the
  intermediate bytecode produced by the front-end compiler) at runtime and run it.
  This is what "loading Dart code dynamically" requires.

Which one is live at runtime is exposed as a **compile-time constant**:

**`engine/src/flutter/runtime/dart_vm.cc:177`:**
```cpp
bool DartVM::IsRunningPrecompiledCode() {
  return Dart_IsPrecompiledRuntime();   // true iff libdart_aotruntime was linked
}
```

And the isolate-startup path branches on it:

**`engine/src/flutter/runtime/isolate_configuration.cc`:**
- `AppSnapshotIsolateConfiguration` → `PrepareForRunningFromPrecompiledCode()` (AOT).
- `KernelIsolateConfiguration` / `KernelListIsolateConfiguration` →
  `PrepareForRunningFromKernel(...)`, but they **explicitly bail** when
  `DartVM::IsRunningPrecompiledCode()` is true:

```cpp
bool DoPrepareIsolate(DartIsolate& isolate) override {
  if (DartVM::IsRunningPrecompiledCode()) {
    return false;                       // kernel loading is impossible under AOT
  }
  return isolate.PrepareForRunningFromKernel(std::move(kernel_), /*child=*/false, /*last=*/true);
}
```

**Conclusion:** "embed the Dart VM and load Dart-from-string at runtime" == "link
`libdart_jit`, ship kernel, and take the `KernelIsolateConfiguration` path." Nothing
in the framework or engine forbids doing that in a release-*configured* build. In
fact Flutter already has a named mode for exactly this.

---

## 2. You (mostly) don't need to fork the engine: `jit_release`

**`packages/flutter_tools/lib/src/build_info.dart`:**
```dart
enum BuildMode {
  debug,
  profile,
  release,
  jitRelease;                                   // line 470

  static const releaseModes = <BuildMode>{release, jitRelease};   // line 477
  static const jitModes     = <BuildMode>{debug, jitRelease};     // line 478
}

static const jitRelease = BuildInfo(BuildMode.jitRelease, ...);   // line 267
bool get isJitRelease => mode == BuildMode.jitRelease;            // line 303
```

`jit_release` is a real, supported mode that is:
- **JIT** (`jitModes` contains it → links `libdart_jit`, ships kernel + VM/isolate
  snapshot data, takes the `KernelIsolateConfiguration` path), **and**
- **release-configured** (`releaseModes` contains it → optimizations on, asserts and
  the observatory/VM-service and debug-only checks off; note e.g.
  `common.dart` gates `trackWidgetCreation` off for release and the engine gates iOS
  `ptrace_check.cc` to debug only).

So on **Android and desktop**, a production Miniapp host is essentially:

```bash
# base "shell" app, release-configured, JIT VM embedded, loads kernel at runtime
flutter build apk       --jit-release      # or aar / bundle
flutter build linux     --jit-release      # windows / macos analogously
```

That binary contains the Dart VM and *will* accept kernel handed to it at runtime.
The remaining work is the **Miniapp runtime** (§4): getting updated code compiled to
kernel and swapping it into the running app.

> The bundling of the JIT runtime + kernel blob is currently gated to
> `BuildMode.debug` in `packages/flutter_tools/lib/src/build_system/targets/common.dart`
> ("Only copy the prebuilt runtimes and kernel blob in debug mode."). For
> Android/desktop, `jit_release` is wired through the platform targets; if you want a
> desktop bundle that carries the kernel blob the way debug does, that copy step is
> the surface to widen. See §5 for the minimal, opt-in engine/tool changes.

---

## 3. The hard per-platform truth

| Platform | Embed real Dart VM + JIT? | Load Dart-from-string at runtime? | Mechanism |
|---|---|---|---|
| **Android** | ✅ yes (`jit_release`) | ✅ yes, native kernel loading | JIT VM + kernel |
| **Windows / Linux / macOS (desktop)** | ✅ yes (`jit_release`) | ✅ yes, native kernel loading | JIT VM + kernel |
| **iOS / iPadOS** | ❌ **no** | ⚠️ only via an interpreter | AOT shell + AOT-safe interpreter |
| **Web** | ❌ **no VM exists** | ✅ but as JS/WASM, not kernel | JS/WASM module load or interpreter |

Two of these are non-negotiable physics/policy, not engineering effort:

### iOS — JIT is forbidden
Apple does not allow third-party apps to allocate writable-then-executable memory
(`mmap` with `PROT_EXEC` on anonymous/JIT pages). The Dart JIT must generate machine
code at runtime, which requires exactly that. Consequences:
- The engine already only links `libdart_jit` for non-release iOS, and `jit_release`
  on iOS is not a shippable App Store path.
- App Store Review Guideline **2.5.2** independently forbids downloading and executing
  code that changes app behavior.
- This is precisely why Flutter's official **Code Push** effort (2019) was cancelled.

The only App-Store-legal way to run downloaded logic on iOS is an **interpreter that
does not generate native code** — it is itself AOT-compiled into your shell and walks
a syntax tree / bytecode. (This is the same category Apple permits for JavaScript
via `JavaScriptCore`.)

### Web — there is no Dart VM
On web, Dart is compiled by `dart2js` (JavaScript) or `dart2wasm` (WebAssembly);
there is no VM, no kernel loader, `Dart_IsPrecompiledRuntime` doesn't apply. "Dynamic
code" on web means **loading a JS/WASM module** (e.g. a `<script>` / dynamic
`import()` of a `dart2js`-compiled unit) or running the same interpreter you use for
iOS, compiled to JS.

**Implication for the design:** stop trying to make one mechanism span all platforms.
Define one *interface* (`MiniappHost`) and give it three implementations.

---

## 4. Recommended architecture: a hybrid `MiniappHost`

```
                    ┌──────────────────────────────────────────────┐
                    │  Base "shell" app (shipped to the store once) │
                    │  - Flutter engine + Dart runtime              │
                    │  - MiniappHost + platform loader              │
                    │  - bootstrap UI / update checker              │
                    └───────────────┬──────────────────────────────┘
                                    │  fetch signed code bundle
                                    ▼
        ┌──────────────────────── Code delivery service ───────────────────────┐
        │  compile Dart source ──► per-platform artifact, signed + versioned    │
        │   • VM platforms  : frontend_server → kernel (.dill)                  │
        │   • iOS           : source/AST → interpreter bytecode                 │
        │   • web           : dart2js/dart2wasm → JS/WASM module                │
        └───────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
              MiniappHost.load(bundle) ─► returns a Widget subtree
                    the shell mounts / hot-swaps at runtime
```

### 4.1 The common interface

```dart
/// Contract every platform loader satisfies. The shell talks only to this.
abstract interface class MiniappHost {
  /// Fetch + verify + load a code bundle, returning the miniapp's root builder.
  /// Implementations must verify the bundle signature before executing anything.
  Future<MiniappHandle> load(MiniappBundle bundle);
}

class MiniappBundle {
  const MiniappBundle({
    required this.id,
    required this.version,
    required this.entrypoint,   // e.g. 'package:miniapp/main.dart' or a symbol name
    required this.bytes,        // kernel .dill | interpreter bytecode | JS/WASM
    required this.signature,    // detached signature over `bytes`
  });
  final String id;
  final int version;
  final String entrypoint;
  final Uint8List bytes;
  final Uint8List signature;
}

abstract interface class MiniappHandle {
  /// Build the miniapp's UI. The shell mounts this in its widget tree.
  Widget build(BuildContext context);
  Future<void> dispose();
}
```

### 4.2 VM platforms (Android + desktop): kernel loader

On a `jit_release` shell the VM will run kernel directly. The clean, supported
primitive is a fresh **isolate** spawned from the downloaded kernel, communicating
with the shell over a `SendPort` (UI is described declaratively and rebuilt in the
shell isolate, or the miniapp drives its own engine view). Reference:

```dart
// vm_loader.dart  (Android/desktop only; requires a jit_release engine)
import 'dart:isolate';
import 'dart:ui' show ... ;

class VmKernelHost implements MiniappHost {
  @override
  Future<MiniappHandle> load(MiniappBundle bundle) async {
    _verify(bundle);                                   // reject unsigned/altered code
    final dillUri = await _writeToCache(bundle);       // persist .dill to app dir
    final recv = ReceivePort();
    // Spawn the downloaded code as a child isolate in THIS (JIT) VM.
    final isolate = await Isolate.spawnUri(
      dillUri,                                          // the kernel we just fetched
      const <String>[],
      recv.sendPort,
    );
    final channel = await recv.first as SendPort;       // miniapp handshake
    return _IsolateHandle(isolate, channel);
  }
}
```

> Notes / honest caveats:
> - `Isolate.spawnUri` with a `.dill` works **only** under the JIT runtime; under the
>   AOT runtime it throws (which is the `IsRunningPrecompiledCode()` guard in §1). That
>   is exactly why the shell must be `jit_release`, not `release`.
> - Cross-isolate UI needs a protocol: either (a) the miniapp emits a serialized
>   widget description the shell renders, or (b) you register a second `FlutterView`.
>   Option (a) is simpler and is what most production "server-driven UI + logic"
>   systems do; (b) is heavier but gives the miniapp real imperative Flutter.
> - Simplest of all for a single-app "swap the whole UI" model: skip child isolates,
>   have the shell's `main()` load kernel for its *own* isolate via the engine's
>   `RunBundleAndSnapshot` path and restart the root widget. That needs the small
>   engine/tool plumbing in §5.

### 4.3 iOS: AOT-safe interpreter

Ship an interpreter compiled into the AOT shell. Download **interpreter bytecode**
(or Dart source and parse on-device) and execute it against a binding layer that maps
interpreted calls onto real Flutter widgets. This is the `dart_eval` / `hetu_script` /
Tencent MXFlutter / 58-Fair category. The `MiniappHost` implementation:

```dart
// interpreter_loader.dart  (iOS; also a universal fallback, incl. web)
class InterpreterHost implements MiniappHost {
  final DartInterpreter _vm;                 // pure-Dart, AOT-safe, no codegen
  @override
  Future<MiniappHandle> load(MiniappBundle bundle) async {
    _verify(bundle);
    final program = _vm.loadBytecode(bundle.bytes);   // or _vm.parse(source)
    return _InterpreterHandle(_vm, program.entrypoint(bundle.entrypoint));
  }
}
```

Trade-off: interpreted code is slower than JIT/AOT and only exposes the API surface
you bind. Keep hot paths (rendering, animations, heavy compute) in the AOT shell and
put *business logic + declarative UI* in the interpreted miniapp.

### 4.4 Web: JS/WASM module loader

Compile each miniapp with `dart2js`/`dart2wasm` to a standalone module and load it at
runtime; or run the §4.3 interpreter compiled to JS. Loading arbitrary JS at runtime
is native to the platform (`import()` / injected `<script>`), so web is the *easiest*
platform for dynamic code — it just isn't "the Dart VM."

### 4.5 Getting code *as a string* into kernel

For VM platforms the compiler that turns Dart **source strings** into loadable kernel
is `frontend_server` (the same one hot reload uses). This repo already drives it:
`packages/flutter_tools/lib/src/compile.dart` (`ResidentCompiler`,
`compileExpression`) and `devfs.dart`. In production you run this **server-side** in
your code-delivery service (not on-device): push source → `frontend_server` →
signed `.dill` → device. The device only *loads* kernel, it doesn't compile it. This
keeps the front-end compiler off the client and keeps bundles small.

---

## 5. If you do want to fork the engine (minimal, opt-in changes)

Most teams should stay on stock `jit_release` for Android/desktop and not fork. But if
you want a release-*named* build (not `jit_release`) to embed the JIT VM — e.g. to get
release packaging/signing while still loading kernel — the changes are small and
should be **opt-in behind a GN arg** so default builds are untouched:

1. **Engine link rule** — `engine/src/flutter/runtime/BUILD.gn`, `group("libdart")`:
   introduce a GN arg `flutter_dynamic_release` (default `false`) and, when set, link
   `libdart_jit` + `//flutter/lib/snapshot` even for `release`:

   ```gn
   declare_args() { flutter_dynamic_release = false }

   group("libdart") {
     public_deps = []
     if ((flutter_runtime_mode == "profile" || flutter_runtime_mode == "release") &&
         !flutter_dynamic_release) {
       public_deps += [ "$dart_src/runtime:libdart_aotruntime" ]
     } else {
       public_deps += [
         "$dart_src/runtime:libdart_jit",
         "//flutter/lib/snapshot",
       ]
     }
   }
   ```

2. **Tool packaging** — widen the "copy prebuilt runtimes + kernel blob" step in
   `packages/flutter_tools/lib/src/build_system/targets/common.dart` (currently
   `if (buildMode == BuildMode.debug)`) to also fire for your dynamic-release build,
   and take the `KernelSnapshot()` target instead of `AotElfRelease` when the flag is
   set.

3. **Nothing in `isolate_configuration.cc` / `dart_vm.cc` needs to change** — once
   `libdart_jit` is linked, `Dart_IsPrecompiledRuntime()` is false and the existing
   `KernelIsolateConfiguration` path just works.

**Reality check on cost:** this requires a full custom engine build
(`gclient sync` + `//flutter/tools/gn` + `ninja`, hours + tens of GB) and you then
ship a custom engine with your app. `jit_release` avoids all of that. Recommend the
fork **only** if release-named packaging is a hard requirement; otherwise §4.2 on
stock `jit_release` is strictly less work.

---

## 6. Security & store-policy checklist (do not skip)

Downloading and running code is exactly the thing platforms police. Before this ships:

- **Sign every bundle** and verify the signature on-device *before* loading (the
  `_verify(bundle)` calls above are not optional). Otherwise you have a remote-code-
  execution backdoor.
- **iOS/App Store**: only the interpreter path (no native codegen) is compliant with
  guideline 2.5.2, and even then downloaded code must not change the app's advertised
  purpose. Do not attempt to ship a JIT/`jit_release` iOS build to the App Store — it
  will be rejected and won't run without special entitlements/jailbreak.
- **Google Play**: dynamic code loading is allowed but must not be the vector for
  policy evasion; the interpreter or `jit_release` kernel path is fine if bundles are
  yours and signed.
- **Transport**: HTTPS + certificate pinning + version pinning + rollback. Treat the
  code-delivery service as security-critical infrastructure.

---

## 7. Recommended path for this fork

1. **Prove the loop on Android/desktop first** with stock `jit_release` and the §4.2
   isolate-kernel host — no engine fork. This gives you instant code updates on 4 of 6
   platforms with the least risk.
2. **Add the interpreter host** (§4.3) as the universal fallback; this is what unlocks
   **iOS** and doubles as a **web** option.
3. **Add the web JS/WASM loader** (§4.4) for best web performance.
4. Only consider the **engine fork** (§5) if you specifically need release-named
   builds to carry the JIT VM.
5. Stand up the **server-side `frontend_server` compile + signing** service (§4.5, §6).

This staging gets you a working cross-platform Miniapp system without betting the
whole thing on the one path (iOS JIT) that physically cannot work.

---

### Source references (this tree)
- `engine/src/flutter/runtime/BUILD.gn` — `group("libdart")` JIT/AOT link decision.
- `engine/src/flutter/runtime/dart_vm.cc:177` — `IsRunningPrecompiledCode()`.
- `engine/src/flutter/runtime/isolate_configuration.cc` — kernel vs app-snapshot startup; kernel path guarded by `IsRunningPrecompiledCode()`.
- `engine/src/flutter/runtime/dart_snapshot.cc` / `dart_snapshot.h` — snapshot sourcing.
- `packages/flutter_tools/lib/src/build_info.dart:267,303,470,477,478` — `BuildMode.jitRelease`, `jitModes`, `releaseModes`.
- `packages/flutter_tools/lib/src/build_system/targets/common.dart` — `KernelSnapshot` vs `AotElfRelease`; debug-only runtime/kernel-blob copy.
- `packages/flutter_tools/lib/src/compile.dart`, `devfs.dart` — `frontend_server`/`ResidentCompiler` (source → kernel).
