import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

// Zucchero's row 220: a phone its Claktory turned away (a pairing code
// that expired) said "Connecting…" forever in the main view, while the
// drawer said Not allowed.

void main() {
  testWidgets('turned away, the main view says so, with Switch Claktory…, '
      'never Connecting…', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(375, 667));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    harness.backend.refused.value =
        "This device isn't allowed on this Claktory. Use a new code.";
    await pumpFrames(tester);
    expect(find.text('Connecting…'), findsNothing);
    expect(
      find.text("This device isn't allowed on this Claktory. Use a new code."),
      findsOneWidget,
    );
    expect(find.textContaining("doesn't allow this device"), findsWidgets);
    await tester.tap(find.text('Switch Claktory…'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);

    // Picked again and let in: the usual view.
    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(
      find.text("This device isn't allowed on this Claktory. Use a new code."),
      findsNothing,
    );
  });
}
