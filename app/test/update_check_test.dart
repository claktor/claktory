// Settings → General's Version row: Check for updates and what it found
// (Workspace.update_check), with Update when a release is ready; on a Mac
// it's the banner's Update, on a phone it updates the Claktory only.

import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/update_check.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

Workspace _workspace(
  UpdateCheckState state, {
  String version = '',
  String error = '',
  String latest = '',
}) => workspace()
  ..version = 'v0.1.16'
  ..latestRelease = latest
  ..settings.name = 'Studio'
  ..updateCheck = UpdateCheck(state: state, version: version, error: error);

/// Settings on a Mac, the Version row in view.
Future<Harness> _mac(WidgetTester tester, Workspace snapshot) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(snapshot)..appVersion = 'v0.1.16';
  harness.bundlePath = '/Applications/Factory.app';
  await harness.show(tester, size: const Size(1200, 1600));
  harness.page.value = AppPage.settings;
  await settle(tester);
  await reveal(tester, find.text('Check for updates'));
  return harness;
}

Finder _button(String label) => find.byWidgetPredicate(
  (widget) => widget is Button && widget.label == label,
);

void main() {
  testWidgets('before any check, the version and the button; a press asks '
      'the Claktory', (tester) async {
    final harness = await _mac(
      tester,
      _workspace(UpdateCheckState.UPDATE_CHECK_STATE_NONE),
    );
    expect(find.text('Version'), findsOneWidget);
    expect(find.text('v0.1.16'), findsOneWidget);
    expect(find.textContaining('up to date'), findsNothing);
    await tester.tap(find.text('Check for updates'));
    await settle(tester);
    expect(harness.last.hasCheckForUpdates(), isTrue);
  });

  testWidgets('what it found shows in place', (tester) async {
    final harness = await _mac(
      tester,
      _workspace(UpdateCheckState.UPDATE_CHECK_STATE_CHECKING),
    );
    expect(find.text('Checking for updates…'), findsOneWidget);
    // Checking already: the button waits.
    expect(
      tester.widget<Button>(_button('Check for updates')).onPressed,
      isNull,
    );

    await harness.apply(
      tester,
      _workspace(
        UpdateCheckState.UPDATE_CHECK_STATE_DOWNLOADING,
        version: 'v0.1.17',
      ),
    );
    expect(find.text('Downloading Claktory v0.1.17…'), findsOneWidget);

    await harness.apply(
      tester,
      _workspace(
        UpdateCheckState.UPDATE_CHECK_STATE_UP_TO_DATE,
        version: 'v0.1.16',
      ),
    );
    expect(find.text('Claktory is up to date.'), findsOneWidget);
    expect(_button('Update'), findsNothing);
    expect(
      tester.widget<Button>(_button('Check for updates')).onPressed,
      isNotNull,
    );

    const why =
        "Claktory couldn't reach the update server. Check your internet "
        'connection, then try again.';
    await harness.apply(
      tester,
      _workspace(UpdateCheckState.UPDATE_CHECK_STATE_FAILED, error: why),
    );
    expect(find.text(why), findsOneWidget);
  });

  testWidgets("ready, it's the banner's Update", (tester) async {
    final harness = await _mac(
      tester,
      _workspace(
        UpdateCheckState.UPDATE_CHECK_STATE_READY,
        version: 'v0.1.17',
        latest: 'v0.1.17',
      ),
    );
    final ready = find.text('Claktory v0.1.17 is ready.');
    // The banner's words, and these.
    expect(ready, findsWidgets);
    final update = find.descendant(
      of: find.byType(UpdateCheckRow),
      matching: _button('Update'),
    );
    expect(update, findsOneWidget);
    await tester.tap(update);
    await settle(tester);
    // The app is older too, so it's replaced with the backend's.
    expect(harness.last.installUpdate.appPath, '/Applications/Factory.app');
    expect(harness.quits, 1);
  });

  testWidgets('after an app-only release, an app already on it is up to '
      'date though the Claktory says ready', (tester) async {
    final snapshot = _workspace(
      UpdateCheckState.UPDATE_CHECK_STATE_READY,
      version: 'v0.1.17',
      latest: 'v0.1.17',
    )..latestBackend = 'v0.1.16';
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot)..appVersion = 'v0.1.17';
    await harness.show(tester, size: const Size(1200, 1600));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text('Check for updates'));
    expect(find.text('v0.1.17'), findsOneWidget);
    expect(find.text('Claktory is up to date.'), findsOneWidget);
    expect(_button('Update'), findsNothing);
  });

  testWidgets('on a phone it says which Claktory updates, and Update updates '
      'only it', (tester) async {
    final harness = Harness(
      _workspace(
        UpdateCheckState.UPDATE_CHECK_STATE_READY,
        version: 'v0.1.17',
        latest: 'v0.1.17',
      ),
    );
    await harness.show(tester, size: const Size(393, 852));
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    await tester.tap(find.text('General'));
    await pumpFrames(tester);
    final ready = find.text('Studio can update to Claktory v0.1.17.');
    await tester.ensureVisible(ready);
    await pumpFrames(tester);
    // The Claktory's version, which is what updates.
    expect(find.text('v0.1.16'), findsOneWidget);
    await tester.tap(_button('Update'));
    await pumpFrames(tester);
    expect(harness.last.installUpdate.appPath, isEmpty);
    expect(harness.quits, 0);
  });
}
