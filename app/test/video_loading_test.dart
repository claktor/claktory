// A video opened on a phone: from the moment it's opened
// until it plays, its name, how much of how much has come, and a bar that
// moves, never an empty screen; on a slow link too, its preview's.
import 'dart:async';

import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/loading_progress.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show snapshot;
import 'video_test.dart' show FakePlayback;

const _path = '/w/river.mp4';

/// How far the bar is drawn, of its length.
double _filled(WidgetTester tester) =>
    tester.getSize(find.byKey(const ValueKey('loading-fill'))).width /
    LoadingProgress.barWidth;

Future<void> _disk(WidgetTester tester) async {
  for (var i = 0; i < 4; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 50)),
    );
    await tester.pump();
  }
}

void main() {
  final system = newVideoPlayback;
  setUp(() => newVideoPlayback = FakePlayback.new);
  tearDown(() => newVideoPlayback = system);

  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets('on a phone (${platform.name}): its name, then how much of '
        'how much with the bar that far, then it plays', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      const size = 3 * videoChunk;
      final harness = Harness(snapshot());
      harness.backend
        ..appliesLayouts = true
        ..files[_path] = FileContent(
          path: _path,
          mime: 'video/mp4',
          size: Int64(size),
          data: List.filled(size, 7),
          sha256: 'v1',
        );
      // Each read waits to be let through, as on a slow link.
      final gates = <Completer<void>>[];
      harness.backend.holdRead = (request) {
        final gate = Completer<void>();
        gates.add(gate);
        return gate.future;
      };
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      tester
          .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
          .routes
          .showFile('a1', _path);
      await pumpFrames(tester);

      // Opened, nothing come yet: what's coming, never an empty screen.
      final page = find.byType(LoadingProgress);
      expect(page, findsOneWidget);
      expect(
        find.descendant(of: page, matching: find.text('river.mp4')),
        findsOneWidget,
      );
      expect(find.text('Loading video…'), findsOneWidget);
      // How much is to come isn't known: a stretch goes along the bar.
      expect(find.byKey(const ValueKey('loading-going')), findsOneWidget);

      // The first piece: how much of how much, the bar a third along.
      gates.last.complete();
      await _disk(tester);
      expect(find.text('Loading video… 8.4 MB of 25.2 MB'), findsOneWidget);
      expect(_filled(tester), closeTo(1 / 3, 0.01));
      // Then a megabyte at a time, each kept as it comes.
      gates.last.complete();
      await _disk(tester);
      expect(find.text('Loading video… 9.4 MB of 25.2 MB'), findsOneWidget);
      expect(_filled(tester), closeTo(9 / 24, 0.01));

      // All of it: it plays.
      for (var i = 0; i < 20 && !gates.last.isCompleted; i++) {
        gates.last.complete();
        await _disk(tester);
      }
      await pumpFrames(tester);
      expect(find.byType(LoadingProgress), findsNothing);
      expect(FakePlayback.made.last.opened, contains('river.mp4'));
      expect(tester.takeException(), isNull);
    });
  }
}
