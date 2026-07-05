// An interactive miniapp: a tappable button drives a counter, rendered live.
// Pointer events come from real browser clicks (via Playwright), run the VM's
// onPointerEvent handler, mutate state, and the next frame reflects it — the
// full event -> VM -> scene -> pixels loop, interactively.

var count = 0;

void onPointerEvent(e) {
  var x = e["x"];
  var y = e["y"];
  // Hit-test the button rect (20,20)-(160,90).
  if (x >= 20 && x <= 160 && y >= 20 && y <= 90) {
    count = count + 1;
  }
}

void onDrawFrame() {
  askHost("dart:ui/PictureRecorder.beginRecording", []);
  // Button.
  askHost("dart:ui/Canvas.drawRect", [20, 20, 160, 90, 4278233600]);            // green
  askHost("dart:ui/Canvas.drawParagraph", ["TAP +1", 52, 62, 22, 4294967295]);  // white
  // A progress bar whose width tracks the count.
  askHost("dart:ui/Canvas.drawRect", [20, 130, 20 + count * 30, 170, 4278190335]); // blue
  askHost("dart:ui/Canvas.drawParagraph", ["count: " + count, 20, 210, 20, 4294967295]);
  var pic = askHost("dart:ui/PictureRecorder.endRecording", []);
  var scene = askHost("dart:ui/Picture.toScene", [pic]);
  askHost("dart:ui/FlutterView.render", [scene]);
}
