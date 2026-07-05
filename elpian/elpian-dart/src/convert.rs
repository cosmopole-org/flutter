//! `dart:convert` — JSON, UTF-8, and Base64 codecs.
//!
//! A foundational, self-contained `dart:*` library used pervasively by app and
//! framework code (platform-channel payloads, asset decoding, HTTP bodies).
//! Implemented host-side and reached over the bridge; no external deps, so it
//! also compiles to `wasm32`.

use serde_json::{json, Value};

pub type OpResult = Result<Value, String>;

/// Dispatch a `dart:convert/<method>` call.
pub fn dispatch(method: &str, args: &[Value]) -> OpResult {
    match method {
        // jsonEncode(value) -> String
        "jsonEncode" => {
            let v = arg(args, 0)?;
            Ok(json!(v.to_string()))
        }
        // jsonDecode(source) -> value
        "jsonDecode" => {
            let s = as_str(args, 0)?;
            serde_json::from_str::<Value>(&s).map_err(|e| format!("FormatException: {e}"))
        }
        // utf8.encode(String) -> List<int> (bytes)
        "utf8.encode" => {
            let s = as_str(args, 0)?;
            Ok(json!(s.into_bytes().into_iter().map(|b| b as u64).collect::<Vec<_>>()))
        }
        // utf8.decode(List<int>) -> String
        "utf8.decode" => {
            let bytes = as_bytes(args, 0)?;
            String::from_utf8(bytes)
                .map(|s| json!(s))
                .map_err(|_| "FormatException: invalid UTF-8".to_string())
        }
        // base64.encode(List<int>) -> String
        "base64.encode" => {
            let bytes = as_bytes(args, 0)?;
            Ok(json!(base64_encode(&bytes)))
        }
        // base64.decode(String) -> List<int>
        "base64.decode" => {
            let s = as_str(args, 0)?;
            base64_decode(&s)
                .map(|b| json!(b.into_iter().map(|x| x as u64).collect::<Vec<_>>()))
                .map_err(|e| format!("FormatException: {e}"))
        }
        other => Err(format!("NoSuchMethodError: dart:convert/{other}")),
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let inv = |c: u8| -> Option<u32> {
        B64.iter().position(|&x| x == c).map(|p| p as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|&c| c != b'=' && !c.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        let mut bits = 0;
        for &c in chunk {
            let v = inv(c).ok_or_else(|| format!("invalid base64 char '{}'", c as char))?;
            n = (n << 6) | v;
            bits += 6;
        }
        // Emit the whole bytes represented by the accumulated bits.
        let bytes = bits / 8;
        n <<= (4 - chunk.len()) as u32 * 6;
        let be = n.to_be_bytes(); // [b0(hi), b1, b2, b3(lo)]  n occupies low 24 bits
        for i in 0..bytes {
            out.push(be[1 + i]);
        }
    }
    Ok(out)
}

fn arg<'a>(args: &'a [Value], i: usize) -> Result<&'a Value, String> {
    args.get(i).ok_or_else(|| format!("missing argument {i}"))
}
fn as_str(args: &[Value], i: usize) -> Result<String, String> {
    arg(args, i)?
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("argument {i} is not a string"))
}
fn as_bytes(args: &[Value], i: usize) -> Result<Vec<u8>, String> {
    let arr = arg(args, i)?
        .as_array()
        .ok_or_else(|| format!("argument {i} is not a byte list"))?;
    Ok(arr.iter().map(|v| v.as_u64().unwrap_or(0) as u8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let decoded = dispatch("jsonDecode", &[json!(r#"{"a":1,"b":[2,3]}"#)]).unwrap();
        assert_eq!(decoded["b"][1], 3);
        let encoded = dispatch("jsonEncode", &[json!({"x": 1})]).unwrap();
        assert_eq!(encoded, json!(r#"{"x":1}"#));
    }

    #[test]
    fn utf8_roundtrip() {
        let bytes = dispatch("utf8.encode", &[json!("hé")]).unwrap();
        // 'h' = 0x68, 'é' = 0xC3 0xA9
        assert_eq!(bytes, json!([0x68, 0xC3, 0xA9]));
        let back = dispatch("utf8.decode", &[bytes]).unwrap();
        assert_eq!(back, json!("hé"));
    }

    #[test]
    fn base64_roundtrip_and_known_vector() {
        // "Man" -> "TWFu" (classic RFC 4648 example).
        let enc = dispatch("base64.encode", &[json!([77, 97, 110])]).unwrap();
        assert_eq!(enc, json!("TWFu"));
        let dec = dispatch("base64.decode", &[json!("TWFu")]).unwrap();
        assert_eq!(dec, json!([77, 97, 110]));
        // padding case: "M" -> "TQ=="
        assert_eq!(dispatch("base64.encode", &[json!([77])]).unwrap(), json!("TQ=="));
        assert_eq!(dispatch("base64.decode", &[json!("TQ==")]).unwrap(), json!([77]));
    }
}
