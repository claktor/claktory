// The Android app never offers Claktory's updates (Bruno's minimum friend
// build, item 3): its new version is a new APK from its link, so there's no
// Update banner and nothing reaches the installer.
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/update.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show Size;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({required String version, required String latest}) =>
    Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
      settings: Settings(userName: 'Matt', name: 'mac-mini'),
      version: version,
      latestRelease: latest,
      latestBackend: latest,
    );

/// The Android app: Android, with Android's own behavior.
void _android() {
  PhonePlatformPolicy.debugAndroidPolicy = true;
  addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
}

Future<Harness> _show(
  WidgetTester tester,
  Workspace snapshot, {
  required String app,
}) async {
  final harness = Harness(snapshot)..appVersion = app;
  // A tablet's width, where a Mac shows the banner too.
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('the Android app shows no Update banner', (tester) async {
    _android();
    final snapshot = _workspace(version: 'v0.1.11', latest: 'v0.1.12');
    final harness = await _show(tester, snapshot, app: 'v0.1.11');
    expect(updatesHere, isFalse);
    expect(updateOffered(snapshot, 'v0.1.11'), isFalse);
    expect(find.text('Update'), findsNothing);
    expect(harness.backend.sent.where((m) => m.hasInstallUpdate()), isEmpty);
  });

  testWidgets('a Mac with the same release waiting shows it', (tester) async {
    final snapshot = _workspace(version: 'v0.1.11', latest: 'v0.1.12');
    await _show(tester, snapshot, app: 'v0.1.11');
    expect(updatesHere, isTrue);
    expect(find.text('Update'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("on Android, a Claktory too old for the app says where to "
      "update it, not to press an Update that isn't there", (tester) async {
    _android();
    await _show(
      tester,
      _workspace(version: 'v0.1.11', latest: 'v0.2.0'),
      app: 'v0.2.0',
    );
    expect(find.text('Update Claktory to use this app'), findsOneWidget);
    expect(
      find.text('Update Claktory on mac-mini, then come back.'),
      findsOneWidget,
    );
    expect(find.textContaining('Update above'), findsNothing);
    expect(defaultTargetPlatform, TargetPlatform.android);
  });
}
