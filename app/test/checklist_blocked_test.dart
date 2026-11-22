// A blocked checklist item says what it waits on (#255): on you, for how
// long and whether its agent asked; else the blocker's few words. One
// blocked on you that nobody asked about stands out in signal.
import 'package:factory_app/extensions.dart' show blockedFor;
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, checklistView, now, showWith, workspace;
import 'harness.dart';

ChecklistItem _blocked({
  String on = '',
  String note = '',
  Duration since = Duration.zero,
  bool asked = false,
  String owner = '',
}) => ChecklistItem(
  id: 4,
  text: 'Publish the site',
  status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
  created: at(now),
  owner: owner,
  note: note,
  blockedOn: on,
  blockedSinceMs: since == Duration.zero ? Int64(0) : at(now.subtract(since)),
  hasPendingAsk: asked,
);

Future<Harness> _show(
  WidgetTester tester,
  ChecklistItem item, {
  Workspace? snapshot,
}) async {
  final harness = await showWith(
    tester,
    snapshot ?? workspace(openPanel: 'checklist'),
  );
  final view = checklistView()..checklist.items.add(item);
  harness.backend.extensionView.apply(view);
  await settle(tester);
  return harness;
}

void main() {
  test('how long, in the app units', () {
    expect(blockedFor(const Duration(seconds: 20)), 'under a minute');
    expect(blockedFor(const Duration(minutes: 40)), '40 min');
    expect(blockedFor(const Duration(hours: 3)), '3 h');
    expect(blockedFor(const Duration(hours: 30)), '1 day');
    expect(blockedFor(const Duration(days: 2, hours: 1)), '2 days');
  });

  testWidgets('blocked on you, never asked: how long, in signal, over its '
      'note', (tester) async {
    await _show(
      tester,
      _blocked(
        on: 'person',
        since: const Duration(days: 2, hours: 3),
        note: 'Needs the domain picked',
      ),
    );
    final line = find.text('Blocked on you for 2 days · not asked yet');
    expect(line, findsOneWidget);
    final signal = Palette.of(tester.element(line)).signal;
    expect(tester.widget<Text>(line).style?.color, signal);
    expect(
      tester.getTopLeft(line).dy,
      lessThan(tester.getTopLeft(find.text('Needs the domain picked')).dy),
    );
  });

  testWidgets('blocked on you with a question waiting: See question opens '
      'it', (tester) async {
    final snapshot = workspace(openPanel: 'checklist');
    snapshot.agents.firstWhere((a) => a.id == 'a2').waitingSeqs.add(Int64(41));
    final harness = await _show(
      tester,
      _blocked(
        on: 'person',
        since: const Duration(days: 1),
        asked: true,
        owner: 'a2',
      ),
      snapshot: snapshot,
    );
    expect(find.text('Blocked on you'), findsOneWidget);
    expect(find.textContaining('not asked yet'), findsNothing);
    await tester.tap(find.text('See question'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'a2');
  });

  testWidgets('blocked on something else: its words', (tester) async {
    await _show(tester, _blocked(on: 'the CI queue'));
    expect(find.text('Blocked on the CI queue'), findsOneWidget);
    expect(find.textContaining('Blocked on you'), findsNothing);
  });

  testWidgets('blocked from before: only its note, as before', (tester) async {
    await _show(tester, _blocked(note: 'Waiting on the domain to transfer'));
    expect(find.text('Waiting on the domain to transfer'), findsOneWidget);
    expect(find.textContaining('Blocked on'), findsNothing);
  });
}
