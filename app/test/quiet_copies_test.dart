// Quiet copies and message provenance (#285): a lead sees what its team
// handed each other without being woken for it (its Messaging panel, opened
// from its header while it has unread ones), and an agent's message says
// why it was sent and what it replies to.
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/identity.dart' show titleBarHeight;
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/quiet_copies.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'messaging_test.dart' show view, workspace;

/// Builder's workspace (a1, Messaging open) with [unread] copies unread.
Workspace _workspace({int unread = 0, bool messaging = true}) {
  final snapshot = workspace();
  final builder = snapshot.agents.firstWhere((agent) => agent.id == 'a1');
  builder.unreadCopies = unread;
  if (!messaging) builder.extensions.clear();
  return snapshot;
}

/// Tester's message to Writer, copied to Builder.
QuietCopy _copy(int id, {bool read = false}) => QuietCopy(
  id: Int64(id),
  from: 'Tester (QA)',
  fromId: 'a2',
  text: 'Copy $id: the build is green, over to you.',
  createdMs: Int64(DateTime(2026, 11, 22, 14, 2).millisecondsSinceEpoch),
  read: read,
  toIds: ['a3'],
  intent: 'handoff',
  replyToIds: id == 1 ? [Int64(4)] : [],
);

Iterable<ListQuietCopies> _asked(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasListQuietCopies()) message.listQuietCopies,
];

Entry _message(
  int seq,
  String text, {
  int id = 0,
  String intent = '',
  List<int> replyTo = const [],
}) => Entry(
  seq: Int64(seq),
  event: Event(
    source: EventSource.EVENT_SOURCE_AGENT,
    from: 'Kenji (Frontend)',
    fromId: 'kenji',
    text: text,
    messageId: Int64(id),
    intent: intent,
    replyToIds: [for (final id in replyTo) Int64(id)],
  ),
);

Finder _rich(String text) => find.textContaining(text, findRichText: true);

void main() {
  test('intents read in the app\'s words, unknown ones not at all', () {
    expect(intentLabel('handoff'), 'Handoff');
    expect(intentLabel('decision_request'), 'Asks for a decision');
    expect(intentLabel('decision'), 'Decision');
    expect(intentLabel('status'), 'Status update');
    expect(intentLabel('gossip'), '');
    expect(intentLabel(''), '');
  });

  group('Messaging panel', () {
    Future<Harness> show(WidgetTester tester, List<QuietCopy> copies) async {
      final harness = Harness(_workspace(unread: 1));
      harness.backend.quietCopies = copies;
      harness.backend.extensionView.apply(view(['a2'], ['a2']));
      await harness.show(tester, size: const Size(1400, 1600));
      await settle(tester);
      return harness;
    }

    testWidgets('lists its copies newest first, a page at a time', (
      tester,
    ) async {
      final harness = await show(tester, [
        for (var id = 25; id >= 1; id--) _copy(id, read: id < 25),
      ]);
      expect(_asked(harness).single.agentId, 'a1');
      expect(_asked(harness).single.beforeId, Int64.ZERO);
      expect(find.text('Quiet copies'), findsOneWidget);
      expect(
        find.text(
          'Messages its team sent each other, copied to it without '
          "interrupting it. It reads them when it's ready.",
        ),
        findsOneWidget,
      );
      expect(find.text('Copy 25: the build is green, over to you.'), findsOne);
      expect(find.byType(QuietCopyRow), findsNWidgets(quietCopiesPage));
      // Who sent it, to whom, when and why.
      expect(_rich('Tester'), findsWidgets);
      expect(_rich('to Writer'), findsWidgets);
      expect(find.text('Handoff'), findsWidgets);
      expect(
        find.text('14:02').evaluate().isNotEmpty ||
            find.text('22 Nov 14:02').evaluate().isNotEmpty,
        isTrue,
      );
      // Only the newest isn't read yet.
      expect(find.bySemanticsLabel('Not read yet by Builder'), findsOneWidget);

      final earlier = find.text('Show earlier copies');
      // Rows settle their height (Show more) as they're built: reach it
      // again once they have.
      for (var i = 0; i < 3; i++) {
        await tester.ensureVisible(earlier);
        await settle(tester);
      }
      await tester.tap(earlier);
      await settle(tester);
      expect(_asked(harness).last.beforeId, Int64(6));
      expect(find.byType(QuietCopyRow), findsNWidgets(25));
      expect(find.text('Show earlier copies'), findsNothing);
      // A copy that replies says so, never by a number.
      expect(
        find.text('Handoff · Replying to an earlier message'),
        findsOneWidget,
      );
      expect(find.textContaining('#'), findsNothing);
    });

    testWidgets('shows nothing while it has none', (tester) async {
      await show(tester, []);
      expect(find.text('Quiet copies'), findsNothing);
      expect(find.text('Can message'), findsOneWidget);
    });

    testWidgets('asks again when how many are unread changes', (tester) async {
      final harness = await show(tester, [_copy(1)]);
      expect(_asked(harness), hasLength(1));
      harness.backend.quietCopies = [_copy(2), _copy(1)];
      await harness.apply(tester, _workspace(unread: 2));
      await settle(tester);
      expect(_asked(harness), hasLength(2));
      expect(find.byType(QuietCopyRow), findsNWidgets(2));
      // Anything else changing asks nothing.
      await harness.apply(tester, _workspace(unread: 2));
      await settle(tester);
      expect(_asked(harness), hasLength(2));
    });
  });

  group("the agent's header", () {
    testWidgets('shows how many are new, and opens Messaging', (tester) async {
      final harness = Harness(_workspace(unread: 3));
      await harness.show(tester);
      await settle(tester);
      expect(find.text('3 new copies'), findsOneWidget);
      await tester.tap(find.text('3 new copies'));
      await tester.pump();
      final opened = harness.backend.sent.lastWhere((m) => m.hasOpenPanel());
      expect(opened.openPanel.agentId, 'a1');
      expect(opened.openPanel.extensionId, 'agents');
      // Tapping it opens no settings.
      expect(find.byType(AgentSettingsForm), findsNothing);

      await harness.apply(tester, _workspace(unread: 1));
      expect(find.text('1 new copy'), findsOneWidget);
      await harness.apply(tester, _workspace());
      expect(find.byType(QuietCopiesChip), findsNothing);
      await harness.apply(tester, _workspace(unread: 2, messaging: false));
      expect(find.byType(QuietCopiesChip), findsNothing);
    });

    testWidgets('on iPhone, how many with a word, a finger\'s size', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final snapshot = _workspace(unread: 2)..openPanel = '';
      snapshot.agents.firstWhere((a) => a.id == 'a1').openPanel = '';
      final harness = Harness(snapshot);
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      expect(find.byType(PhoneShell), findsOneWidget);
      final chip = find.bySemanticsLabel('2 new copies');
      expect(chip, findsOneWidget);
      expect(find.text('2 copies'), findsOneWidget);
      expect(tester.getSize(chip).height, greaterThanOrEqualTo(44));
      await tester.tap(chip);
      await tester.pump();
      final opened = harness.backend.sent.lastWhere((m) => m.hasOpenPanel());
      expect(opened.openPanel.extensionId, 'agents');
    });
  });

  group('at any text size', () {
    testWidgets('the title bar keeps its height at ordinary text', (
      tester,
    ) async {
      await Harness(_workspace(unread: 3)).show(tester);
      await settle(tester);
      final bar = find
          .ancestor(
            of: find.byType(QuietCopiesChip),
            matching: find.byType(Container),
          )
          .first;
      expect(tester.getSize(bar).height, titleBarHeight);
    });

    for (final scale in [1.5, 2.0]) {
      testWidgets('nothing cut at ${scale}x on an iPad', (tester) async {
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await Harness(_workspace(unread: 128))
            .show(tester, size: const Size(1024, 768));
        await settle(tester);
        expect(find.text('128 new copies'), findsOneWidget);
      }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
    }

    testWidgets('nothing cut on a small iPhone at the largest text', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final snapshot = _workspace(unread: 128)..openPanel = '';
      snapshot.agents.firstWhere((a) => a.id == 'a1').openPanel = '';
      await Harness(snapshot).show(tester, size: const Size(320, 568));
      await pumpFrames(tester);
      expect(find.text('99+ copies'), findsOneWidget);
    });
  });

  group('provenance under a message', () {
    Future<Harness> show(WidgetTester tester, List<Entry> entries) async {
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(agentId: 'a1', entries: entries),
      );
      await harness.show(tester, size: const Size(1400, 1600));
      await settle(tester);
      return harness;
    }

    testWidgets('says why it was sent and that it replies, never by number', (
      tester,
    ) async {
      await show(tester, [
        _message(1, 'Is the plan ready?', id: 7),
        _message(2, 'It is: see plan.md.', intent: 'handoff', replyTo: [7]),
        _message(3, 'Shipped.', intent: 'status'),
        _message(4, 'Both done.', replyTo: [97, 98]),
        _message(5, 'Old message, says nothing.'),
      ]);
      expect(
        find.text('Handoff · Replying to an earlier message'),
        findsOneWidget,
      );
      expect(find.text('Status update'), findsOneWidget);
      expect(find.text('Replying to earlier messages'), findsOneWidget);
      expect(find.textContaining('#'), findsNothing);
      expect(find.textContaining('97'), findsNothing);
      // One that says neither has no line.
      expect(find.textContaining('Replying'), findsNWidgets(2));
    });
  });
}
