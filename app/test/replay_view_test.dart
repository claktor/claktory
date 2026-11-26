import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/history/board.dart';
import 'package:factory_app/history/scrubber.dart' show HistoryScrubber;
import 'package:factory_app/history/view.dart';
import 'package:factory_app/map.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/rendering.dart' show OffsetLayer, RenderParagraph;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'map_fixture.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'replay_fixture.dart';

/// History replay (#458): the Map's History mode plays what the backend
/// recorded.
void main() {
  Future<Harness> showHistory(
    WidgetTester tester, {
    Size size = const Size(1440, 900),
    bool reply = true,
  }) async {
    final harness = Harness(claktoryWorkspace());
    if (reply) {
      harness.backend
        ..historyReply = fixtureOverview()
        ..historyPages = [fixtureEvents()];
    }
    await harness.show(tester, size: size);
    harness.page.value = AppPage.map;
    await pumpFrames(tester);
    return harness;
  }

  Finder mode(String name) => find.descendant(
    of: find.byType(MapModeSwitch),
    matching: find.text(name),
  );

  Finder inBoard(String text) =>
      find.descendant(of: find.byType(HistoryBoard), matching: find.text(text));

  double opacityOf(WidgetTester tester, String name) {
    final fade = find.ancestor(
      of: inBoard(name).first,
      matching: find.byType(AnimatedOpacity),
    );
    return tester.widget<AnimatedOpacity>(fade.first).opacity;
  }

  testWidgets('History plays the team growing, from day one', (tester) async {
    final harness = await showHistory(tester);
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasGetHistoryOverview()),
      hasLength(1),
    );
    expect(
      harness.backend.sent
          .where((m) => m.hasWatchHistory())
          .single
          .watchHistory
          .on,
      isTrue,
    );
    // Day one: Rivet, reconstructed; Kai and Ana not yet there.
    expect(opacityOf(tester, 'Rivet'), 1);
    expect(opacityOf(tester, 'Kai'), 0);
    expect(find.text('Recording began'), findsNothing);

    // Next chapter: the project's first agent.
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    await pumpFrames(tester);
    expect(opacityOf(tester, 'Kai'), 1);
    expect(find.textContaining('App started'), findsWidgets);
    // Before recording began: said first, so a narrow line never cuts it
    // (Zucchero's row 502).
    expect(
      find.textContaining('Pieced together · ', findRichText: true),
      findsOneWidget,
    );

    // Playing runs on into Ana's arrival and on to the end, where it
    // follows now.
    await tester.tap(find.bySemanticsLabel('Play'));
    await tester.pump();
    for (var i = 0; i < 120; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(opacityOf(tester, 'Ana'), 1);
    // Space pauses it.
    await tester.tap(find.byType(HistoryBoard), warnIfMissed: false);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pump();
    expect(find.bySemanticsLabel('Play'), findsOneWidget);

    // Back to Now: the Map as it is.
    await tester.tap(mode('Now'));
    await pumpFrames(tester);
    expect(find.byType(HistoryView), findsNothing);
    expect(harness.backend.sent.last.watchHistory.on, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a chip pinned shows how much it talked', (tester) async {
    await showHistory(tester);
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    // To the end of the history: a click on the board takes the keys.
    await tester.tap(find.byType(HistoryBoard), warnIfMissed: false);
    await tester.sendKeyEvent(LogicalKeyboardKey.end);
    await tester.pump();
    await tester.tap(find.bySemanticsLabel('Pause'));
    await pumpFrames(tester);
    await tester.tap(inBoard('Ana').first);
    await pumpFrames(tester);
    expect(find.text('Sent 1 · Received 2'), findsOneWidget);
    expect(find.text('Most with You'), findsOneWidget);
  });

  testWidgets('going ahead of what is loaded says so, and loads there '
      'first', (tester) async {
    final harness = Harness(claktoryWorkspace());
    final hold = Completer<void>();
    harness.backend
      ..historyReply = (fixtureOverview()..cursors.addAll(fixtureCursors()))
      ..historyPages = fixturePages()
      ..historyHold = hold.future;
    await harness.show(tester, size: const Size(1440, 900));
    harness.page.value = AppPage.map;
    await pumpFrames(tester);
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    expect(find.textContaining('Loading…'), findsOneWidget);
    // To recording beginning, at hour 30: past the second page's start.
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    await tester.pump();
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    await tester.pump();
    expect(find.textContaining('Recording began'), findsNothing);
    expect(find.textContaining('Loading…'), findsOneWidget);
    hold.complete();
    await pumpFrames(tester);
    expect(find.textContaining('Loading…'), findsNothing);
    expect(find.textContaining('Recording began'), findsOneWidget);
    final asked = [
      for (final message in harness.backend.sent)
        if (message.hasGetHistoryEvents()) message.getHistoryEvents.afterCursor,
    ];
    expect(asked.take(2), [
      <int>[],
      <int>[1],
    ]);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a Claktory from before History says to update', (tester) async {
    final harness = await showHistory(tester, reply: false);
    harness.backend.refuse = 'No such thing.';
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    expect(find.textContaining("Couldn't load the history."), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
  });

  testWidgets('on a phone, the controls are a bar with the chapters in a '
      'sheet', (tester) async {
    await showHistory(tester, size: const Size(390, 844));
    await tester.tap(find.bySemanticsLabel('History'));
    await pumpFrames(tester);
    expect(find.byType(HistoryView), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Chapters'));
    await pumpFrames(tester);
    await tester.tap(find.text('Recording began'));
    await pumpFrames(tester);
    expect(opacityOf(tester, 'Ana'), 1);
    expect(find.byType(MapView), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  group('at the largest text', () {
    // The real font's widths: the test font's squares wrap everything.
    // Loaded before, so its change of fonts is no test's.
    setUpAll(loadGeist);

    testWidgets("at the largest text on a small phone, when it is and the "
        "chapter keep their words, and the bar leaves the Map its room "
        "(Zucchero's row 508)", (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await showHistory(tester, size: const Size(375, 667));
      await tester.tap(find.bySemanticsLabel('History'));
      await pumpFrames(tester);
      // A chapter before recording began: "Pieced together" and its title.
      await tester.tap(find.bySemanticsLabel('Chapters'));
      await pumpFrames(tester);
      await tester.tap(find.text('App started').last);
      await pumpFrames(tester);
      if (Platform.environment['SHOTS'] != null) {
        await _shot(tester, 'phone-ax5');
      }
      expect(tester.takeException(), isNull);
      final bar = find
          .ancestor(
            of: find.byType(HistoryScrubber),
            matching: find.byType(Column),
          )
          .first;
      // No word of it cut: the time, the chapter, the speed.
      final words = find.descendant(of: bar, matching: find.byType(RichText));
      expect(words, findsWidgets);
      for (final element in words.evaluate()) {
        final paragraph = element.renderObject! as RenderParagraph;
        expect(
          paragraph.didExceedMaxLines,
          isFalse,
          reason: paragraph.text.toPlainText(),
        );
      }
      expect(
        find.descendant(of: bar, matching: find.text('Auto')),
        findsOneWidget,
      );
      final auto = tester.getSize(
        find.descendant(of: bar, matching: find.text('Auto')),
      );
      expect(auto.height, lessThan(60), reason: 'one line');
      // The bar takes well under half the screen.
      expect(tester.getTopLeft(bar).dy, greaterThan(667 * 0.55));
    });

    testWidgets("at the largest text on a phone turned on its side, the "
        "bar is one row, so the board keeps its height (row 525)", (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await showHistory(tester, size: const Size(667, 375));
      await tester.tap(find.bySemanticsLabel('History'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final bar = find
          .ancestor(
            of: find.byType(HistoryScrubber),
            matching: find.byType(Column),
          )
          .first;
      expect(
        find.descendant(of: bar, matching: find.text('Auto')),
        findsOneWidget,
      );
      // Under half the height: the board keeps the rest.
      expect(tester.getTopLeft(bar).dy, greaterThan(375 * 0.5));
    });

    testWidgets("at the largest text on an iPad, the wide bar's time, chapter "
        "and speed keep their words (Zucchero's row 508, iPad)", (
      tester,
    ) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await showHistory(tester, size: const Size(1024, 768));
      await tester.tap(mode('History'));
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('Next chapter'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final bar = find
          .ancestor(
            of: find.byType(HistoryScrubber),
            matching: find.byType(Column),
          )
          .first;
      final words = find.descendant(of: bar, matching: find.byType(RichText));
      expect(words, findsWidgets);
      for (final element in words.evaluate()) {
        final paragraph = element.renderObject! as RenderParagraph;
        expect(
          paragraph.didExceedMaxLines,
          isFalse,
          reason: paragraph.text.toPlainText(),
        );
      }
      final speed = find.descendant(
        of: bar,
        matching: find.text('Whole history in a minute'),
      );
      expect(speed, findsOneWidget);
      expect(tester.getSize(speed).height, lessThan(80), reason: 'one line');
    });

    /// No word of the History bar cut: the time, the chapter, the speed.
    void readsWhole(WidgetTester tester) {
      final bar = find
          .ancestor(
            of: find.byType(HistoryScrubber),
            matching: find.byType(Column),
          )
          .first;
      final words = find.descendant(of: bar, matching: find.byType(RichText));
      expect(words, findsWidgets);
      for (final element in words.evaluate()) {
        final paragraph = element.renderObject! as RenderParagraph;
        expect(
          paragraph.didExceedMaxLines,
          isFalse,
          reason: paragraph.text.toPlainText(),
        );
      }
    }

    // iOS's biggest size before the accessibility ones (xxxLarge, 23 pt).
    const xxxLarge = 23 / 17;

    for (final (width, phone) in [
      (760.0, false),
      (487.0, true),
      (321.0, true),
    ]) {
      testWidgets("at a large text size ${width.round()} points wide, when it "
          "is and the chapter read whole (Sale's row 149)", (tester) async {
        asIPad();
        tester.platformDispatcher.textScaleFactorTestValue = xxxLarge;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await showHistory(tester, size: Size(width, 1024));
        await tester.tap(
          phone ? find.bySemanticsLabel('History') : mode('History'),
        );
        await pumpFrames(tester);
        // A chapter before recording began: "Pieced together" and its
        // title, the longest line there is.
        if (phone) {
          await tester.tap(find.bySemanticsLabel('Chapters'));
          await pumpFrames(tester);
          await tester.tap(find.text('App started').last);
        } else {
          await tester.tap(find.bySemanticsLabel('Next chapter'));
        }
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        expect(
          find.textContaining('Pieced together · ', findRichText: true),
          findsOneWidget,
        );
        readsWhole(tester);
      });
    }
  });

  group('at the default text size', () {
    setUpAll(loadGeist);
    for (final width in [820.0, 900.0, 1000.0, 1100.0]) {
      testWidgets("on an iPad ${width.round()} points wide, when it is and the "
          "chapter read whole beside Now and Share replay… (Sale's row 150)", (
        tester,
      ) async {
        asIPad();
        await showHistory(tester, size: Size(width, 1024));
        await tester.tap(mode('History'));
        await pumpFrames(tester);
        await tester.tap(find.bySemanticsLabel('Next chapter'));
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        final bar = find
            .ancestor(
              of: find.byType(HistoryScrubber),
              matching: find.byType(Column),
            )
            .first;
        for (final element
            in find
                .descendant(of: bar, matching: find.byType(RichText))
                .evaluate()) {
          final paragraph = element.renderObject! as RenderParagraph;
          expect(
            paragraph.didExceedMaxLines,
            isFalse,
            reason: paragraph.text.toPlainText(),
          );
        }
      });
    }
  });

  testWidgets("where there's room, the controls are one row, so Now and "
      "Share replay… stay put as the chapter's words change (Sale's row "
      "149)", (tester) async {
    await showHistory(tester);
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    await pumpFrames(tester);
    final play = tester.getCenter(find.bySemanticsLabel('Play')).dy;
    expect(tester.getCenter(find.text('Go to Now')).dy, moreOrLessEquals(play));
    expect(
      tester.getCenter(find.text('Whole history in a minute')).dy,
      moreOrLessEquals(play, epsilon: 2),
    );
  });

  // To look at: SHOTS=/tmp/replay flutter test test/replay_view_test.dart
  testWidgets('shots', (tester) async {
    if (Platform.environment['SHOTS'] == null) return;
    await loadGeist();
    await showHistory(tester);
    await tester.tap(mode('History'));
    await pumpFrames(tester);
    await _shot(tester, 'mac-day1');
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Play'));
    for (var i = 0; i < 60; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    await _shot(tester, 'mac-playing');
    await tester.tap(find.bySemanticsLabel('Next chapter'));
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    await _shot(tester, 'mac-recording');
  });

  testWidgets('phone shots', (tester) async {
    if (Platform.environment['SHOTS'] == null) return;
    await loadGeist();
    await showHistory(tester, size: const Size(390, 844));
    await tester.tap(find.bySemanticsLabel('History'));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Play'));
    for (var i = 0; i < 80; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    await _shot(tester, 'phone');
  });
}

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS']!;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 1),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}
