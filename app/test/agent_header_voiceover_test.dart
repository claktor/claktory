// The agent's header for VoiceOver (Abbacchio's row 1615): each control is
// an element of its own with a tap, so pressing it with a screen reader
// does what a click does, rather than landing on the pane around it.
import 'dart:ui' show Tristate;

import 'package:factory_app/inbox.dart' show NeedsYouList;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart' show SemanticsAction, SemanticsNode;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent_menu_test.dart' show header, showHeader;
import 'harness.dart';

/// The element [label] reads as: its own node, with a tap.
SemanticsNode _node(WidgetTester tester, String label) {
  final node = tester.semantics.find(find.bySemanticsLabel(label));
  expect(node.label, label, reason: 'merged with nothing else');
  expect(
    node.getSemanticsData().hasAction(SemanticsAction.tap),
    isTrue,
    reason: '$label has a tap',
  );
  return node;
}

/// Presses [label] as VoiceOver does.
Future<void> _press(WidgetTester tester, String label) async {
  final node = _node(tester, label);
  for (final view in tester.binding.renderViews) {
    view.owner?.semanticsOwner?.performAction(node.id, SemanticsAction.tap);
  }
  await settle(tester);
}

void main() {
  testWidgets('the name opens its settings', (tester) async {
    final semantics = tester.ensureSemantics();
    await showHeader(tester, header());
    await _press(tester, 'June, Researcher');
    expect(find.text('Agent settings'), findsOneWidget);
    semantics.dispose();
  });

  testWidgets('the model opens its popover, and Undo sets it back', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = await showHeader(tester, header());
    await _press(tester, 'Model: Claude Opus 4.8 · High');
    expect(find.text('Reasoning effort'), findsOneWidget);
    await tester.tap(find.text('High').last);
    await settle(tester);
    await tester.tap(find.text('Low').last);
    await settle(tester);
    await harness.apply(tester, header()..agents.first.effort = 'low');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    // What it changed to reads on its own, not with the pane.
    expect(
      tester.semantics.find(find.bySemanticsLabel('Changed to Low · ')).label,
      'Changed to Low · ',
    );
    await _press(tester, 'Undo, back to High');
    expect(harness.last.updateAgent.effort, 'high');
    semantics.dispose();
  });

  testWidgets('Tasks and Checklist open their panels', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await showHeader(
      tester,
      header(checklistOpen: 4, tasksRunning: 2),
    );
    await _press(tester, 'Tasks, 2 running');
    expect(harness.last.openPanel.extensionId, 'tasks');
    await _press(tester, 'Checklist, 4 open');
    expect(harness.last.openPanel.extensionId, 'checklist');
    // Toggles that read as on, never dimmed (Zucchero's row 695).
    for (final label in ['Tasks, 2 running', 'Checklist, 4 open']) {
      final flags = _node(tester, label).getSemanticsData().flagsCollection;
      expect(flags.isEnabled, Tristate.isTrue, reason: label);
    }
    semantics.dispose();
  });

  testWidgets('the meter reads as what its card says', (tester) async {
    final semantics = tester.ensureSemantics();
    await showHeader(tester, header());
    const words =
        'This conversation: 212k of 400k. At 400k, June summarizes it and '
        'carries on.';
    expect(tester.semantics.find(find.bySemanticsLabel(words)).label, words);
    semantics.dispose();
  });

  testWidgets('Needs you opens what waits', (tester) async {
    final semantics = tester.ensureSemantics();
    final snapshot = header()..agents.first.waitingSeqs.add(Int64(7));
    final harness = Harness(snapshot);
    harness.backend.entriesById['a1/7'] = Entry(
      seq: Int64(7),
      timestampMs: Int64(1007),
      askPerson: AskPerson(
        id: 'ask7',
        title: 'Which region should the bucket live in?',
        wantsText: true,
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
      ),
    );
    await harness.show(tester, size: const Size(2200, 900));
    await settle(tester);
    await _press(tester, '1 thing needs your answer. Show it.');
    expect(find.byType(NeedsYouList), findsOneWidget);
    semantics.dispose();
  });
}
