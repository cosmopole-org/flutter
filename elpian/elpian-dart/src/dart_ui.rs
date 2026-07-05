//! `dart:ui` — the canonical "group 3" wall library.
//!
//! In stock Flutter, `dart:ui` (`Canvas`, `Picture`, `Scene`, `SceneBuilder`,
//! `ParagraphBuilder`, `PlatformDispatcher`, …) is **not Dart source**: the
//! methods are native functions implemented in the C++ engine and bound into the
//! isolate via tonic/`Dart_SetNativeResolver`. That native binding is exactly
//! what a non-Dart VM cannot inherit for free, and is why "run the framework
//! kernel unchanged" is a native-integration problem, not a language problem.
//!
//! The way through — the one every shippable dynamic-Flutter system uses — is to
//! re-express the `dart:ui` surface as **host-bridge calls** that the embedder
//! services. The guest's `Canvas` calls are *recorded* into a serializable
//! display-list (a scene tree); `endRecording` hands that tree back, and the
//! real engine (native/AOT, iOS-legal) rasterizes it. The VM never generates
//! machine code, so there is no JIT and no App Store violation.
//!
//! This module implements a faithful slice of that: `PictureRecorder`/`Canvas`
//! recording a display-list of paint ops, returned as a JSON scene tree ready
//! for a native rasterizer (or the elpis protocol renderer) to consume.

use serde_json::{json, Value};

/// A single recorded paint operation — one node of the display list.
#[derive(Debug, Clone)]
struct PaintOp(Value);

/// Records `Canvas` operations into a display-list, mirroring the engine's
/// `PictureRecorder` → `Canvas` → `Picture` flow.
#[derive(Debug, Default)]
pub struct SceneRecorder {
    recording: bool,
    ops: Vec<PaintOp>,
    /// Completed pictures keyed by handle, awaiting composition into a scene.
    pictures: std::collections::HashMap<u32, Vec<Value>>,
    next_id: u32,
}

pub type OpResult = Result<Value, String>;

impl SceneRecorder {
    pub fn new() -> Self {
        SceneRecorder::default()
    }

    /// Total recorded ops across live pictures — used for resource accounting.
    pub fn op_count(&self) -> u64 {
        self.pictures.values().map(|p| p.len() as u64).sum::<u64>() + self.ops.len() as u64
    }

    /// Dispatch a `dart:ui/<Class.method>` call. Supported slice:
    /// `PictureRecorder.beginRecording`, `Canvas.drawRect`, `Canvas.drawCircle`,
    /// `Canvas.drawParagraph`, and `PictureRecorder.endRecording` (returns a
    /// picture handle whose display-list can be fetched with `Picture.toScene`).
    pub fn dispatch(&mut self, method: &str, args: &[Value]) -> OpResult {
        match method {
            "PictureRecorder.beginRecording" => {
                if self.recording {
                    return Err("StateError: PictureRecorder already recording".into());
                }
                self.recording = true;
                self.ops.clear();
                Ok(Value::Null)
            }
            "Canvas.drawRect" => {
                self.require_recording()?;
                // args: [left, top, right, bottom, colorArgb]
                let r = rect(args)?;
                let color = as_u64(args, 4)?;
                self.ops.push(PaintOp(json!({
                    "op": "drawRect",
                    "rect": r,
                    "color": color,
                })));
                Ok(Value::Null)
            }
            "Canvas.drawCircle" => {
                self.require_recording()?;
                // args: [cx, cy, radius, colorArgb]
                let cx = as_f64(args, 0)?;
                let cy = as_f64(args, 1)?;
                let radius = as_f64(args, 2)?;
                let color = as_u64(args, 3)?;
                self.ops.push(PaintOp(json!({
                    "op": "drawCircle",
                    "center": [cx, cy],
                    "radius": radius,
                    "color": color,
                })));
                Ok(Value::Null)
            }
            "Canvas.drawParagraph" => {
                self.require_recording()?;
                // args: [text, x, y, fontSize, colorArgb]
                let text = as_str(args, 0)?;
                let x = as_f64(args, 1)?;
                let y = as_f64(args, 2)?;
                let size = as_f64(args, 3)?;
                let color = as_u64(args, 4)?;
                self.ops.push(PaintOp(json!({
                    "op": "drawParagraph",
                    "text": text,
                    "offset": [x, y],
                    "fontSize": size,
                    "color": color,
                })));
                Ok(Value::Null)
            }
            "PictureRecorder.endRecording" => {
                self.require_recording()?;
                let id = self.next_id;
                self.next_id += 1;
                let ops: Vec<Value> = self.ops.drain(..).map(|o| o.0).collect();
                self.pictures.insert(id, ops);
                self.recording = false;
                Ok(json!(id))
            }
            "Picture.toScene" => {
                let id = as_u32(args, 0)?;
                let ops = self
                    .pictures
                    .get(&id)
                    .ok_or_else(|| format!("StateError: no Picture for handle {id}"))?;
                Ok(json!({ "root": { "op": "picture", "ops": ops } }))
            }
            other => Err(format!("NoSuchMethodError: dart:ui/{other}")),
        }
    }

    fn require_recording(&self) -> Result<(), String> {
        if self.recording {
            Ok(())
        } else {
            Err("StateError: Canvas op outside an active recording".into())
        }
    }
}

fn rect(args: &[Value]) -> Result<Value, String> {
    Ok(json!([
        as_f64(args, 0)?,
        as_f64(args, 1)?,
        as_f64(args, 2)?,
        as_f64(args, 3)?
    ]))
}

fn get<'a>(args: &'a [Value], i: usize) -> Result<&'a Value, String> {
    args.get(i).ok_or_else(|| format!("missing argument {i}"))
}

fn as_f64(args: &[Value], i: usize) -> Result<f64, String> {
    get(args, i)?
        .as_f64()
        .ok_or_else(|| format!("argument {i} is not a number"))
}

fn as_u64(args: &[Value], i: usize) -> Result<u64, String> {
    get(args, i)?
        .as_u64()
        .ok_or_else(|| format!("argument {i} is not a non-negative integer"))
}

fn as_u32(args: &[Value], i: usize) -> Result<u32, String> {
    as_u64(args, i).map(|v| v as u32)
}

fn as_str(args: &[Value], i: usize) -> Result<String, String> {
    get(args, i)?
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("argument {i} is not a string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_composes_a_scene() {
        let mut r = SceneRecorder::new();
        r.dispatch("PictureRecorder.beginRecording", &[]).unwrap();
        r.dispatch(
            "Canvas.drawRect",
            &[json!(0.0), json!(0.0), json!(100.0), json!(50.0), json!(4294901760u64)],
        )
        .unwrap();
        r.dispatch(
            "Canvas.drawCircle",
            &[json!(50.0), json!(25.0), json!(10.0), json!(4278190335u64)],
        )
        .unwrap();
        let pic = r.dispatch("PictureRecorder.endRecording", &[]).unwrap();
        let pic_id = pic.as_u64().unwrap() as i64;
        let scene = r.dispatch("Picture.toScene", &[json!(pic_id)]).unwrap();
        let ops = &scene["root"]["ops"];
        assert_eq!(ops.as_array().unwrap().len(), 2);
        assert_eq!(ops[0]["op"], "drawRect");
        assert_eq!(ops[1]["op"], "drawCircle");
    }

    #[test]
    fn canvas_op_outside_recording_is_a_state_error() {
        let mut r = SceneRecorder::new();
        let err = r
            .dispatch("Canvas.drawRect", &[json!(0.0), json!(0.0), json!(1.0), json!(1.0), json!(0)])
            .unwrap_err();
        assert!(err.contains("StateError"), "got: {err}");
    }
}
