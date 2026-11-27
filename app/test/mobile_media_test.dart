// Images and videos on an iPhone (research mobile.md §4, "Comments and
// annotations"): Comment under the picture, in the thumb's reach, then a
// tap for a spot, a drag for an area, two fingers to zoom meanwhile, or
// Whole image with no gesture at all; a pin a fingertip reaches. A
// video's controls a finger's size, its timeline a slider VoiceOver
// moves, and Comment twice writes about the moment on screen.
import 'package:factory_app/annotations.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show SemanticsAction;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes, snapshot;
import 'video_test.dart' show FakePlayback;
import 'viewers_test.dart' show png;

const _chart = '/w/chart.png';
const _river = '/w/river.mp4';

Thread _spot() => Thread(
  id: 's1',
  agentId: 'a1',
  path: _chart,
  kind: 'image',
  versionSha256: 'v1',
  status: ThreadStatus.THREAD_STATUS_OPEN,
  anchor: AnchorState.ANCHOR_STATE_ANCHORED,
  selectors: const Region(Rect.fromLTWH(0.5, 0.5, 0, 0)).selectors,
  comments: [ThreadComment(id: 'c1', body: 'This dot.')],
);

Iterable<ClientMessage> _sent(
  Harness harness,
  bool Function(ClientMessage) of,
) => harness.backend.sent.where(of);

/// An iPhone 16 with [path] pushed over Builder's conversation.
Future<Harness> _phone(
  WidgetTester tester,
  String path,
  FileContent file, {
  String kind = 'image',
  List<Thread> threads = const [],
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(snapshot(layout: beside(kind, path)));
  harness.backend.files[path] = file;
  harness.backend.rangedReads = true;
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  await harness.show(tester, size: const Size(393, 852));
  await pumpFrames(tester);
  await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
  await pumpFrames(tester);
  await tester.tap(find.text(path.split('/').last).last);
  await pumpFrames(tester);
  harness.backend.threads.apply(
    ThreadList(path: path, versionSha256: 'v1', threads: threads),
  );
  // Decoding, or writing the video's copy, is real work.
  for (var i = 0; i < 6; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await pumpFrames(tester);
  await tester.pump();
  return harness;
}

Future<Harness> _chartOnPhone(
  WidgetTester tester, {
  List<Thread> threads = const [],
}) async {
  final data = await png(tester, 400, 200);
  return _phone(
    tester,
    _chart,
    FileContent(
      path: _chart,
      mime: 'image/png',
      size: Int64(data.length),
      data: data,
      sha256: 'v1',
    ),
    threads: threads,
  );
}

Rect _picture(WidgetTester tester) => tester.getRect(find.byType(RawImage));

/// The sheet's comment field: written in, then Return.
Future<void> _write(WidgetTester tester, String body) async {
  final field = find.descendant(
    of: find.byType(PhoneSheet),
    matching: find.byType(EditableText),
  );
  await tester.enterText(field.last, body);
  await tester.testTextInput.receiveAction(TextInputAction.send);
  await pumpFrames(tester);
}

void main() {
  group('image', () {
    testWidgets('Comment is under the picture, alone; picking offers Whole '
        'image, which needs no gesture', (tester) async {
      final harness = await _chartOnPhone(tester);
      // The bar's Comment only: no button floating over the picture.
      expect(find.text('Comment'), findsOneWidget);
      final comment = tester.getRect(find.text('Comment'));
      expect(comment.top, greaterThan(_picture(tester).bottom));
      await tester.tap(find.text('Comment'));
      await pumpFrames(tester);
      expect(find.text('Tap a spot, or drag across an area.'), findsOneWidget);
      expect(find.text('Cancel'), findsOneWidget);
      await tester.tap(find.text('Whole image'));
      await pumpFrames(tester);
      expect(find.byType(PhoneSheet), findsOneWidget);
      await _write(tester, 'Brighter, all of it.');
      final create = _sent(
        harness,
        (m) => m.hasCreateThread(),
      ).single.createThread;
      expect(create.body, 'Brighter, all of it.');
      expect(create.path, _chart);
      expect(
        create.selectors.map((s) => s.json).join(),
        contains('xywh=percent:0,0,100,100'),
      );
    });

    testWidgets('picking, one finger drags an area and two zoom without '
        'marking anything', (tester) async {
      final harness = await _chartOnPhone(tester);
      await tester.tap(find.text('Comment'));
      await pumpFrames(tester);
      final before = _picture(tester);

      // Two fingers spread: bigger, and nothing marked.
      final center = before.center;
      final one = await tester.startGesture(center - const Offset(20, 0));
      final two = await tester.startGesture(
        center + const Offset(20, 0),
        pointer: 2,
      );
      for (var i = 1; i <= 5; i++) {
        await one.moveTo(center - Offset(20.0 + i * 12, 0));
        await two.moveTo(center + Offset(20.0 + i * 12, 0));
        await tester.pump();
      }
      await one.up();
      await two.up();
      await pumpFrames(tester);
      expect(_picture(tester).width, greaterThan(before.width * 1.5));
      expect(find.byType(PhoneSheet), findsNothing);
      expect(find.text('Cancel'), findsOneWidget, reason: 'still picking');

      // One finger across: that area, written about at once.
      // Inside the picture, zoomed past the screen's width.
      const from = Offset(120, 380);
      expect(_picture(tester).contains(from), isTrue);
      await tester.timedDragFrom(
        from,
        const Offset(80, 60),
        const Duration(milliseconds: 300),
      );
      await pumpFrames(tester);
      expect(find.byType(PhoneSheet), findsOneWidget);
      expect(find.byType(RegionThumbnail), findsOneWidget);
      await _write(tester, 'This part.');
      final create = _sent(
        harness,
        (m) => m.hasCreateThread(),
      ).single.createThread;
      expect(create.selectors.map((s) => s.json).join(), contains('xywh'));
    });

    testWidgets("a spot's pin is a fingertip's target", (tester) async {
      await _chartOnPhone(tester, threads: [_spot()]);
      final box = _picture(tester);
      // 20pt off the pin: on a Mac it'd be a miss.
      await tester.tapAt(box.center + const Offset(14, 14));
      await tester.pump(const Duration(milliseconds: 400));
      await pumpFrames(tester);
      expect(find.byType(PhoneSheet), findsOneWidget);
      expect(find.text('This dot.'), findsOneWidget);
    });
  });

  group('video', () {
    final system = newVideoPlayback;
    setUp(() {
      FakePlayback.made.clear();
      newVideoPlayback = FakePlayback.new;
    });
    tearDown(() => newVideoPlayback = system);

    Future<Harness> riverOnPhone(WidgetTester tester) {
      const size = 1024 * 1024;
      return _phone(
        tester,
        _river,
        FileContent(
          path: _river,
          mime: 'video/mp4',
          size: Int64(size),
          data: List<int>.filled(size, 7),
          sha256: 'v1',
        ),
        kind: 'video',
      );
    }

    testWidgets("its controls are a finger's size, past Back's edge, and "
        'VoiceOver moves along the timeline', (tester) async {
      final semantics = tester.ensureSemantics();
      await riverOnPhone(tester);
      final player = FakePlayback.made.single;
      final play = tester.getRect(find.bySemanticsLabel('Play').last);
      expect(play.width, greaterThanOrEqualTo(44));
      expect(play.height, greaterThanOrEqualTo(44));
      final timeline = tester.getRect(find.byType(Timeline));
      expect(timeline.height, greaterThanOrEqualTo(44));
      expect(timeline.left, greaterThanOrEqualTo(20));

      // A tap anywhere in its height scrubs.
      await tester.tapAt(Offset(timeline.center.dx, timeline.top + 3));
      await pumpFrames(tester);
      expect(player.seeks.last, closeTo(30, 1));

      final slider = find.bySemanticsLabel('Position');
      expect(
        tester.getSemantics(slider),
        matchesSemantics(
          label: 'Position',
          value: '0:30 of 1:00',
          increasedValue: '0:33 of 1:00',
          decreasedValue: '0:27 of 1:00',
          isSlider: true,
          hasIncreaseAction: true,
          hasDecreaseAction: true,
        ),
      );
      tester.semantics.performAction(
        find.semantics.byLabel('Position'),
        SemanticsAction.increase,
      );
      await pumpFrames(tester);
      expect(player.seeks.last, closeTo(33, 1));
      semantics.dispose();
    });

    testWidgets('Comment twice writes about the moment on screen; a tap on '
        'the timeline picks another first', (tester) async {
      final harness = await riverOnPhone(tester);
      final player = FakePlayback.made.single;
      await player.seek(12);
      await pumpFrames(tester);
      await tester.tap(find.text('Comment'));
      await pumpFrames(tester);
      expect(find.textContaining('At 0:12.'), findsOneWidget);
      // No button over the frame: the bar's Cancel and Comment only.
      expect(find.text('Comment'), findsOneWidget);
      expect(find.text('Cancel'), findsOneWidget);

      final timeline = tester.getRect(find.byType(Timeline));
      await tester.tapAt(Offset(timeline.center.dx, timeline.center.dy));
      await pumpFrames(tester);
      expect(find.textContaining('At 0:30.'), findsOneWidget);
      expect(player.seeks.last, 12, reason: 'marking, not scrubbing');

      await tester.tap(find.text('Comment'));
      await pumpFrames(tester);
      expect(find.byType(PhoneSheet), findsOneWidget);
      await _write(tester, 'Too slow here.');
      final create = _sent(
        harness,
        (m) => m.hasCreateThread(),
      ).single.createThread;
      expect(create.body, 'Too slow here.');
      expect(create.selectors.map((s) => s.json).join(), contains('t=30'));
      // Written: picking's over.
      expect(find.text('Cancel'), findsNothing);
    });

    testWidgets('Cancel stops picking and lets go of the moment', (
      tester,
    ) async {
      await riverOnPhone(tester);
      await tester.tap(find.text('Comment'));
      await pumpFrames(tester);
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
      expect(find.textContaining('At 0:'), findsNothing);
      expect(find.text('Comment'), findsOneWidget);
    });
  });

  testWidgets('the Mac keeps its button over the picture', (tester) async {
    final data = await png(tester, 400, 200);
    await showPanes(
      tester,
      beside('image', _chart),
      files: {
        _chart: FileContent(
          path: _chart,
          mime: 'image/png',
          size: Int64(data.length),
          data: data,
          sha256: 'v1',
        ),
      },
    );
    tester.view.physicalSize = const Size(2000, 900);
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 300)),
    );
    await pumpFrames(tester);
    expect(find.byType(CommentPick), findsOneWidget);
    expect(find.byType(CommentModeBar), findsNothing);
  });
}
