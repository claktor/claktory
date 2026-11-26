import 'package:factory_app/compatibility.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:flutter/foundation.dart' show TargetPlatform;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({
  required String version,
  String latest = '',
  String latestBackend = '',
}) => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt', name: 'mac-mini'),
  version: version,
  latestRelease: latest,
  latestBackend: latestBackend,
);

void main() {
  test('only versions on one line work together', () {
    expect(versionLine('v0.1.11'), (BigInt.zero, BigInt.one));
    expect(versionLine('v0.1.12-dev.20261120-1830.05'), (
      BigInt.zero,
      BigInt.one,
    ));
    expect(versionLine('dev'), isNull);
    expect(versionLine('2026.11.19-1958'), isNull);

    // Today's normal cases: the same line, an app-only release ahead of
    // its backend, dev builds, backends from before v0.1.0.
    expect(mismatch('v0.1.11', 'v0.1.11'), isNull);
    expect(mismatch('v0.1.12', 'v0.1.11'), isNull);
    expect(mismatch('v0.1.12-dev.x', 'v0.1.11'), isNull);
    expect(mismatch('dev', 'v0.2.0'), isNull);
    expect(mismatch('v0.2.0', 'dev'), isNull);
    expect(mismatch('v0.1.11', '2026.11.19-1958'), isNull);

    // Across an --incompatible release, either way.
    expect(mismatch('v0.2.0', 'v0.1.11'), Mismatch.backendOlder);
    expect(mismatch('v0.1.11', 'v0.2.0'), Mismatch.appOlder);
    expect(mismatch('v1.0.0', 'v0.9.3'), Mismatch.backendOlder);
    expect(mismatch('v0.10.0', 'v0.9.0'), Mismatch.backendOlder);
  });

  Future<Harness> show(
    WidgetTester tester,
    Workspace snapshot, {
    required String app,
  }) async {
    final harness = Harness(snapshot)..appVersion = app;
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('a backend too old for this app says to update Claktory, and '
      'Update does it', (tester) async {
    final harness = await show(
      tester,
      _workspace(version: 'v0.1.11', latest: 'v0.2.0', latestBackend: 'v0.2.0'),
      app: 'v0.2.0',
    );

    expect(find.text('Update Claktory to use this app'), findsOneWidget);
    expect(
      find.text(
        'This app is v0.2.0, but Claktory on mac-mini is still v0.1.11.',
      ),
      findsOneWidget,
    );
    expect(find.text('Click Update above to update it.'), findsOneWidget);
    // The banner above it still works.
    await tester.tap(find.text('Update').hitTestable());
    await settle(tester);
    expect(harness.last.hasInstallUpdate(), isTrue);
  });

  testWidgets('with no release to update to, it says where to update', (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace(version: 'v0.1.11'),
      app: 'v0.2.0',
    );

    expect(find.text('Update Claktory to use this app'), findsOneWidget);
    expect(
      find.text('Update Claktory on mac-mini, then come back.'),
      findsOneWidget,
    );
    expect(find.text('Update'), findsNothing);

    // It steps aside for What's new, to read what an update brings.
    harness.page.value = AppPage.changelog;
    await settle(tester);
    expect(find.byType(VersionStep), findsNothing);
  });

  testWidgets('an app too old for its Claktory says so, and Update replaces '
      'it', (tester) async {
    final harness = await show(
      tester,
      _workspace(version: 'v0.2.0', latest: 'v0.2.0', latestBackend: 'v0.2.0'),
      app: 'v0.1.11',
    );

    expect(find.text('This app is too old for this Claktory'), findsOneWidget);
    expect(
      find.text(
        'This app is v0.1.11, but Claktory on mac-mini is already v0.2.0.',
      ),
      findsOneWidget,
    );
    expect(find.text('Click Update above to update the app.'), findsOneWidget);
    await tester.tap(find.text('Update').hitTestable());
    await settle(tester);
    expect(harness.last.installUpdate.appPath, isNotEmpty);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('versions that work together show nothing', (tester) async {
    await show(
      tester,
      _workspace(version: 'v0.1.11', latest: 'v0.1.12'),
      app: 'v0.1.12',
    );

    expect(find.byType(VersionStep), findsNothing);
  });
}
