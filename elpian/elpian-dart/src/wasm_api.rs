//! Minimal C-ABI entry points so the VM can run **in a browser** as a
//! `wasm32-unknown-unknown` module — no `wasm-bindgen` required.
//!
//! Protocol (all UTF-8 bytes in the module's linear memory):
//! 1. JS calls [`elpian_alloc`] to reserve `len` bytes and writes the Dart
//!    source there.
//! 2. JS calls [`elpian_run`], which compiles + runs the program (with a fixed
//!    clock for determinism), captures the scene the guest submitted via
//!    `dart:ui/FlutterView.render`, stores the JSON result, and returns its
//!    length.
//! 3. JS reads [`elpian_result_ptr`]`..+len` from memory to get the scene JSON.
//!
//! This is the seam a browser page (or the real engine embedder) renders from.

use std::sync::Mutex;

use crate::{DartCapabilitySet, DartRuntime, ResourceMeter};

static RESULT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Reserve `len` bytes in wasm memory and return a pointer the host writes to.
#[no_mangle]
pub extern "C" fn elpian_alloc(len: usize) -> *mut u8 {
    let mut buf: Vec<u8> = Vec::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Free a buffer previously returned by [`elpian_alloc`].
///
/// # Safety
/// `ptr`/`len` must come from a prior [`elpian_alloc`] call.
#[no_mangle]
pub unsafe extern "C" fn elpian_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len));
}

/// Compile + run the Dart source at `ptr..+len`, capture the submitted scene,
/// store its JSON, and return the JSON byte length. On error, stores an
/// `{"error": "..."}` object instead.
///
/// # Safety
/// `ptr`/`len` must describe valid UTF-8 bytes in memory.
#[no_mangle]
pub unsafe extern "C" fn elpian_run(ptr: *const u8, len: usize) -> usize {
    let src = std::slice::from_raw_parts(ptr, len);
    let src = std::str::from_utf8(src).unwrap_or("");
    let json = run_to_scene_json(src);
    let bytes = json.into_bytes();
    let n = bytes.len();
    *RESULT.lock().unwrap() = bytes;
    n
}

/// Pointer to the result bytes stored by the last [`elpian_run`].
#[no_mangle]
pub extern "C" fn elpian_result_ptr() -> *const u8 {
    RESULT.lock().unwrap().as_ptr()
}

fn run_to_scene_json(src: &str) -> String {
    // Each browser run needs a fresh VM id so the global registry doesn't collide.
    let id = format!("web-{}", next_id());
    let rt = DartRuntime::from_dart(
        id,
        src,
        DartCapabilitySet::full(),
        ResourceMeter::unbounded(),
    );
    let mut rt = match rt {
        Ok(rt) => rt.with_fixed_clock(0),
        Err(e) => return format!("{{\"error\":\"compile: {e:?}\"}}"),
    };
    if rt.run().is_err() {
        return "{\"error\":\"runtime\"}".to_string();
    }
    match rt.last_scene() {
        Some(scene) => scene.to_string(),
        None => "{\"error\":\"no scene submitted (call dart:ui/FlutterView.render)\"}".to_string(),
    }
}

fn next_id() -> u64 {
    static COUNTER: Mutex<u64> = Mutex::new(0);
    let mut c = COUNTER.lock().unwrap();
    *c += 1;
    *c
}
