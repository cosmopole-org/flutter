// Copyright 2013 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_RUNTIME_ELPIAN_ELPIAN_FFI_H_
#define FLUTTER_RUNTIME_ELPIAN_ELPIAN_FFI_H_

#include <cstddef>
#include <cstdint>

// C ABI of the Elpian VM (Rust crate `elpian-dart`, built as a staticlib and
// linked into the engine). These are the same `#[no_mangle] extern "C"` symbols
// the wasm/browser build exposes, so the *runtime* the engine embeds is exactly
// the one covered by the crate's test suite — the C++ here is only glue.
//
// Contract (all bytes are UTF-8 in the process heap the crate manages):
//   * `elpian_alloc(n)` reserves n bytes and returns a pointer to write into.
//   * `elpian_init(ptr, len)` compiles + runs the bundle at ptr..+len (defining
//     its onPointerEvent / onDrawFrame handlers) and keeps the runtime live.
//   * `elpian_pointer(x, y, down)` delivers a pointer event to the guest.
//   * `elpian_frame()` renders one frame and stores the scene-tree JSON,
//     returning its byte length; read it at `elpian_result_ptr()`.

extern "C" {

uint8_t* elpian_alloc(size_t len);
void elpian_free(uint8_t* ptr, size_t len);

// Returns 0 on success, 1 on compile/load failure.
int32_t elpian_init(const uint8_t* ptr, size_t len);
void elpian_pointer(double x, double y, int32_t down);
size_t elpian_frame();
const uint8_t* elpian_result_ptr();

}  // extern "C"

#endif  // FLUTTER_RUNTIME_ELPIAN_ELPIAN_FFI_H_
