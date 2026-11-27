// What's copied from a selection reads as it's laid out: an item's rows on
// lines of their own, a blank line between items, and nothing trailing
// (QA rows 46 and Abbacchio's Needs you pass: lines ran together).
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage, AppScope;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:factory_app/controls.dart' show showModal;

import 'harness.dart';
import 'selection_test.dart' show watchClipboard;
import 'app_test.dart' show builder, workspace;

void main() {
  testWidgets("the conversation's Select All: items apart, nothing after", (
    tester,
  ) async {
    final copied = watchClipboard(tester);
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            assistant: AssistantText(text: 'First reply'),
          ),
          Entry(
            seq: Int64(2),
            event: Event(
              source: EventSource.EVENT_SOURCE_AGENT,
              from: 'Kenji (Frontend)',
              fromId: 'kenji',
              text: 'Second, from Kenji',
              intent: 'status',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    final region = tester.state<SelectableRegionState>(
      find.byType(SelectableRegion).first,
    );
    region.selectAll(SelectionChangedCause.keyboard);
    await tester.pump();
    Actions.invoke(
      tester.element(find.text('First reply', findRichText: true).first),
      CopySelectionTextIntent.copy,
    );
    await tester.pump();
    final text = copied.single;
    expect(text, isNot(endsWith('\n')), reason: text);
    expect(text, contains('First reply\n\n'), reason: text);
    expect(text, contains('Second, from Kenji\nStatus update'), reason: text);
  });

  testWidgets("Needs you's History: each row apart", (tester) async {
    final copied = watchClipboard(tester);
    final now = DateTime.now();
    NeedsYouRecord record(int seq, String title, String reply) =>
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
          agentId: 'a1',
          agentName: 'June',
          seq: Int64(seq),
          title: title,
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_ANSWERED,
          reply: reply,
          askedMs: Int64(now.millisecondsSinceEpoch - 7200000),
          decidedMs: Int64(now.millisecondsSinceEpoch - 3600000 + seq),
        );
    final harness = Harness(workspace(agents: [builder(working: false)]))
      ..backend.needsYouHistory = [
        record(2, 'Pick a colour', 'Blue please'),
        record(1, 'Which region?', 'eu-west-1'),
      ];
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    await pumpFrames(tester);
    final region = tester.state<SelectableRegionState>(
      find.ancestor(
        of: find.text('Pick a colour'),
        matching: find.byType(SelectableRegion),
      ),
    );
    region.selectAll(SelectionChangedCause.keyboard);
    await tester.pump();
    Actions.invoke(
      tester.element(find.text('Pick a colour')),
      CopySelectionTextIntent.copy,
    );
    await tester.pump();
    final text = copied.single;
    expect(text, isNot(endsWith('\n')), reason: text);
    // Its search box's placeholder isn't history.
    expect(text, isNot(contains('Search by agent')), reason: text);
    final colour = text.indexOf('Pick a colour');
    final region2 = text.indexOf('Which region?');
    expect(colour, greaterThanOrEqualTo(0), reason: text);
    expect(region2, greaterThan(colour), reason: text);
    expect(text.substring(colour, region2), contains('\n\n'), reason: text);
  });

  // Abbacchio's: ⌘A selected the conversation clicked in before, not Needs
  // you clicked in since, unless that click was a drag on its words.
  group('⌘A selects where you last clicked', () {
    Harness harness() {
      final harness = Harness(
        Workspace(
          projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
          agents: [
            Agent(
              id: 'june',
              name: 'June',
              projectId: 'p1',
              modelAvailable: true,
              status: AgentStatus.AGENT_STATUS_IDLE,
            ),
            Agent(
              id: 'omar',
              name: 'Omar',
              projectId: 'p1',
              modelAvailable: true,
              status: AgentStatus.AGENT_STATUS_IDLE,
              waitingSeqs: [Int64(2)],
            ),
          ],
          selectedAgentId: 'june',
        ),
      );
      harness.backend.entriesById['omar/2'] = Entry(
        seq: Int64(2),
        timestampMs: Int64(500),
        proposal: Proposal(id: 'prop2', text: 'Add a designer agent'),
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(
              seq: Int64(1),
              assistant: AssistantText(text: 'Line one of the conversation'),
            ),
          ],
        ),
      );
      return harness;
    }

    Future<void> click(WidgetTester tester, Finder finder) async {
      await tester.tap(finder, kind: PointerDeviceKind.mouse);
      await settle(tester);
      await pumpFrames(tester);
    }

    Future<void> openNeedsYou(WidgetTester tester, Harness harness) async {
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      await pumpFrames(tester);
      // Beside the conversation, where a pane opens it.
      harness.backend.appliesLayouts = true;
      AppScope.openPage(
        tester.element(
          find.text('Line one of the conversation', findRichText: true).first,
        ),
        AppPage.inbox,
      );
      await settle(tester);
      await pumpFrames(tester);
    }

    testWidgets('a click on a Needs you card, after one in the conversation', (
      tester,
    ) async {
      final copied = watchClipboard(tester);
      await openNeedsYou(tester, harness());
      await click(
        tester,
        find.text('Line one of the conversation', findRichText: true).first,
      );
      await click(tester, find.textContaining('Add a designer agent').first);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.pump();
      final text = copied.single;
      expect(text, contains('Add a designer agent'), reason: text);
      expect(text, isNot(contains('Line one')), reason: text);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('a click on a Needs you card, from the message box', (
      tester,
    ) async {
      final copied = watchClipboard(tester);
      await openNeedsYou(tester, harness());
      await click(tester, find.byType(EditableText).last);
      await click(tester, find.textContaining('Add a designer agent').first);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.pump();
      expect(copied.single, contains('Add a designer agent'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  });

  // Abbacchio's next two: a click on the pane's tabs, and its area gone.
  group('⌘A in the Needs you page', () {
    Future<(Harness, List<String>)> showHistory(WidgetTester tester) async {
      final copied = watchClipboard(tester);
      final now = DateTime.now();
      final harness = Harness(workspace(agents: [builder(working: false)]))
        ..backend.needsYouHistory = [
          NeedsYouRecord(
            kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
            agentId: 'a1',
            agentName: 'June',
            seq: Int64(1),
            title: 'Pick a colour',
            outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_ANSWERED,
            reply: 'Blue please',
            askedMs: Int64(now.millisecondsSinceEpoch - 7200000),
            decidedMs: Int64(now.millisecondsSinceEpoch - 3600000),
          ),
        ];
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              assistant: AssistantText(text: 'Line one of the conversation'),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      // Beside the conversation, as the sidebar opens it.
      harness.backend.appliesLayouts = true;
      AppScope.openPage(
        tester.element(
          find.text('Line one of the conversation', findRichText: true).first,
        ),
        AppPage.inbox,
      );
      await settle(tester);
      await pumpFrames(tester);
      expect(
        find.text('Line one of the conversation', findRichText: true),
        findsWidgets,
      );
      await tester.tap(find.text('History'), kind: PointerDeviceKind.mouse);
      await settle(tester);
      await settle(tester);
      await pumpFrames(tester);
      return (harness, copied);
    }

    Future<void> click(WidgetTester tester, Finder finder) async {
      await tester.tap(finder, kind: PointerDeviceKind.mouse);
      await settle(tester);
      await pumpFrames(tester);
    }

    Future<void> selectAllCopy(WidgetTester tester) async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.pump();
    }

    testWidgets('a click on its tabs, after one in the conversation', (
      tester,
    ) async {
      final (_, copied) = await showHistory(tester);
      await click(
        tester,
        find.text('Line one of the conversation', findRichText: true).first,
      );
      await click(tester, find.text('History'));
      await selectAllCopy(tester);
      expect(copied.single, contains('Pick a colour'));
      expect(copied.single, isNot(contains('Line one')));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('the pane gone (a row opened another conversation): ⌘A '
        'goes to the conversation shown', (tester) async {
      final (harness, copied) = await showHistory(tester);
      await click(tester, find.text('History'));
      // As opening another agent's conversation does: the pane goes.
      await harness.backend.setLayout('a1', null);
      await settle(tester);
      await pumpFrames(tester);
      expect(find.text('Pick a colour'), findsNothing);
      await selectAllCopy(tester);
      expect(copied.single, contains('Line one of the conversation'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('with a dialog open, the pane going: the dialog takes ⌘A, '
        'not what is under it', (tester) async {
      final (harness, _) = await showHistory(tester);
      await click(tester, find.text('History'));
      showModal(
        tester.element(find.text('Pick a colour')),
        (context, close) => const Text('A dialog'),
      );
      await settle(tester);
      await pumpFrames(tester);
      await harness.backend.setLayout('a1', null);
      await settle(tester);
      await pumpFrames(tester);
      expect(find.text('A dialog'), findsOneWidget);
      final primary = FocusManager.instance.primaryFocus;
      final dialogScope = FocusScope.of(tester.element(find.text('A dialog')));
      expect(
        primary == dialogScope ||
            (primary?.ancestors.contains(dialogScope) ?? false),
        isTrue,
        reason: '$primary',
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  });
}
