// Android hooks for shared conversation Find and Report this reply.
// Framework/recording-backend evidence only: no physical keyboard, native IME,
// live report service, or emulator acceptance is implied by these tests.
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/report_reply.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../app_test.dart' show builder, workspace;
import '../harness.dart';
import 'support/android_harness.dart';

Harness _conversation() {
  final harness = Harness(
    workspace(agents: [builder(working: false)])..selectedAgentId = 'a1',
  );
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(text: 'Private user context'),
        ),
        for (var seq = 2; seq <= 4; seq++)
          Entry(
            seq: Int64(seq),
            timestampMs: Int64(seq * 1000),
            assistant: AssistantText(text: 'Lapwing reply $seq.'),
          ),
      ],
    ),
  );
  harness.backend.matches = (request) => ConversationMatches(
    agentId: request.agentId,
    query: request.query,
    matches: [
      for (final seq in [4, 3, 2])
        ConversationMatch(
          seq: Int64(seq),
          snippet: 'Lapwing reply $seq.',
          starts: [0],
          lengths: [7],
          hits: 1,
          timestampMs: Int64(seq * 1000),
        ),
    ],
    total: 3,
  );
  // Deliberately different from the rendered conversation: the sheet must
  // display only factoryd's preview, not copy arbitrary transcript content.
  harness.backend.replyReport = ReplyReport(
    text: 'Only the backend-selected reply.',
    model: 'QA model',
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
  for (final request in harness.backend.sent)
    if (request.hasReportReply()) request.reportReply,
];

Future<void> _controlF(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await pumpFrames(tester);
}

Future<void> _report(WidgetTester tester) async {
  await tester.longPress(_message('Lapwing reply 4.'));
  await pumpFrames(tester);
  expect(find.text('Report…'), findsOneWidget);
  await tester.tap(find.text('Report…'));
  await pumpFrames(tester);
  await settle(tester);
  expect(find.byType(ReportReplyForm), findsOneWidget);
}

void main() {
  setUp(() => PhonePlatformPolicy.debugAndroidPolicy = true);
  tearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
  final android = TargetPlatformVariant.only(TargetPlatform.android);

  for (final scale in [1.0, 2.0]) {
    testWidgets(
      'Android Search menu and Ctrl+F preserve draft at text scale $scale',
      (tester) async {
        final harness = _conversation();
        await showAndroid(tester, harness: harness, textScale: scale);
        await tester.enterText(find.byType(EditableText).last, 'Unsent draft');
        await tester.tap(find.bySemanticsLabel('Extensions').last);
        await pumpFrames(tester);
        expect(find.text('Search'), findsOneWidget);
        // Search is the first action in the conversation's more sheet.
        expect(
          tester.getTopLeft(find.text('Search')).dy,
          lessThan(
            tester
                .getTopLeft(
                  find.textContaining(
                    RegExp(r"^Builder(?:'s extensions| has no extensions)"),
                  ),
                )
                .dy,
          ),
        );
        await tester.tap(find.text('Search'));
        await pumpFrames(tester);
        expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
        expect(find.text('Unsent draft'), findsNothing);
        await tester.enterText(
          find.byKey(const ValueKey('phone-find')),
          'lapwing',
        );
        await tester.pump(const Duration(milliseconds: 200));
        await pumpFrames(tester);
        final requests = harness.backend.sent
            .where((request) => request.hasSearchConversation())
            .map((request) => request.searchConversation);
        expect(requests.single.agentId, 'a1');
        expect(requests.single.query, 'lapwing');
        expect(find.text('1 of 3'), findsOneWidget);

        // Framework inset, not a claim that a real Android keyboard opened.
        tester.view.viewInsets = const FakeViewPadding(bottom: 280);
        addTearDown(tester.view.resetViewInsets);
        await pumpFrames(tester);
        expect(
          tester.getRect(find.byKey(const ValueKey('phone-find-bar'))).bottom,
          closeTo(852 - 280, 1),
        );
        final next = find.bySemanticsLabel('Next match').last;
        final previous = find.bySemanticsLabel('Previous match').last;
        expect(
          tester.getCenter(next).dx,
          lessThan(tester.getCenter(previous).dx),
        );
        await tester.tap(next);
        await pumpFrames(tester);
        expect(find.text('2 of 3'), findsOneWidget);
        await tester.tap(previous);
        await pumpFrames(tester);
        expect(find.text('1 of 3'), findsOneWidget);
        await tester.testTextInput.receiveAction(TextInputAction.search);
        await pumpFrames(tester);
        expect(find.text('2 of 3'), findsOneWidget);
        await tester.tap(find.text('Done'));
        await pumpFrames(tester);
        expect(find.byKey(const ValueKey('phone-find-bar')), findsNothing);
        expect(find.text('Unsent draft'), findsOneWidget);

        // Android uses Ctrl, not Apple's Command modifier. It must also
        // work while the message composer, rather than the pane, has focus.
        await tester.tap(find.byType(EditableText).last);
        await _controlF(tester);
        expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
        await tester.tap(find.text('Done'));
        await pumpFrames(tester);
        expect(find.text('Unsent draft'), findsOneWidget);
        expect(tester.takeException(), isNull);
      },
      variant: android,
    );
  }

  testWidgets(
    'Android report preview cancels without sending and sends only on Send',
    (tester) async {
      final harness = _conversation();
      await showAndroid(tester, harness: harness);
      await _report(tester);
      expect(find.text('Only the backend-selected reply.'), findsOneWidget);
      expect(find.text('Model: QA model'), findsOneWidget);
      expect(find.text('Version: v0.1.17'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(ReportReplyForm),
          matching: find.textContaining('Private user context'),
        ),
        findsNothing,
      );
      expect(_reports(harness).single.send, isFalse);
      expect(_reports(harness).single.seq.toInt(), 4);
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
      expect(find.byType(ReportReplyForm), findsNothing);
      expect(_reports(harness).where((request) => request.send), isEmpty);

      await _report(tester);
      await tester.enterText(
        find.descendant(
          of: find.byType(ReportReplyForm),
          matching: find.byType(EditableText),
        ),
        'Optional report note',
      );
      expect(_reports(harness).where((request) => request.send), isEmpty);
      await tester.tap(find.text('Send'));
      await settle(tester);
      final sent = _reports(harness).where((request) => request.send).single;
      expect(sent.agentId, 'a1');
      expect(sent.seq.toInt(), 4);
      expect(sent.note, 'Optional report note');
      expect(
        find.text("Report sent. Thanks, Claktory's team will look at it."),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    },
    variant: android,
  );

  testWidgets('Android user message does not offer Report', (tester) async {
    await showAndroid(tester, harness: _conversation());
    await tester.longPress(_message('Private user context'));
    await pumpFrames(tester);
    expect(find.text('Copy'), findsOneWidget);
    expect(find.text('Report…'), findsNothing);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
  }, variant: android);
}
