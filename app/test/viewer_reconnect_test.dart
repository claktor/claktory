// A viewer opened before the window's Claktory is connected (an iPad or
// iPhone opened cold over the tunnel, a Mac starting up) reads its file
// again once it connects, saying meanwhile that it waits for it;
// and only on connecting, not on every word from the connection.
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, reads, snapshot, text;

void main() {
  testWidgets('a viewer that could not read its file before the backend '
      'connected reads it once it connects', (tester) async {
    final harness = Harness(snapshot(layout: beside()));
    harness.backend.files['/w/main.rs'] = text('/w/main.rs', 'fn main() {}\n');
    harness.backend.connected.value = false;
    harness.backend.refuse = "Can't reach your Claktory right now.";
    await harness.show(tester);
    await settle(tester);
    // It waits for the connection, rather than saying it failed.
    expect(find.text("Can't reach your Claktory right now."), findsNothing);
    expect(
      find.text(
        "Waiting for the connection. It carries on from here when it's back.",
      ),
      findsOneWidget,
    );
    expect(find.text('fn main() {}'), findsNothing);

    harness.backend.refuse = null;
    harness.backend.connected.value = true;
    await settle(tester);
    expect(find.text('fn main() {}'), findsOneWidget);
    expect(find.text("Can't reach your Claktory right now."), findsNothing);
    final after = reads(harness);

    // Still connected: nothing to read again.
    harness.backend.connected.value = true;
    await settle(tester);
    expect(reads(harness), after);
  });

  testWidgets('a viewer that read its file does not read it again when the '
      'connection comes back', (tester) async {
    final harness = Harness(snapshot(layout: beside()));
    harness.backend.files['/w/main.rs'] = text('/w/main.rs', 'fn main() {}\n');
    harness.backend.connected.value = true;
    await harness.show(tester);
    await settle(tester);
    expect(find.text('fn main() {}'), findsOneWidget);
    final before = reads(harness);
    harness.backend.connected.value = false;
    await settle(tester);
    harness.backend.connected.value = true;
    await settle(tester);
    expect(reads(harness), before);
  });
}
