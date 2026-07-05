// Copyright 2013 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_RUNTIME_ELPIAN_ELPIAN_RUNTIME_H_
#define FLUTTER_RUNTIME_ELPIAN_ELPIAN_RUNTIME_H_

#include <string>

#include "flutter/display_list/display_list.h"
#include "flutter/fml/macros.h"

namespace flutter {

// Embeds the Elpian VM as an alternative execution layer for **dynamically
// delivered** Flutter bundles. Where the normal path runs a Dart isolate
// (`DartIsolate`) against an AOT snapshot or kernel, this runs a bundle on the
// Elpian VM: no JIT (App-Store-safe), reloadable at runtime, and the guest
// drives rendering by emitting a `dart:ui` scene tree that this class lowers to
// a `DisplayList` for the engine's compositor.
//
// The rendering/logic contract is identical to the framework's: the guest is
// invoked per-frame (`onDrawFrame`) and per-input (`onPointerEvent`), and the
// widget/scene specification it produces decides what is drawn — so different
// bundles render different widgets purely from their own specification and
// logic, with no engine rebuild.
class ElpianRuntime {
 public:
  ElpianRuntime();
  ~ElpianRuntime();

  // Compile + run a bundle (Dart-subset source or, later, Elpian bytecode),
  // defining its handlers. Returns false on compile/load failure.
  bool LoadBundle(const std::string& source);

  // Compile + run a bundle authored as a *Flutter-style widget app* — a program
  // of StatelessWidget/StatefulWidget classes with build() methods and a main()
  // that calls runApp(...). The widget framework is prepended by the VM layer,
  // so the guest's handlers build/lay out/paint the widget tree and route taps.
  // Returns false on compile/load failure.
  bool LoadWidgetApp(const std::string& source);

  // Deliver a pointer event to the guest's `onPointerEvent` handler.
  void DispatchPointer(double x, double y, bool down);

  // Produce one frame: invoke the guest's frame handler and lower the scene it
  // submits into a DisplayList the rasterizer paints. Returns nullptr if the
  // guest rendered nothing.
  sk_sp<DisplayList> RenderFrame();

 private:
  bool loaded_ = false;

  FML_DISALLOW_COPY_AND_ASSIGN(ElpianRuntime);
};

}  // namespace flutter

#endif  // FLUTTER_RUNTIME_ELPIAN_ELPIAN_RUNTIME_H_
