//! # elpian-vm
//!
//! The Elpian AST-based bytecode virtual machine, ported from the Elpian
//! project for use as the application logic core of the Elpa framework.
//!
//! ## Pipeline
//!
//! ```text
//! JS source ──(compiler::parse_js, in-VM front-end)──▶ Elpian AST JSON
//!           ──(compiler::compile_ast)───────────────▶ bytecode (Vec<u8>)
//!           ──(program::DecodedProgram::decode)──────▶ in-memory operation list
//!           ──(executor)──────────────────────────────▶ execution + host calls
//! ```
//!
//! An Elpa instance can therefore be created from JavaScript source just like
//! from a hand-written AST: the compiler module lowers JS to the very same
//! Elpian AST JSON and feeds it to the shared `from ast` compiler. An external
//! acorn/babel front-end may still be used to emit the AST directly, but is no
//! longer required.
//!
//! The front-end (JS/AST → bytecode) can run **ahead of time**: a tool compiles
//! the program to bytecode once at build time (`api::compile_js_to_bytecode`),
//! and the deployed app loads the bytecode straight into a VM
//! (`api::create_vm_from_bytecode`) — no parsing or AST work at startup. The
//! executor then decodes the bytecode **once**, at construction, into an
//! addressable in-memory list of operation objects (see [`sdk::program`]) and
//! traverses that on every step instead of re-parsing the raw bytes, so a
//! program that re-runs its render path every frame pays the decode cost only
//! once.
//!
//! The VM is a *pausing* interpreter: when user code calls
//! `askHost(apiName, payload)` it suspends and hands a host-call request back
//! to the embedder. The embedder (the Elpa runtime) services the call —
//! crucially `askHost("render", uiTree)` — and resumes the VM with
//! [`api::continue_execution`].
//!
//! This crate is renderer-agnostic. It knows nothing about wgpu; it only emits
//! host-call requests as JSON. The `elpa-runtime` crate wires those requests to
//! the `elpa-renderer`.
//!
//! See `PLAN.md` at the repository root for the full architecture.

pub mod api;
pub mod sdk;

pub use sdk::data::Val;
pub use sdk::vm::VM;
