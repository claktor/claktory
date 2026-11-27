// Android: a new version of this app. The phone's drawer and
// Settings → General say so while a newer build that works with the
// Claktory is out, with Download, which opens its download page in the
// browser; a page that doesn't open says so, and Download tries again.
import 'dart:convert';

import 'package:factory_app/android/android_app_updates.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/update.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

String _manifest({int code = 101900}) => jsonEncode({
  'version': 'v0.1.19',
  'version_code': code,
  'min_server': 'v0.1.15',
  'page': 'https://claktory.com/android',
  'apk': 'https://claktory.com/media/example/claktory-0.1.19-101900.apk',
  'size': 89755471,
  'sha256': 'a' * 64,
  'package': 'com.claktory.app',
  'cert_sha256': 'b' * 64,
});

const _line = 'This app is v0.1.18. A new version is available.';

void _words() => test('the words name the app\'s own version when stamped', () {
  expect(
    androidUpdateText('v0.1.18'),
    'This app is v0.1.18. A new version is available.',
  );
  expect(androidUpdateText('dev'), 'A new version of this app is available.');
});

Finder _download(String label) => find.byWidgetPredicate(
  (widget) => widget is Button && widget.label == label,
);

void main() {
  _words();
  setUp(() => PhonePlatformPolicy.debugAndroidPolicy = true);
  tearDown(() {
    PhonePlatformPolicy.debugAndroidPolicy = false;
    AndroidAppUpdateHost.debugMake = null;
    AndroidUpdateNotice.debugOpen = null;
  });

  Future<Harness> show(
    WidgetTester tester, {
    int installed = 101800,
    String backend = 'v0.1.19',
  }) async {
    AndroidAppUpdateHost.debugMake = () => AndroidAppUpdates(
      enabled: true,
      loadManifest: () async => _manifest(),
      readVersionCode: () async => installed,
    );
    final harness = Harness(workspace()..version = backend)
      ..appVersion = 'v0.1.18';
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('the drawer says so, and Download opens the download page', (
    tester,
  ) async {
    final opened = <Uri>[];
    var answer = false;
    AndroidUpdateNotice.debugOpen = (uri) async {
      opened.add(uri);
      return answer;
    };
    await show(tester);
    await tester.tap(find.bySemanticsLabel('Menu').first);
    await pumpFrames(tester);
    expect(find.text(_line), findsOneWidget);

    // The browser doesn't open it: said, and Download tries again.
    await tester.tap(_download('Download'));
    await pumpFrames(tester);
    expect(opened, [Uri.parse('https://claktory.com/android')]);
    expect(
      find.text("Couldn't open the download page. Try again."),
      findsOneWidget,
    );
    answer = true;
    await tester.tap(_download('Download'));
    await pumpFrames(tester);
    expect(opened, hasLength(2));
    expect(find.textContaining("Couldn't open"), findsNothing);
  });

  testWidgets('Settings → General says so under the version', (tester) async {
    final harness = await show(tester);
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    await tester.tap(find.text('General'));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text(_line));
    await pumpFrames(tester);
    expect(find.text(_line), findsOneWidget);
    expect(_download('Download'), findsOneWidget);
  });

  testWidgets('nothing when this build is the newest, or the new one '
      "wouldn't work with this Claktory", (tester) async {
    await show(tester, installed: 101900);
    await tester.tap(find.bySemanticsLabel('Menu').first);
    await pumpFrames(tester);
    expect(find.text(_line), findsNothing);
  });

  testWidgets("nothing for a Claktory older than the new build's floor", (
    tester,
  ) async {
    await show(tester, backend: 'v0.1.14');
    await tester.tap(find.bySemanticsLabel('Menu').first);
    await pumpFrames(tester);
    expect(find.text(_line), findsNothing);
  });
}
