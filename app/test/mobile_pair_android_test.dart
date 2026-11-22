// Pairing an Android phone (the first friend build): no camera to scan
// with, so the screen opens on pasting, and its words name no iPhone or
// Mac: the code comes from a device already connected, or from where the
// Claktory was set up (a server, often). The iPhone keeps its own words.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Future<void> _show(WidgetTester tester, TargetPlatform platform) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(Workspace(), localBackend: false);
  harness.camera.state = platform == TargetPlatform.android
      ? CameraAccess.unavailable
      : CameraAccess.granted;
  await harness.show(tester, size: const Size(393, 852));
  await pumpFrames(tester);
}

void main() {
  testWidgets('on Android: straight to pasting, in words for a server', (
    tester,
  ) async {
    await _show(tester, TargetPlatform.android);
    expect(find.textContaining('Paste the code for this phone'), findsOne);
    expect(find.textContaining('iPhone'), findsNothing);
    expect(find.textContaining('Mac'), findsNothing);
    expect(find.textContaining('scan'), findsNothing);
    expect(find.byType(EditableText), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'not a code');
    await pumpFrames(tester);
    expect(
      find.text(
        "That isn't a Claktory code. Copy it again, all of it, and paste it "
        'here.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('on iPhone: its own words, scanning first', (tester) async {
    await _show(tester, TargetPlatform.iOS);
    expect(find.textContaining('On your Mac'), findsOne);
    expect(find.textContaining('Paste the code for this phone'), findsNothing);
  });
}
