// Settings → General's Version row: Check for updates and what it found
// (Workspace.update_check), with Update when a release is ready; on a Mac
// it's the banner's Update, on a phone it updates the Claktory only.

import 'package:factory_app/android/android_app_updates.dart'
    show androidAppInstallPage;
import 'package:factory_app/controls.dart' show Button, GlyphButton;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/update.dart';
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

  group('an update that would leave a device behind (#355 D1)', () {
    // v0.2.0 starts a new line: apps on v0.1.x can't connect to it.
    Workspace offer({List<PairedDevice> devices = const []}) => _workspace(
      UpdateCheckState.UPDATE_CHECK_STATE_READY,
      version: 'v0.2.0',
      latest: 'v0.2.0',
    )..remote = RemoteAccess(devices: devices);
    PairedDevice paired(String id, String name, {bool behind = false}) =>
        PairedDevice(
          id: id,
          name: name,
          appVersion: 'v0.1.19',
          updateLeavesBehind: behind,
        );

    test('the devices it names, in words', () {
      final workspace = offer(
        devices: [
          paired('me', "Ana's iPhone", behind: true),
          paired('d2', 'Pixel', behind: true),
          paired('d3', 'Studio'),
          paired('d4', "Ana's iPad", behind: true),
        ],
      );
      // This device isn't named: it's told what to do instead.
      expect(
        leftBehindText(leftBehind(workspace, 'me')),
        "Pixel and Ana's iPad won't connect after this update until their "
        'apps are updated.',
      );
      expect(
        leftBehindText(leftBehind(workspace, 'elsewhere')),
        "Ana's iPhone, Pixel and Ana's iPad won't connect after this update "
        'until their apps are updated.',
      );
      expect(
        leftBehindText(
          leftBehind(
            offer(devices: workspace.remote.devices.take(2).toList()),
            'me',
          ),
        ),
        "Pixel won't connect after this update until its app is updated.",
      );
      expect(leftBehindText(leftBehind(offer(), 'me')), isNull);
    });

    test("whether this device can't follow it", () {
      bool cant(
        Workspace workspace, {
        String app = 'v0.1.19',
        bool replaces = false,
      }) => cantFollowUpdate(
        workspace,
        thisDevice: 'me',
        appVersion: app,
        replacesApp: replaces,
      );
      // Its own row says so.
      expect(
        cant(offer(devices: [paired('me', 'Phone', behind: true)]), app: 'dev'),
        isTrue,
      );
      // An older backend that doesn't say: its own build's line.
      expect(cant(offer()), isTrue);
      expect(cant(offer(), app: 'v0.2.0'), isFalse);
      expect(cant(offer(), app: 'dev'), isFalse);
      // A Mac's app is replaced along with it.
      expect(cant(offer(), replaces: true), isFalse);
      // Nothing of the backend's on offer.
      expect(cant(offer()..latestBackend = 'v0.1.16'), isFalse);
    });

    testWidgets("on a phone that can't follow it: no Update, and its app "
        'first', (tester) async {
      final harness = Harness(
        offer(devices: [paired('me', "Ana's iPhone", behind: true)]),
      )..appVersion = 'v0.1.19';
      harness.backend.deviceId = 'me';
      await harness.show(tester, size: const Size(393, 852));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      await tester.tap(find.text('General'));
      await pumpFrames(tester);
      final first = find.textContaining(
        "app first, from the App Store or "
        'TestFlight. Then Studio can update to Claktory v0.2.0.',
      );
      await tester.ensureVisible(first);
      await pumpFrames(tester);
      expect(first, findsOneWidget);
      expect(_button('Update'), findsNothing);
      expect(find.textContaining("won't connect"), findsNothing);
    });

    testWidgets("on an Android phone that can't follow it: no Update, and "
        'Download for its app, not a store (Sale\'s row 60)', (tester) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() {
        PhonePlatformPolicy.debugAndroidPolicy = false;
        UpdateCheckRow.debugOpen = null;
      });
      final opened = <Uri>[];
      var answer = false;
      UpdateCheckRow.debugOpen = (uri) async {
        opened.add(uri);
        return answer;
      };
      final harness = Harness(
        offer(devices: [paired('me', 'Pixel', behind: true)]),
      )..appVersion = 'v0.1.19';
      harness.backend.deviceId = 'me';
      await harness.show(tester, size: const Size(412, 915));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      await tester.tap(find.text('General'));
      await pumpFrames(tester);
      final first = find.textContaining(
        "app first: tap Download for its new version. Then Studio can "
        'update to Claktory v0.2.0.',
      );
      await tester.ensureVisible(first);
      await pumpFrames(tester);
      expect(first, findsOneWidget);
      expect(find.textContaining('App Store'), findsNothing);
      expect(_button('Update'), findsNothing);
      // Download opens the app's download page; one that doesn't open
      // says so, and Download tries again.
      await tester.tap(_button('Download'));
      await pumpFrames(tester);
      expect(opened, [androidAppInstallPage]);
      expect(
        find.text("Couldn't open the download page. Try again."),
        findsOneWidget,
      );
      answer = true;
      await tester.tap(_button('Download'));
      await pumpFrames(tester);
      expect(opened, hasLength(2));
      expect(
        find.text("Couldn't open the download page. Try again."),
        findsNothing,
      );
    });

    testWidgets('on a phone that can, the others are named above Update', (
      tester,
    ) async {
      final harness = Harness(
        offer(
          devices: [
            paired('me', "Ana's iPhone"),
            paired('d2', 'Pixel', behind: true),
          ],
        ),
        // A phone on the new line already can't reach the old Claktory at
        // all: only a build that isn't numbered follows either.
      )..appVersion = 'dev';
      harness.backend.deviceId = 'me';
      await harness.show(tester, size: const Size(393, 852));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      await tester.tap(find.text('General'));
      await pumpFrames(tester);
      final behind = find.text(
        "Pixel won't connect after this update until its app is updated.",
      );
      await tester.ensureVisible(behind);
      await pumpFrames(tester);
      expect(
        tester.getRect(behind).bottom,
        lessThanOrEqualTo(tester.getRect(_button('Update')).top),
      );
      await tester.tap(_button('Update'));
      await pumpFrames(tester);
      expect(harness.last.hasInstallUpdate(), isTrue);
    });

    testWidgets('on a Mac, the banner and the Version row name them', (
      tester,
    ) async {
      await _mac(
        tester,
        offer(devices: [paired('d1', "Ana's iPhone", behind: true)]),
      );
      expect(
        find.text(
          "Ana's iPhone won't connect after this update until its app is "
          'updated.',
        ),
        findsNWidgets(2),
      );
      // The Mac's own app is replaced with it: Update is offered.
      expect(_button('Update'), findsWidgets);
    });
  });

  group("an iPad whose app is behind (Matt's iPad, Bruno 45175)", () {
    Future<Harness> iPad(
      WidgetTester tester, {
      required String claktory,
    }) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() {
        debugDefaultTargetPlatformOverride = null;
        resetAppBehindNote();
      });
      final harness = Harness(
        _workspace(
          UpdateCheckState.UPDATE_CHECK_STATE_UP_TO_DATE,
          version: 'v0.1.23',
          latest: 'v0.1.23',
        )..version = claktory,
      )..appVersion = 'v0.1.22';
      await harness.show(tester, size: const Size(1180, 820));
      await pumpFrames(tester);
      return harness;
    }

    Finder close() => find.byWidgetPredicate(
      (widget) => widget is GlyphButton && widget.label == 'Close',
    );

    testWidgets('with its Claktory up to date: no Update, a note that its app '
        'comes from the App Store or TestFlight, which Close puts away', (
      tester,
    ) async {
      final harness = await iPad(tester, claktory: 'v0.1.23');
      expect(find.text('Claktory v0.1.23 is ready.'), findsNothing);
      expect(_button('Update'), findsNothing);
      final note = find.text(
        'A newer Claktory app (v0.1.23) is in the App Store or TestFlight. '
        'Your Claktory is already up to date.',
      );
      expect(note, findsOneWidget);
      await tester.tap(close());
      await pumpFrames(tester);
      expect(note, findsNothing);
      expect(harness.backend.sent.where((m) => m.hasInstallUpdate()), isEmpty);
    });

    testWidgets('with its Claktory behind too: Update updates only the '
        'Claktory', (tester) async {
      final harness = await iPad(tester, claktory: 'v0.1.22');
      expect(find.textContaining('A newer Claktory app'), findsNothing);
      final update = find.descendant(
        of: find.byType(UpdateBanner),
        matching: _button('Update'),
      );
      expect(update, findsOneWidget);
      await tester.tap(update);
      await pumpFrames(tester);
      expect(harness.last.installUpdate.appPath, isEmpty);
      expect(harness.quits, 0, reason: "its app isn't Claktory's to replace");
    });
  });
}
