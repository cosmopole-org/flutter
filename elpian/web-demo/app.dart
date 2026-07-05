// A Dart-subset "miniapp" delivered dynamically and run by the Elpian VM
// (compiled to wasm) inside a headless browser. It exercises classes, methods
// with arrow bodies, list indexing, and async/await, then paints a scene
// through the dart:ui bridge which the page rasterizes to a canvas.

class Palette {
  int red() => 4294901760;    // 0xFFFF0000
  int green() => 4278255360;  // 0xFF00FF00
  int blue() => 4278190335;   // 0xFF0000FF
}

Future<int> pick(int c) async {
  return c;
}

void main() {
  var p = Palette();
  var xs = [20, 130, 240];
  var colors = [p.red(), p.green(), p.blue()];

  askHost("dart:ui/PictureRecorder.beginRecording", []);

  // A row of three swatches.
  for (var k = 0; k < 3; k = k + 1) {
    var left = xs[k];
    askHost("dart:ui/Canvas.drawRect", [left, 20, left + 90, 110, colors[k]]);
  }

  // A circle whose colour arrives through an awaited Future, and a label,
  // then submit the frame. This runs on a microtask after main() returns, so
  // it also proves the async event loop drives to a rendered result.
  pick(4278190335).then((c) {
    askHost("dart:ui/Canvas.drawCircle", [175, 200, 55, c]);
    askHost("dart:ui/Canvas.drawParagraph", ["Elpian VM in the browser", 20, 300, 26, 4294901760]);
    var pic = askHost("dart:ui/PictureRecorder.endRecording", []);
    var scene = askHost("dart:ui/Picture.toScene", [pic]);
    askHost("dart:ui/FlutterView.render", [scene]);
  });
}
