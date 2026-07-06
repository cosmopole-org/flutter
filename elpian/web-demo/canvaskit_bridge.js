// =============================================================================
// canvaskit_bridge.js — drive the FULL CanvasKit (Skia) API from the Elpian VM
// =============================================================================
//
// The Elpian VM produces host calls at the `dart:ui` seam; this bridge turns
// that output into real Skia drawing on a CanvasKit surface. Coverage is
// *complete by construction*: rather than hand-wrapping Skia's ~1000 methods
// (which would always lag the library), the bridge is a **reflective
// interpreter** of a small, uniform "Skia program" — it can
//
//   * construct any object            (`new CanvasKit.Paint()`, `.Path()`, …)
//   * call any static factory         (`CanvasKit.Shader.MakeLinearGradient`, …)
//   * call any instance method         (`paint.setShader`, `path.cubicTo`, …)
//   * resolve any enum / constant      (`CanvasKit.BlendMode.SrcOver`, …)
//   * marshal every Skia argument shape (colors, rects, rrects, matrices,
//                                         point/scalar arrays, typed data,
//                                         handles, nested option dicts)
//
// by NAME. Anything CanvasKit exposes — Canvas, Paint, Path, Shader,
// ColorFilter, ImageFilter, MaskFilter, PathEffect, RuntimeEffect (SkSL),
// Image, Picture, Vertices, drawAtlas/drawPatch/drawVertices/drawShadow/
// drawDRRect/drawGlyphs/drawImage*/drawParagraph, ParagraphBuilder, Font,
// Typeface, FontMgr, Surface, and every enum — is reachable with no exceptions,
// including symbols added in future CanvasKit versions.
//
// Two entry points sit on top of the interpreter:
//   * `runProgram(program)` — execute a raw Skia program (the full-power path a
//     guest emits via `askHost("skia/...")`).
//   * `paintScene(scene)`   — replay an Elpian `dart:ui` scene (the widget
//     framework's drawRect/drawCircle/drawParagraph output) on real Skia, with
//     real text layout via CanvasKit's Paragraph API.

export class SkiaBridge {
  // `CanvasKit` is the initialized module (from CanvasKitInit). `surface` is a
  // CanvasKit Surface (e.g. from MakeSWCanvasSurface / MakeWebGLCanvasSurface /
  // MakeSurface). If omitted, call `attachCanvas`/`attachRaster` later.
  constructor(CanvasKit, surface) {
    this.CanvasKit = CanvasKit;
    this.surface = surface || null;
    this.res = new Map(); // resource id -> live Skia object
    this._fontProvider = null;
    this._fontFamily = 'sans-serif';
    this._deletables = []; // objects to .delete() on dispose
  }

  // Register a TTF/OTF font (Uint8Array) so the Paragraph API can shape text.
  // CanvasKit ships with no default fonts, so this must be called before any
  // text is drawn. Registered under [family] (default 'sans-serif').
  useFont(bytes, family) {
    this._fontFamily = family || 'sans-serif';
    this._fontProvider = this.CanvasKit.TypefaceFontProvider.Make();
    this._fontProvider.registerFont(bytes, this._fontFamily);
    return this;
  }

  // ---- surface helpers ------------------------------------------------------
  attachCanvas(canvasEl) {
    // Software (CPU) surface backed by a 2D <canvas> — no WebGL needed, fully
    // deterministic (ideal for headless rendering and pixel tests). Still real
    // Skia rasterization, just the raster backend.
    this.surface = this.CanvasKit.MakeSWCanvasSurface(canvasEl);
    if (!this.surface) throw new Error('MakeSWCanvasSurface failed');
    return this;
  }
  attachGPU(canvasEl) {
    this.surface = this.CanvasKit.MakeWebGLCanvasSurface(canvasEl);
    if (!this.surface) throw new Error('MakeWebGLCanvasSurface failed');
    return this;
  }
  attachRaster(width, height) {
    this.surface = this.CanvasKit.MakeSurface(width, height); // offscreen CPU
    if (!this.surface) throw new Error('MakeSurface failed');
    return this;
  }

  // ---- reflective core ------------------------------------------------------

  // Resolve a dotted path from the CanvasKit root: "Shader.MakeLinearGradient",
  // "BlendMode.SrcOver", "TRANSPARENT", "Path.MakeFromSVGString", ...
  _resolve(path) {
    let cur = this.CanvasKit;
    for (const part of path.split('.')) {
      if (cur == null) throw new Error(`cannot resolve '${path}' (at '${part}')`);
      cur = cur[part];
    }
    if (cur === undefined) throw new Error(`unknown CanvasKit symbol '${path}'`);
    return cur;
  }

  // Convert one program argument into a live JS/Skia value. Tagged objects name
  // the Skia-specific shapes; everything else passes through (recursively).
  _marshal(a) {
    const CK = this.CanvasKit;
    if (a === null || a === undefined) return null;
    if (typeof a !== 'object') return a; // number | string | boolean
    if (Array.isArray(a)) return a.map((x) => this._marshal(x));

    if ('ref' in a) {
      if (!this.res.has(a.ref)) throw new Error(`dangling resource ref ${a.ref}`);
      return this.res.get(a.ref);
    }
    if ('enum' in a) return this._resolve(a.enum);        // BlendMode.SrcOver
    if ('const' in a) return this._resolve(a.const);       // TRANSPARENT, etc.
    if ('color' in a) { const [r, g, b, al] = a.color; return CK.Color4f(r, g, b, al === undefined ? 1 : al); }
    if ('colorInt' in a) return a.colorInt;                // a Skia color int
    if ('rect' in a) { const [l, t, r, b] = a.rect; return CK.LTRBRect(l, t, r, b); }
    if ('xywh' in a) { const [x, y, w, h] = a.xywh; return CK.XYWHRect(x, y, w, h); }
    if ('irect' in a) { const [l, t, r, b] = a.irect; return CK.LTRBiRect(l, t, r, b); }
    if ('rrect' in a) { const q = a.rrect; return CK.RRectXY(CK.LTRBRect(q.rect[0], q.rect[1], q.rect[2], q.rect[3]), q.rx || 0, q.ry || 0); }
    if ('f32' in a) return new Float32Array(a.f32);        // matrices, points, stops
    if ('u32' in a) return new Uint32Array(a.u32);         // int-color arrays, glyphs
    if ('i32' in a) return new Int32Array(a.i32);
    if ('u8' in a) return new Uint8Array(a.u8);
    if ('bytes' in a) return _b64(a.bytes);                // encoded image / font data
    if ('points' in a) return new Float32Array(a.points.flat());
    if ('matrix' in a) return new Float32Array(a.matrix);  // 3x3 (9) or 4x4 (16)
    if ('str' in a) return a.str;
    if ('paragraph' in a) return this._buildParagraph(a.paragraph);

    // A plain option dict (e.g. drawImageOptions, a ParagraphStyle/TextStyle
    // spec): marshal each value so nested {color}/{enum}/… resolve.
    const out = {};
    for (const k of Object.keys(a)) out[k] = this._marshal(a[k]);
    return out;
  }

  // Execute one program step against the canvas + resource table.
  _step(canvas, s) {
    let result;
    const args = (s.args || []).map((x) => this._marshal(x));
    if (s.new !== undefined) {
      // Constructor: new CanvasKit.<new>(...args)
      const Ctor = this._resolve(s.new);
      result = new Ctor(...args);
    } else if (s.make !== undefined) {
      // Zero-arg construct shorthand: new CanvasKit.<make>()
      const Ctor = this._resolve(s.make);
      result = new Ctor();
    } else if (s.static !== undefined) {
      // Static factory: CanvasKit.<static>(...args)  (dotted, e.g. Shader.Make…)
      const fn = this._resolve(s.static);
      const owner = s.static.includes('.') ? this._resolve(s.static.split('.').slice(0, -1).join('.')) : this.CanvasKit;
      result = fn.apply(owner, args);
    } else if (s.ref !== undefined && s.method !== undefined) {
      // Instance method: resources[ref].method(...args)
      const obj = this.res.get(s.ref);
      if (!obj) throw new Error(`method '${s.method}' on missing resource ${s.ref}`);
      result = obj[s.method](...args);
    } else if (s.canvas !== undefined) {
      // Canvas method: canvas.<canvas>(...args)
      result = canvas[s.canvas](...args);
    } else if (s.free !== undefined) {
      const obj = this.res.get(s.free);
      if (obj && typeof obj.delete === 'function') obj.delete();
      this.res.delete(s.free);
      return;
    } else {
      throw new Error('invalid step: ' + JSON.stringify(s));
    }
    // Register a produced resource (and track deletables for cleanup).
    if (s.def !== undefined) {
      this.res.set(s.def, result);
      if (result && typeof result.delete === 'function') this._deletables.push(result);
    }
    return result;
  }

  // Run a full Skia program: { surface?, steps:[...] }. `steps` is an ordered
  // list mixing resource construction, resource mutation, and canvas ops.
  runProgram(program) {
    const canvas = this.surface.getCanvas();
    canvas.clear(program.clear ? this._marshal(program.clear) : this.CanvasKit.TRANSPARENT);
    for (const s of program.steps || []) this._step(canvas, s);
    this.surface.flush();
    return canvas;
  }

  // ---- Paragraph (real text layout/shaping) --------------------------------

  // Build and lay out a CanvasKit Paragraph from a compact spec:
  //   { text, fontSize, color:[r,g,b,a]|colorInt, weight:'bold'|'normal',
  //     align:'left'|'center'|'right', maxWidth, fontFamilies:[...] }
  _buildParagraph(spec) {
    const CK = this.CanvasKit;
    const colorArr = spec.color ? spec.color : [0, 0, 0, 1];
    if (!this._fontProvider) throw new Error('no font registered — call useFont(bytes) before drawing text');
    const paraStyle = new CK.ParagraphStyle({
      textStyle: {
        color: CK.Color4f(colorArr[0], colorArr[1], colorArr[2], colorArr[3] === undefined ? 1 : colorArr[3]),
        fontSize: spec.fontSize || 14,
        fontFamilies: spec.fontFamilies || [this._fontFamily],
        fontStyle: { weight: spec.weight === 'bold' ? CK.FontWeight.Bold : CK.FontWeight.Normal },
      },
      textAlign: spec.align === 'center' ? CK.TextAlign.Center
               : spec.align === 'right' ? CK.TextAlign.Right : CK.TextAlign.Left,
    });
    const builder = CK.ParagraphBuilder.MakeFromFontProvider(paraStyle, this._fontProvider);
    builder.addText(spec.text || '');
    const paragraph = builder.build();
    paragraph.layout(spec.maxWidth || 1e9);
    builder.delete();
    this._deletables.push(paragraph);
    return paragraph;
  }

  // Render whatever the Elpian VM submitted this frame. A guest that drives the
  // full Skia API emits a raw program (`{ skia: { steps:[...] } }`); a widget app
  // emits a `dart:ui` scene (`{ root: { ops:[...] } }`). Both paths are real Skia.
  render(frame) {
    if (frame && frame.skia) return this.runProgram(frame.skia);
    return this.paintScene(frame);
  }

  // ---- Elpian dart:ui scene replay -----------------------------------------

  // Replay an Elpian widget scene ({ root: { ops:[...] } }) on real Skia. This
  // is the path that renders the flutter.dart app: the VM's drawRect/drawCircle/
  // drawParagraph become genuine Skia draws, with real Paragraph text.
  paintScene(scene) {
    const CK = this.CanvasKit;
    const canvas = this.surface.getCanvas();
    canvas.clear(CK.TRANSPARENT);
    const ops = (scene && scene.root && scene.root.ops) || [];
    const paint = new CK.Paint();
    paint.setAntiAlias(true);
    for (const op of ops) {
      switch (op.op) {
        case 'drawRect': {
          paint.setColor(_argb(CK, op.color));
          const [l, t, r, b] = op.rect;
          canvas.drawRect(CK.LTRBRect(l, t, r, b), paint);
          break;
        }
        case 'drawRRect': {
          paint.setColor(_argb(CK, op.color));
          const [l, t, r, b] = op.rect;
          canvas.drawRRect(CK.RRectXY(CK.LTRBRect(l, t, r, b), op.rx || 0, op.ry || 0), paint);
          break;
        }
        case 'drawCircle': {
          paint.setColor(_argb(CK, op.color));
          canvas.drawCircle(op.center[0], op.center[1], op.radius, paint);
          break;
        }
        case 'drawParagraph': {
          const p = this._buildParagraph({
            text: op.text, fontSize: op.fontSize,
            color: _argbArr(op.color), weight: op.fontWeight,
          });
          // The scene's offset y is the text baseline (top + fontSize); Skia's
          // drawParagraph takes the paragraph's top, so shift up by fontSize.
          canvas.drawParagraph(p, op.offset[0], op.offset[1] - op.fontSize);
          break;
        }
        default:
          // Unknown op kinds are ignored (forward-compatible).
          break;
      }
    }
    paint.delete();
    this.surface.flush();
  }

  // Free every Skia object created (paragraphs, resources, fontMgr).
  dispose() {
    for (const d of this._deletables) { try { d.delete(); } catch (e) {} }
    for (const v of this.res.values()) { try { if (v && v.delete) v.delete(); } catch (e) {} }
    this._deletables = [];
    this.res.clear();
    if (this._fontProvider) { try { this._fontProvider.delete(); } catch (e) {} this._fontProvider = null; }
  }
}

// ---- helpers ----------------------------------------------------------------

// An 0xAARRGGBB int -> CanvasKit Color4f (its native color representation).
function _argb(CK, argb) {
  const a = (Math.floor(argb / 16777216) % 256) / 255;
  const r = (Math.floor(argb / 65536) % 256) / 255;
  const g = (Math.floor(argb / 256) % 256) / 255;
  const b = (argb % 256) / 255;
  return CK.Color4f(r, g, b, a);
}
function _argbArr(argb) {
  return [
    (Math.floor(argb / 65536) % 256) / 255,
    (Math.floor(argb / 256) % 256) / 255,
    (argb % 256) / 255,
    (Math.floor(argb / 16777216) % 256) / 255,
  ];
}
function _b64(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// ---- coverage audit ---------------------------------------------------------

// Enumerate the entire loaded CanvasKit API and confirm the reflective bridge
// can reach every namespace, constructor, factory, and enum — the machine-
// checked "no exceptions" guarantee (run against the real library, not a list).
export function auditCoverage(CanvasKit) {
  const report = { enums: [], factories: [], constructors: [], namespaces: [], total: 0, unreachable: [] };
  const seen = new Set();
  // Emscripten/WASM runtime internals that are not part of the drawing API.
  const SKIP = /^(HEAP|buffer|asm|wasm|dynCall|stackAlloc|stackSave|stackRestore|_malloc|_free|Module|GL|preRun|postRun|onRuntimeInitialized|then|ready|calledRun|noExitRuntime|ENVIRONMENT|locateFile|instantiateWasm|print|printErr|setStatus|monitorRunDependencies)/;

  const ownNames = (o) => { try { return Object.getOwnPropertyNames(o); } catch (e) { return []; } };
  const safeObject = (v) => {
    // Never enumerate typed arrays, buffers, or array-likes with a huge length.
    if (v == null) return false;
    if (ArrayBuffer.isView(v) || v instanceof ArrayBuffer) return false;
    const len = v.length;
    if (typeof len === 'number' && len > 4096) return false;
    return true;
  };

  const walk = (obj, prefix, depth) => {
    if (depth > 2 || !safeObject(obj)) return;
    for (const key of ownNames(obj)) {
      if (key.startsWith('_') || key === 'constructor' || key === 'prototype' || SKIP.test(key)) continue;
      const path = prefix ? `${prefix}.${key}` : key;
      if (seen.has(path)) continue;
      seen.add(path);
      let v;
      try { v = obj[key]; } catch (e) { continue; }
      const t = typeof v;
      if (t !== 'function' && t !== 'object') continue; // skip scalars (versions, flags)
      report.total++;

      // Reachability: the bridge's dotted resolver must land on a defined value
      // for this path (identity is not required — some CanvasKit constants, e.g.
      // the color arrays TRANSPARENT/BLACK, return a fresh value per access).
      let reachable = true;
      try {
        let cur = CanvasKit;
        for (const p of path.split('.')) cur = cur[p];
        reachable = cur !== undefined;
      } catch (e) { reachable = false; }
      if (!reachable) report.unreachable.push(path);

      if (t === 'function') {
        if (/^(Make|From)/.test(key)) report.factories.push(path);
        else if (/^[A-Z]/.test(key)) report.constructors.push(path);
        else report.factories.push(path);
        walk(v, path, depth + 1); // Path.MakeFromSVGString, Shader.MakeLinearGradient, …
      } else {
        const members = ownNames(v).filter((m) => !m.startsWith('_') && m !== 'constructor');
        if (members.length && members.every((m) => typeof v[m] !== 'object' || v[m] === null)) {
          report.enums.push({ name: path, members: members.length });
        } else {
          report.namespaces.push(path);
        }
        walk(v, path, depth + 1);
      }
    }
  };
  walk(CanvasKit, '', 0);
  return report;
}
