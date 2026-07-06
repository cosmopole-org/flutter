// skia_guest.dart — a guest that drives the FULL Skia API directly from Elpian
// bytecode. There is no widget framework here: the program builds a reflective
// "Skia program" (a list of steps that construct paints/shaders/paths/filters
// and issue canvas ops by name) and submits it over the dart:ui host seam. The
// browser bridge replays it onto real CanvasKit/Skia. Taps mutate state and the
// next frame re-emits — so guest bytecode is genuinely controlling Skia, live.

var taps = 0;

void onPointerEvent(e) {
  taps = taps + 1;
}

void onDrawFrame() {
  var steps = [];

  // 1) A gradient-filled rounded rectangle (real Skia LinearGradient shader).
  steps.add({"def": 1, "make": "Paint"});
  steps.add({"ref": 1, "method": "setAntiAlias", "args": [true]});
  steps.add({"def": 2, "static": "Shader.MakeLinearGradient", "args": [
    [0, 0], [360, 0],
    [{"color": [0.13, 0.83, 0.98, 1.0]}, {"color": [0.61, 0.35, 0.98, 1.0]}],
    [0.0, 1.0], {"enum": "TileMode.Clamp"}
  ]});
  steps.add({"ref": 1, "method": "setShader", "args": [{"ref": 2}]});
  steps.add({"canvas": "drawRRect", "args": [
    {"rrect": {"rect": [20, 20, 380, 130], "rx": 18.0, "ry": 18.0}}, {"ref": 1}
  ]});

  // 2) A stroked cubic Bézier path with a Gaussian blur mask filter.
  steps.add({"def": 3, "make": "Paint"});
  steps.add({"ref": 3, "method": "setAntiAlias", "args": [true]});
  steps.add({"ref": 3, "method": "setStyle", "args": [{"enum": "PaintStyle.Stroke"}]});
  steps.add({"ref": 3, "method": "setStrokeWidth", "args": [6.0]});
  steps.add({"ref": 3, "method": "setColor", "args": [{"color": [1.0, 0.42, 0.21, 1.0]}]});
  steps.add({"def": 4, "static": "MaskFilter.MakeBlur", "args": [{"enum": "BlurStyle.Normal"}, 2.0, true]});
  steps.add({"ref": 3, "method": "setMaskFilter", "args": [{"ref": 4}]});
  steps.add({"def": 5, "make": "Path"});
  steps.add({"ref": 5, "method": "moveTo", "args": [30, 200]});
  steps.add({"ref": 5, "method": "cubicTo", "args": [130, 150, 250, 260, 370, 200]});
  steps.add({"canvas": "drawPath", "args": [{"ref": 5}, {"ref": 3}]});

  // 3) A filled circle whose radius tracks the tap count (interactive).
  steps.add({"def": 6, "make": "Paint"});
  steps.add({"ref": 6, "method": "setAntiAlias", "args": [true]});
  steps.add({"ref": 6, "method": "setColor", "args": [{"color": [0.30, 0.69, 0.31, 0.95]}]});
  steps.add({"canvas": "drawCircle", "args": [90, 262, 14.0 + taps * 6.0, {"ref": 6}]});

  // 4) Real shaped text via the Paragraph API.
  steps.add({"canvas": "drawParagraph", "args": [
    {"paragraph": {
      "text": "Elpian VM -> raw Skia ops (taps: " + taps + ")",
      "fontSize": 18.0, "color": [0.9, 0.94, 1.0, 1.0], "weight": "bold", "maxWidth": 360.0
    }}, 20, 300
  ]});

  askHost("dart:ui/FlutterView.render", [{"skia": {"steps": steps}}]);
}
