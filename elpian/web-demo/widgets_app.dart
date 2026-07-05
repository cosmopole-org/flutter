// A REAL Flutter-style widget app — the exact idiom you'd write in Flutter —
// delivered dynamically and run by the Elpian VM (compiled to wasm) in a
// headless browser. No raw dart:ui calls here: the app is authored entirely as
// widgets (StatelessWidget/StatefulWidget, build(), nested children, a
// GestureDetector). The widget framework (prepended by elpian_init_widgets)
// builds, lays out, and paints it into the dart:ui scene the page rasterizes.
//
// A tap on the "+1" button re-enters the VM, runs onTap -> setState, and the
// next frame reflects the incremented count and a progress bar that tracks it.

class CounterApp extends StatelessWidget {
  Widget build() {
    return Counter();
  }
}

class Counter extends StatefulWidget {
  State createState() { return CounterState(); }
}

class CounterState extends State {
  int count = 0;

  Widget build() {
    return Column(
      crossAxisAlignment: "start",
      children: [
        // The tappable button.
        GestureDetector(
          onTap: () { setState(() { count = count + 1; }); },
          child: Container(
            width: 140.0,
            height: 60.0,
            color: 4278233600, // 0xFF00A000 green
            child: Center(child: Text("+1", size: 26.0, color: 4294967295)),
          ),
        ),
        // A progress bar whose width tracks the count.
        Container(
          width: 20.0 + count * 30.0,
          height: 30.0,
          color: 4278190335, // 0xFF0000FF blue
        ),
        // A live label.
        Text("count: $count", size: 22.0, color: 4294967295),
      ],
    );
  }
}

void main() {
  runApp(CounterApp());
}
