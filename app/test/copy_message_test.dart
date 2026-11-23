// Copying a whole message in one go (friend bug #5): with a pointer, Copy
// beside an agent's reply's foot while it's under the pointer, and Copy
// message in the conversation's right-click menu, for any message. It
// copies what a selection of the whole message would, as a phone's Copy.
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/markdown.dart' show markdownToPlain;
import 'package:factory_app/message_time.dart' show TimeFoot;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'events_test.dart' show writer;
import 'harness.dart';
import 'selection_test.dart' show rightClick, watchClipboard;

const _reply = 'I **shipped** it.\n\n- the app\n- the docs';
const _short = 'Done.';

Harness _harness() {
  final harness = Harness(
    workspace(agents: [builder(working: false), writer()])
      ..selectedAgentId = 'a1',
  );
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(text: 'Ship **it** today'),
        ),
        Entry(
          seq: Int64(2),
          timestampMs: Int64(
            DateTime(2026, 11, 22, 11, 31).millisecondsSinceEpoch,
          ),
          assistant: AssistantText(text: _reply),
        ),
        Entry(
          seq: Int64(3),
          event: Event(
            source: EventSource.EVENT_SOURCE_AGENT,
            from: 'Writer (Docs)',
            fromId: 'a2',
            text: 'Docs are *done*',
          ),
        ),
        Entry(
          seq: Int64(4),
          assistant: AssistantText(text: _short),
        ),
      ],
    ),
  );
  return harness;
}

Finder _message(String words) => find
    .descendant(
      of: find.byType(TranscriptView),
      matching: find.textContaining(words, findRichText: true),
    )
    .first;

/// The painted bubble around [words]: the Bubble widget spans the row.
Finder _bubbleOf(String words) => find
    .ancestor(
      of: _message(words),
      matching: find.byWidgetPredicate(
        (widget) =>
            widget is CustomPaint &&
            widget.painter.runtimeType.toString() == '_BubblePainter',
      ),
    )
    .first;

final Finder _copy = find.byWidgetPredicate(
  (widget) => widget is GlyphButton && widget.label == 'Copy message',
);

Future<TestGesture> _mouse(WidgetTester tester) async {
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: const Offset(1, 1));
  addTearDown(mouse.removePointer);
  return mouse;
}

Future<void> _show(
  WidgetTester tester, {
  Size size = const Size(1200, 900),
}) async {
  await _harness().show(tester, size: size);
  await settle(tester);
  await pumpFrames(tester);
}

void main() {
  final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

  testWidgets("Copy shows beside a reply's foot under the pointer, moving "
      'nothing, and copies the whole reply', (tester) async {
    final copied = watchClipboard(tester);
    await _show(tester);
    final before = tester.getRect(_bubbleOf('shipped'));
    expect(_copy, findsNothing, reason: 'only under the pointer');

    final mouse = await _mouse(tester);
    await mouse.moveTo(tester.getCenter(_message('shipped')));
    await tester.pump();
    expect(_copy, findsOneWidget);
    // Beside the bubble, at its foot; the bubble as it was.
    expect(tester.getRect(_bubbleOf('shipped')), before);
    final copy = tester.getRect(_copy);
    expect(copy.left, greaterThanOrEqualTo(before.right));
    expect(copy.bottom, moreOrLessEquals(before.bottom));

    // Onto it, and click: still shown on the way.
    await mouse.moveTo(copy.center);
    await tester.pump();
    await tester.tapAt(copy.center, kind: PointerDeviceKind.mouse);
    await tester.pump();
    expect(copied, [markdownToPlain(_reply)]);
    expect(copied.single, isNot(contains('**')));
    expect(find.text('Copied'), findsOneWidget);
    expect(tester.getRect(_bubbleOf('shipped')), before);

    // Away: "Copied" for a moment, then nothing.
    await mouse.moveTo(const Offset(1, 1));
    await tester.pump();
    expect(find.text('Copied'), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('Copied'), findsNothing);
    expect(_copy, findsNothing);
  }, variant: mac);

  testWidgets('one reply at a time: the one under the pointer', (tester) async {
    final copied = watchClipboard(tester);
    await _show(tester);
    final mouse = await _mouse(tester);
    await mouse.moveTo(tester.getCenter(_message(_short)));
    await tester.pump();
    expect(_copy, findsOneWidget);
    expect(
      tester.getRect(_copy).left,
      greaterThanOrEqualTo(tester.getRect(_bubbleOf(_short)).right),
    );
    await tester.tapAt(tester.getCenter(_copy), kind: PointerDeviceKind.mouse);
    await tester.pump();
    expect(copied, [_short]);
    // Not beside the person's message or another agent's.
    for (final other in ['Ship', 'Docs are']) {
      await mouse.moveTo(tester.getCenter(_message(other)));
      await tester.pump(const Duration(seconds: 2));
      expect(_copy, findsNothing, reason: other);
    }
  }, variant: mac);

  testWidgets("the right click's menu copies the message it's on, any "
      "message, as it reads; yours as you typed it", (tester) async {
    final copied = watchClipboard(tester);
    await _show(tester);
    final mouse = await _mouse(tester);
    for (final (words, expected) in [
      ('shipped', markdownToPlain(_reply)),
      ('Ship', 'Ship **it** today'),
      ('Docs are', 'Docs are done'),
    ]) {
      final at = tester.getCenter(_message(words));
      await mouse.moveTo(at);
      await tester.pump();
      await rightClick(tester, _message(words), at: at);
      await pumpFrames(tester);
      expect(find.text('Reply'), findsOneWidget, reason: words);
      await tester.tap(find.text('Copy message'));
      await pumpFrames(tester);
      expect(copied.last, expected, reason: words);
    }
  }, variant: mac);

  testWidgets('on an iPad with a pointer too', (tester) async {
    final copied = watchClipboard(tester);
    await _show(tester, size: const Size(1180, 820));
    final mouse = await _mouse(tester);
    await mouse.moveTo(tester.getCenter(_message('shipped')));
    await tester.pump();
    await tester.tapAt(tester.getCenter(_copy), kind: PointerDeviceKind.mouse);
    await tester.pump();
    expect(copied, [markdownToPlain(_reply)]);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  testWidgets("not on a phone: its Copy is in the message's touch and hold", (
    tester,
  ) async {
    await _show(tester, size: const Size(390, 844));
    final mouse = await _mouse(tester);
    await mouse.moveTo(tester.getCenter(_message('shipped')));
    await tester.pump();
    expect(_copy, findsNothing);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  for (final (name, size) in [
    ('a wide window', const Size(1200, 900)),
    ('a narrow one', const Size(620, 900)),
  ]) {
    testWidgets('in $name, Copied shows where the button was, clear of the '
        "reply's time (Abbacchio's row 668)", (tester) async {
      watchClipboard(tester);
      await _show(tester, size: size);
      final mouse = await _mouse(tester);
      await mouse.moveTo(tester.getCenter(_message('shipped')));
      await tester.pump();
      final button = tester.getRect(_copy);
      await tester.tapAt(button.center, kind: PointerDeviceKind.mouse);
      await tester.pump();
      final copied = find.text('Copied');
      expect(copied, findsOneWidget);
      final check = find.byWidgetPredicate(
        (widget) => widget is GlyphButton && widget.label == 'Copied',
      );
      expect(tester.getRect(check), button);
      final time = find
          .descendant(
            of: find.ancestor(
              of: _message('shipped'),
              matching: find.byType(TimeFoot),
            ),
            matching: find.textContaining(RegExp(r'\d:\d\d')),
          )
          .first;
      final overlap = tester.getRect(copied).intersect(tester.getRect(time));
      expect(
        overlap.width <= 0 || overlap.height <= 0,
        isTrue,
        reason: '${tester.getRect(copied)} over ${tester.getRect(time)}',
      );
      expect(
        tester.getRect(check).overlaps(tester.getRect(time)),
        isFalse,
        reason: 'the check, where the button was',
      );
    }, variant: mac);
  }
}
