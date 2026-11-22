// Reporting an agent's reply to Claktory's team (Google Play's policy on
// AI-generated content): only the agent's own reply offers Report…; the
// sheet shows what the backend says will go and sends nothing until Send.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'events_test.dart' show writer;
import 'harness.dart';
import 'selection_test.dart' show rightClick;

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
          user: UserText(text: 'Ship it today'),
        ),
        Entry(
          seq: Int64(2),
          assistant: AssistantText(text: 'I shipped it.'),
        ),
        Entry(
          seq: Int64(3),
          event: Event(
            source: EventSource.EVENT_SOURCE_AGENT,
            from: 'Writer (Docs)',
            fromId: 'a2',
            text: 'Docs are done',
          ),
        ),
      ],
    ),
  );
  harness.backend.replyReport = ReplyReport(
    text: 'I shipped it.',
    model: 'Claude Opus 4.8',
    version: 'v0.1.17',
  );
  return harness;
}

Finder _message(String words) => find
    .descendant(
      of: find.byType(TranscriptView),
      matching: find.textContaining(words, findRichText: true),
    )
    .first;

Iterable<ReportReply> _reports(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReportReply()) message.reportReply,
];

/// Chooses Report… in the menu open, and waits for what would go.
Future<void> openReport(WidgetTester tester) async {
  await tester.tap(find.text('Report…'));
  await settle(tester);
  await pumpFrames(tester);
  await settle(tester);
}

void main() {
  group('on a Mac', () {
    /// Right-clicks [words]' message, the pointer over it first, as a
    /// person's is.
    TestGesture? hovering;
    setUp(() => hovering = null);

    Future<void> menuOn(WidgetTester tester, String words) async {
      final at = tester.getCenter(_message(words));
      var mouse = hovering;
      if (mouse == null) {
        mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: at);
        addTearDown(mouse.removePointer);
        hovering = mouse;
      }
      await mouse.moveTo(at + const Offset(1, 0));
      await tester.pump();
      await rightClick(tester, _message(words), at: at);
      await pumpFrames(tester);
    }

    Future<Harness> show(WidgetTester tester) async {
      final harness = _harness();
      await harness.show(tester);
      await settle(tester);
      await pumpFrames(tester);
      return harness;
    }

    testWidgets("only the agent's own reply offers Report…", (tester) async {
      await show(tester);
      await menuOn(tester, 'I shipped it.');
      expect(find.text('Report…'), findsOneWidget);
      await tester.tapAt(const Offset(5, 5));
      await pumpFrames(tester);
      for (final other in ['Ship it today', 'Docs are done']) {
        await menuOn(tester, other);
        expect(find.text('Reply'), findsOneWidget, reason: other);
        expect(find.text('Report…'), findsNothing, reason: other);
        await tester.tapAt(const Offset(5, 5));
        await pumpFrames(tester);
      }
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('the sheet shows what goes, as the backend has it, and sends '
        'only on Send, with the note', (tester) async {
      final harness = await show(tester);
      await menuOn(tester, 'I shipped it.');
      await openReport(tester);
      expect(find.text('Report this reply'), findsOneWidget);
      expect(find.text('Model: Claude Opus 4.8'), findsOneWidget);
      expect(find.text('Version: v0.1.17'), findsOneWidget);
      expect(
        find.text('This reply is long, so only its beginning is sent.'),
        findsNothing,
      );
      // Asked what would go: nothing sent yet.
      final preview = _reports(harness).single;
      expect(preview.agentId, 'a1');
      expect(preview.chatId, '');
      expect(preview.seq.toInt(), 2);
      expect(preview.send, isFalse);

      await tester.enterText(
        find
            .descendant(
              of: find.byType(Semantics),
              matching: find.byType(EditableText),
            )
            .last,
        'It made that up',
      );
      await tester.tap(find.text('Send'));
      await settle(tester);
      final sent = _reports(harness).last;
      expect(sent.send, isTrue);
      expect(sent.seq.toInt(), 2);
      expect(sent.note, 'It made that up');
      expect(
        find.text("Report sent. Thanks, Claktory's team will look at it."),
        findsOneWidget,
      );
      await tester.tap(find.text('Done'));
      await settle(tester);
      expect(find.text('Report this reply'), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('a long reply says only its beginning goes', (tester) async {
      final harness = await show(tester);
      harness.backend.replyReport = harness.backend.replyReport.deepCopy()
        ..truncated = true;
      await menuOn(tester, 'I shipped it.');
      await openReport(tester);
      expect(
        find.text('This reply is long, so only its beginning is sent.'),
        findsOneWidget,
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("refused: too many says so, and can't be tried again; "
        "claktory.com out of reach can", (tester) async {
      final harness = await show(tester);
      // Claktory's words say how long; without any, the app's own.
      harness.backend.refuseReport = const RequestFailure(
        '',
        code: ErrorCode.ERROR_CODE_RATE_LIMITED,
      );
      await menuOn(tester, 'I shipped it.');
      await openReport(tester);
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(
        find.text('Too many reports for now. Try again later.'),
        findsOneWidget,
      );
      harness.backend.refuseReport = const RequestFailure(
        "You've sent a lot of reports. Try again in 40 minutes.",
        code: ErrorCode.ERROR_CODE_RATE_LIMITED,
      );
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      await menuOn(tester, 'I shipped it.');
      await openReport(tester);
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(
        find.text("You've sent a lot of reports. Try again in 40 minutes."),
        findsOneWidget,
      );
      final sends = _reports(harness).where((r) => r.send).length;
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(_reports(harness).where((r) => r.send).length, sends);

      await tester.tap(find.text('Cancel'));
      await settle(tester);
      harness.backend.refuseReport = const RequestFailure(
        '',
        code: ErrorCode.ERROR_CODE_UNREACHABLE,
      );
      await menuOn(tester, 'I shipped it.');
      await openReport(tester);
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(
        find.text(
          "Couldn't send the report: claktory.com can't be reached. Try "
          'again in a moment.',
        ),
        findsOneWidget,
      );
      harness.backend.refuseReport = null;
      await tester.tap(find.text('Try again'));
      await settle(tester);
      expect(
        find.text("Report sent. Thanks, Claktory's team will look at it."),
        findsOneWidget,
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  });

  group('on a phone', () {
    Future<Harness> show(WidgetTester tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = _harness();
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      return harness;
    }

    testWidgets("touch and hold the agent's reply: Report… opens the sheet", (
      tester,
    ) async {
      final harness = await show(tester);
      await tester.longPress(_message('I shipped it.'));
      await pumpFrames(tester);
      expect(find.text('Report…'), findsOneWidget);
      await tester.tap(find.text('Report…'));
      await pumpFrames(tester);
      await settle(tester);
      expect(find.text('Report this reply'), findsOneWidget);
      expect(find.text('Model: Claude Opus 4.8'), findsOneWidget);
      expect(_reports(harness).single.send, isFalse);
    });

    testWidgets("the person's own message and another agent's don't offer it", (
      tester,
    ) async {
      await show(tester);
      for (final other in ['Ship it today', 'Docs are done']) {
        await tester.longPress(_message(other));
        await pumpFrames(tester);
        expect(find.text('Copy'), findsOneWidget, reason: other);
        expect(find.text('Report…'), findsNothing, reason: other);
        await tester.tap(find.text('Cancel'));
        await pumpFrames(tester);
      }
    });
  });
}
