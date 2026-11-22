// Typed blocks: a blocked item says what kind of thing it waits on (the
// person's decision or action, another item or agent, something outside
// with when it's tried again); ongoing work set aside is Parked, apart from
// Blocked, with when it resumes; and an item waiting on the person with no card asking is in
// Needs you on its own.
import 'package:factory_app/extensions.dart' show statusLabel, waitingWords;
import 'package:factory_app/identity.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, checklistView, now, showWith, workspace;
import 'harness.dart';

final _ws = workspace();

String _words(
  BlockedKind kind, {
  String on = '',
  String ref = '',
  Duration since = Duration.zero,
  DateTime? retry,
  DateTime? reask,
  bool dismissed = false,
  bool asked = false,
}) => waitingWords(
  kind: kind,
  blockedOn: on,
  ref: ref,
  sinceMs: since == Duration.zero
      ? 0
      : now.subtract(since).millisecondsSinceEpoch,
  retryAtMs: retry?.millisecondsSinceEpoch ?? 0,
  reaskAtMs: reask?.millisecondsSinceEpoch ?? 0,
  dismissedMs: dismissed
      ? now.subtract(const Duration(hours: 1)).millisecondsSinceEpoch
      : 0,
  hasPendingAsk: asked,
  agentId: 'a1',
  checklistAgentId: 'a1',
  workspace: _ws,
  now: now,
).text;

String _clock(DateTime time) =>
    '${time.hour.toString().padLeft(2, '0')}:'
    '${time.minute.toString().padLeft(2, '0')}';

final _tonight = DateTime(now.year, now.month, now.day, 23, 59);

void main() {
  test('what each kind of wait says', () {
    const decision = BlockedKind.BLOCKED_KIND_PERSON_DECISION;
    expect(
      _words(decision, since: const Duration(days: 2, hours: 1)),
      'Waiting 2 days for your decision · not asked yet',
    );
    expect(
      _words(BlockedKind.BLOCKED_KIND_PERSON_ACTION),
      'Waiting for you to act · not asked yet',
    );
    expect(_words(decision, asked: true), 'Waiting for your decision');
    final later = DateTime(now.year, now.month, now.day, 23, 59);
    expect(
      _words(decision, dismissed: true, reask: later),
      'Waiting for your decision · you left it to Builder · asks again at '
      '${_clock(later)}',
    );
    expect(
      _words(decision, dismissed: true),
      // No cap on re-asking; with no time set, it's left to the agent.
      'Waiting for your decision · you left it to Builder',
    );
    expect(
      _words(
        BlockedKind.BLOCKED_KIND_ITEM,
        ref: 'a2#12',
        on: 'Fix the scrollbar in the long checklist panel',
      ),
      'Waiting for Tester: Fix the scrollbar in the long…',
    );
    expect(
      _words(BlockedKind.BLOCKED_KIND_ITEM, ref: 'a1#3', on: 'Someday'),
      'Waiting for: Someday',
    );
    expect(
      _words(BlockedKind.BLOCKED_KIND_AGENT, ref: 'a2'),
      'Waiting for Tester',
    );
    expect(
      _words(
        BlockedKind.BLOCKED_KIND_EXTERNAL,
        on: 'the CI queue',
        retry: later,
      ),
      'Waiting for the CI queue · checks again at ${_clock(later)}',
    );
    // From before kinds: as before.
    expect(
      _words(BlockedKind.BLOCKED_KIND_UNSPECIFIED, on: 'the CI queue'),
      'Blocked on the CI queue',
    );
    expect(
      _words(BlockedKind.BLOCKED_KIND_UNSPECIFIED, on: 'person'),
      'Blocked on you · not asked yet',
    );
  });

  testWidgets('the panel: a typed wait under its item, parked work last '
      'and counted apart', (tester) async {
    // Tall enough for every item.
    final harness = Harness(workspace(openPanel: 'checklist'));
    await harness.show(tester, size: const Size(1200, 1600));
    await settle(tester);
    final view = checklistView()
      ..checklist.items.addAll([
        ChecklistItem(
          id: 4,
          text: 'Pick the region',
          status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
          created: at(now),
          blockedOn: 'person',
          blockedKind: BlockedKind.BLOCKED_KIND_PERSON_DECISION,
          blockedSinceMs: at(now.subtract(const Duration(days: 2, hours: 1))),
        ),
        ChecklistItem(
          id: 5,
          text: 'Ship the build',
          status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
          created: at(now),
          blockedOn: 'Usage circles',
          blockedKind: BlockedKind.BLOCKED_KIND_ITEM,
          blockedRef: 'a2#2',
        ),
        ChecklistItem(
          id: 6,
          text: 'Weekly cleanup',
          status: ChecklistStatus.CHECKLIST_STATUS_PARKED,
          created: at(now.subtract(const Duration(days: 3))),
          note: 'Every Friday',
        ),
        ChecklistItem(
          id: 7,
          text: 'Rerun the benchmark',
          status: ChecklistStatus.CHECKLIST_STATUS_PARKED,
          created: at(now.subtract(const Duration(days: 3))),
          retryAtMs: at(_tonight),
        ),
      ]);
    harness.backend.extensionView.apply(view);
    await settle(tester);

    final decision = find.text(
      'Waiting 2 days for your decision · not asked yet',
    );
    expect(decision, findsOneWidget);
    expect(
      tester.widget<Text>(decision).style?.color,
      Palette.of(tester.element(decision)).signal,
    );
    expect(find.text('Waiting for Tester: Usage circles'), findsOneWidget);
    // Parked: last of what's outstanding, with its note, counted apart.
    expect(statusLabel(ChecklistStatus.CHECKLIST_STATUS_PARKED), 'Parked');
    expect(find.text('Every Friday'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Weekly cleanup')).dy,
      greaterThan(tester.getTopLeft(find.text('Someday')).dy),
    );
    // Parked until a time: when it goes back to open.
    expect(find.text('Resumes at ${_clock(_tonight)}'), findsOneWidget);
    expect(find.textContaining('2 parked'), findsOneWidget);
    expect(find.textContaining('2 blocked'), findsOneWidget);
  });

  group('Needs you', () {
    Workspace waiting({bool asked = false, DateTime? reask}) {
      final snapshot = workspace();
      snapshot.agents
          .firstWhere((agent) => agent.id == 'a2')
          .waitingItems
          .add(
            WaitingItem(
              item: 7,
              text: 'Pick the bucket region',
              kind: BlockedKind.BLOCKED_KIND_PERSON_DECISION,
              sinceMs: at(now.subtract(const Duration(days: 2))),
              hasPendingAsk: asked,
              reaskAtMs: reask == null ? null : at(reask),
              dismissedMs: reask == null
                  ? null
                  : at(now.subtract(const Duration(hours: 1))),
            ),
          );
      return snapshot;
    }

    test('an item with a card asking counts once, as its card', () {
      expect(needsYouCount(waiting()), 1);
      expect(needsYouCount(waiting(asked: true)), 0);
    });

    testWidgets('an item waiting on you with no card: its words, how long, '
        'and its checklist', (tester) async {
      final reask = now.add(const Duration(hours: 5));
      final harness = await showWith(tester, waiting(reask: reask));
      harness.page.value = AppPage.inbox;
      await settle(tester);
      await settle(tester);

      expect(
        find.text('Waiting for your decision: Pick the bucket region'),
        findsOneWidget,
      );
      expect(
        find.textContaining('Tester · 2 days ago', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(
        find.text('Waiting for your decision: Pick the bucket region'),
      );
      await pumpFrames(tester);
      expect(
        find.textContaining('you left it to Tester · asks again'),
        findsOneWidget,
      );

      await tester.tap(find.text('Show in checklist'));
      await settle(tester);
      expect(
        harness.backend.sent.any((m) => m.selectAgent.agentId == 'a2'),
        isTrue,
      );
      final open = harness.last.openPanel;
      expect(open.agentId, 'a2');
      expect(open.extensionId, 'checklist');
    });
  });

  testWidgets("Show in checklist: the checklist scrolls to the item, not to "
      'its end, even when it comes after', (tester) async {
    final harness = Harness(workspace(openPanel: 'checklist'));
    await harness.show(tester, size: const Size(1200, 700));
    await settle(tester);
    ExtensionView long() => checklistView()
      ..checklist.items.addAll([
        ChecklistItem(
          id: 50,
          text: 'Pick the launch date',
          status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
          created: at(now.subtract(const Duration(days: 2))),
          blockedOn: 'person',
          blockedKind: BlockedKind.BLOCKED_KIND_PERSON_DECISION,
          blockedSinceMs: at(now.subtract(const Duration(days: 2))),
        ),
        for (var i = 0; i < 40; i++)
          ChecklistItem(
            id: 100 + i,
            text: 'Step $i',
            status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
            created: at(DateTime(now.year, now.month, now.day, 1, i)),
          ),
      ]);
    bool inView(Finder finder) {
      if (finder.evaluate().isEmpty) return false;
      final y = tester.getCenter(finder).dy;
      return y > 0 && y < 700;
    }

    // Left alone, it follows its end.
    harness.backend.extensionView.apply(long());
    await pumpFrames(tester);
    expect(inView(find.text('Step 39')), isTrue);
    expect(inView(find.text('Pick the launch date')), isFalse);

    // Pointed at before the checklist comes, as Needs you does.
    harness.backend.extensionView.apply(checklistView(agentId: 'a2'));
    await settle(tester);
    harness.backend.highlights.pointedAt.value = (
      keys: ['checklist_item:a1#50'],
      at: DateTime.now(),
    );
    harness.backend.extensionView.apply(long());
    await pumpFrames(tester);
    expect(inView(find.text('Pick the launch date')), isTrue);
    // And it stays there as the list changes.
    harness.backend.extensionView.apply(long());
    await pumpFrames(tester);
    expect(inView(find.text('Pick the launch date')), isTrue);
  });
}
