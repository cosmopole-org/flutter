//! `dart:typed_data` — foundational byte-buffer library.
//!
//! `typed_data` is a "group 3" foundational library (a native part of the Dart
//! runtime, not written in user Dart) that the Flutter framework leans on
//! everywhere — every `Image`, `Path`, vertex list, and platform-channel message
//! is `ByteData`/`Uint8List` underneath. It is, however, *self-contained*: it is
//! pure memory with no GPU/OS dependency, which makes it the natural first
//! group-3 library to implement completely.
//!
//! Model: the guest holds opaque integer buffer handles; the bytes live host-
//! side in this store. Guest calls arrive as
//! `dart:typed_data/ByteData.<op>` with a JSON argument array. This mirrors how
//! the Dart VM keeps the backing store native and hands the guest a view object.

use std::collections::HashMap;

use serde_json::{json, Value};

/// Host-side store of byte buffers, keyed by the handle the guest holds.
#[derive(Debug, Default)]
pub struct TypedDataStore {
    buffers: HashMap<u32, Vec<u8>>,
    next_id: u32,
}

/// Result of a typed_data op: either a JSON reply for the guest, or a Dart error
/// message (e.g. `RangeError`) to surface as a thrown exception.
pub type OpResult = Result<Value, String>;

impl TypedDataStore {
    pub fn new() -> Self {
        TypedDataStore::default()
    }

    /// Number of live buffers — used by the runtime to account memory.
    pub fn total_bytes(&self) -> u64 {
        self.buffers.values().map(|b| b.len() as u64).sum()
    }

    fn alloc(&mut self, len: usize) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.buffers.insert(id, vec![0u8; len]);
        id
    }

    fn buf(&self, id: u32) -> Result<&Vec<u8>, String> {
        self.buffers
            .get(&id)
            .ok_or_else(|| format!("StateError: no ByteData for handle {id}"))
    }

    fn buf_mut(&mut self, id: u32) -> Result<&mut Vec<u8>, String> {
        self.buffers
            .get_mut(&id)
            .ok_or_else(|| format!("StateError: no ByteData for handle {id}"))
    }

    /// Dispatch a `ByteData.<method>` call. `method` is the segment after the
    /// `dart:typed_data/` prefix; `args` is the guest's JSON argument array.
    ///
    /// Supported (the load-bearing subset): `ByteData.alloc(len) -> handle`,
    /// `lengthInBytes(handle) -> int`, and get/set for `Uint8`, `Int32`, and
    /// `Float64` with an explicit little-endian flag (Dart's `Endian` argument).
    pub fn dispatch(&mut self, method: &str, args: &[Value]) -> OpResult {
        match method {
            "ByteData.alloc" => {
                let len = as_usize(args, 0)?;
                Ok(json!(self.alloc(len)))
            }
            "ByteData.lengthInBytes" => {
                let id = as_u32(args, 0)?;
                Ok(json!(self.buf(id)?.len()))
            }
            "ByteData.setUint8" => {
                let (id, off, v) = (as_u32(args, 0)?, as_usize(args, 1)?, as_i64(args, 2)?);
                let b = self.buf_mut(id)?;
                bounds(b.len(), off, 1)?;
                b[off] = v as u8;
                Ok(Value::Null)
            }
            "ByteData.getUint8" => {
                let (id, off) = (as_u32(args, 0)?, as_usize(args, 1)?);
                let b = self.buf(id)?;
                bounds(b.len(), off, 1)?;
                Ok(json!(b[off] as u64))
            }
            "ByteData.setInt32" => {
                let (id, off, v) = (as_u32(args, 0)?, as_usize(args, 1)?, as_i64(args, 2)?);
                let little = as_bool(args, 3);
                let bytes = (v as i32).to_le_bytes();
                let bytes = if little { bytes } else { swap4(bytes) };
                let b = self.buf_mut(id)?;
                bounds(b.len(), off, 4)?;
                b[off..off + 4].copy_from_slice(&bytes);
                Ok(Value::Null)
            }
            "ByteData.getInt32" => {
                let (id, off) = (as_u32(args, 0)?, as_usize(args, 1)?);
                let little = as_bool(args, 2);
                let b = self.buf(id)?;
                bounds(b.len(), off, 4)?;
                let mut raw = [0u8; 4];
                raw.copy_from_slice(&b[off..off + 4]);
                if !little {
                    raw = swap4(raw);
                }
                Ok(json!(i32::from_le_bytes(raw)))
            }
            "ByteData.setFloat64" => {
                let (id, off) = (as_u32(args, 0)?, as_usize(args, 1)?);
                let v = as_f64(args, 2)?;
                let little = as_bool(args, 3);
                let bytes = v.to_le_bytes();
                let bytes = if little { bytes } else { swap8(bytes) };
                let b = self.buf_mut(id)?;
                bounds(b.len(), off, 8)?;
                b[off..off + 8].copy_from_slice(&bytes);
                Ok(Value::Null)
            }
            "ByteData.getFloat64" => {
                let (id, off) = (as_u32(args, 0)?, as_usize(args, 1)?);
                let little = as_bool(args, 2);
                let b = self.buf(id)?;
                bounds(b.len(), off, 8)?;
                let mut raw = [0u8; 8];
                raw.copy_from_slice(&b[off..off + 8]);
                if !little {
                    raw = swap8(raw);
                }
                Ok(json!(f64::from_le_bytes(raw)))
            }
            other => Err(format!("NoSuchMethodError: dart:typed_data/{other}")),
        }
    }
}

fn bounds(len: usize, off: usize, width: usize) -> Result<(), String> {
    if off + width > len {
        Err(format!(
            "RangeError: offset {off}+{width} out of range for ByteData of {len} bytes"
        ))
    } else {
        Ok(())
    }
}

fn swap4(mut b: [u8; 4]) -> [u8; 4] {
    b.reverse();
    b
}

fn swap8(mut b: [u8; 8]) -> [u8; 8] {
    b.reverse();
    b
}

fn arg<'a>(args: &'a [Value], i: usize) -> Result<&'a Value, String> {
    args.get(i)
        .ok_or_else(|| format!("missing argument {i}"))
}

fn as_usize(args: &[Value], i: usize) -> Result<usize, String> {
    arg(args, i)?
        .as_u64()
        .map(|v| v as usize)
        .ok_or_else(|| format!("argument {i} is not a non-negative integer"))
}

fn as_u32(args: &[Value], i: usize) -> Result<u32, String> {
    arg(args, i)?
        .as_u64()
        .map(|v| v as u32)
        .ok_or_else(|| format!("argument {i} is not a handle"))
}

fn as_i64(args: &[Value], i: usize) -> Result<i64, String> {
    arg(args, i)?
        .as_i64()
        .ok_or_else(|| format!("argument {i} is not an integer"))
}

fn as_f64(args: &[Value], i: usize) -> Result<f64, String> {
    arg(args, i)?
        .as_f64()
        .ok_or_else(|| format!("argument {i} is not a number"))
}

fn as_bool(args: &[Value], i: usize) -> bool {
    args.get(i).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int32_roundtrips_little_endian() {
        let mut s = TypedDataStore::new();
        let h = s.dispatch("ByteData.alloc", &[json!(8)]).unwrap();
        let h = h.as_u64().unwrap() as i64;
        s.dispatch("ByteData.setInt32", &[json!(h), json!(0), json!(1234567), json!(true)])
            .unwrap();
        let got = s
            .dispatch("ByteData.getInt32", &[json!(h), json!(0), json!(true)])
            .unwrap();
        assert_eq!(got, json!(1234567));
    }

    #[test]
    fn endianness_is_honored() {
        let mut s = TypedDataStore::new();
        let h = s.dispatch("ByteData.alloc", &[json!(4)]).unwrap().as_u64().unwrap() as i64;
        s.dispatch("ByteData.setInt32", &[json!(h), json!(0), json!(1), json!(false)])
            .unwrap();
        // Big-endian 1 => bytes 00 00 00 01 => byte[3] == 1.
        let b3 = s
            .dispatch("ByteData.getUint8", &[json!(h), json!(3)])
            .unwrap();
        assert_eq!(b3, json!(1));
    }

    #[test]
    fn float64_roundtrips() {
        let mut s = TypedDataStore::new();
        let h = s.dispatch("ByteData.alloc", &[json!(8)]).unwrap().as_u64().unwrap() as i64;
        s.dispatch("ByteData.setFloat64", &[json!(h), json!(0), json!(3.5), json!(true)])
            .unwrap();
        let got = s
            .dispatch("ByteData.getFloat64", &[json!(h), json!(0), json!(true)])
            .unwrap();
        assert_eq!(got, json!(3.5));
    }

    #[test]
    fn out_of_range_is_a_range_error() {
        let mut s = TypedDataStore::new();
        let h = s.dispatch("ByteData.alloc", &[json!(2)]).unwrap().as_u64().unwrap() as i64;
        let err = s
            .dispatch("ByteData.setInt32", &[json!(h), json!(0), json!(1), json!(true)])
            .unwrap_err();
        assert!(err.contains("RangeError"), "got: {err}");
    }
}
