// Needs you pulled down to refresh, drawn: with SHOTS set, PNGs of the
// list held mid-pull on an iPhone (its spinner) and on Android (its
// circle), dark and light:
//   SHOTS=/tmp/refresh flutter test test/mobile_needs_you_refresh_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/inbox.dart' show NeedsYouList;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  for (final platform in const [TargetPlatform.iOS, TargetPlatform.android]) {
    for (final dark in const [true, false]) {
      final name = '${platform.name}-${dark ? 'dark' : 'light'}';
      testWidgets('$name: held mid-pull', (tester) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        if (platform == TargetPlatform.android) {
          PhonePlatformPolicy.debugAndroidPolicy = true;
          addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
        }
        tester.platformDispatcher.platformBrightnessTestValue = dark
            ? Brightness.dark
            : Brightness.light;
        addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
        tester.view.padding = const FakeViewPadding(top: 62, bottom: 34);
        addTearDown(tester.view.resetPadding);
        final harness = Harness(
          Workspace(
            agents: [
              Agent(
                id: 'june',
                name: 'June',
                role: 'Backend',
                modelAvailable: true,
                waitingSeqs: [Int64(1)],
              ),
            ],
            selectedAgentId: 'june',
          ),
        );
        harness.backend.entriesById['june/1'] = Entry(
          seq: Int64(1),
          timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
          askPerson: AskPerson(id: 'q1', title: 'Ship the migration tonight?'),
        );
        await harness.show(tester, size: const Size(393, 852));
        harness.page.value = AppPage.inbox;
        await pumpFrames(tester);
        await tester.pump();
        await pumpFrames(tester);
        final gesture = await tester.startGesture(
          tester.getTopLeft(find.byType(NeedsYouList)) + const Offset(200, 40),
        );
        for (var step = 0; step < 6; step++) {
          await gesture.moveBy(const Offset(0, 30));
          await tester.pump(const Duration(milliseconds: 16));
        }
        await pumpFrames(tester);
        await _shot(tester, name);
        await gesture.up();
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
      });
    }
  }
}
