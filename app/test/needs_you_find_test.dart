// Find in Needs you (⌘F): what waits is searched by who asks and what, the
// bubble found brought into sight and marked; History's ⌘F goes to its
// own search field.
import 'package:factory_app/find.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, List<int> waiting) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Entry _ask(int seq, String title) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(1000 + seq),
  askPerson: AskPerson(id: 'ask$seq', title: title, wantsText: true),
);

List<(String, bool)> _washes(WidgetTester tester) {
  final found = <(String, bool)>[];
  for (final widget in tester.widgetList<RichText>(find.byType(RichText))) {
    widget.text.visitChildren((span) {
      final background = span.style?.backgroundColor;
      if (span is TextSpan && background != null && span.text != null) {
        found.add((span.text!, background.a > 0.9));
      }
      return true;
    });
  }
  return found;
}

Future<void> _commandF(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
  await pumpFrames(tester);
}

void main() {
  testWidgets('what waits is found by who asks and what; History goes to its '
      'own search', (tester) async {
    final harness = Harness(
      Workspace(
        projects: [Project(id: 'p1', name: 'Factory', path: '/w/factory')],
        agents: [
          _agent('june', 'June', [7]),
          _agent('omar', 'Omar', [2]),
        ],
      ),
    );
    harness.backend.entriesById['june/7'] = _ask(7, 'Which bucket region?');
    harness.backend.entriesById['omar/2'] = _ask(2, 'Ship the bucket now?');
    await harness.show(tester);
    await settle(tester);
    await openNeedsYouPage(tester);
    await settle(tester);

    await tester.tap(find.textContaining('Which bucket', findRichText: true));
    await pumpFrames(tester);
    await _commandF(tester);
    expect(find.text('Search Needs you'), findsOneWidget);
    final session = tester.widget<FindBar>(find.byType(FindBar)).session;
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) => widget is EditableText && widget.focusNode == session.field,
      ),
      'bucket',
    );
    await tester.pump(const Duration(milliseconds: 200));
    await pumpFrames(tester);
    expect(session.target.countLabel, '1 of 2');
    // Found, the list rests: no frame after frame (Formaggio's check).
    await pumpFrames(tester);
    expect(tester.binding.hasScheduledFrame, isFalse);
    // The newest first: June's, in full; Omar's washed.
    expect(_washes(tester), containsAll([('bucket', true), ('bucket', false)]));
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(session.target.countLabel, '2 of 2');

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.byType(FindBar), findsNothing);
    expect(_washes(tester), isEmpty);

    // History: ⌘F is its own search field's.
    await tester.tap(find.text('History'));
    await settle(tester);
    await tester.tapAt(
      tester.getCenter(find.text('History')) + const Offset(0, 120),
    );
    await pumpFrames(tester);
    await _commandF(tester);
    expect(find.byType(FindBar), findsNothing);
    final field = find.byWidgetPredicate(
      (widget) =>
          widget is EditableText &&
          widget.focusNode.debugLabel == 'Search history',
    );
    expect(tester.widget<EditableText>(field).focusNode.hasFocus, isTrue);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
