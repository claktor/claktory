// Needs you at the largest text on a phone: a
// row's name and what it asks wrap rather than being cut, a procedure's
// step included ("Rotate the router password · S…"), and an answered
// row's name beside Answered ("Ri…").
import 'package:factory_app/inbox.dart' show NeedsYouList;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _ax5 = 53 / 17;

Workspace _workspace(List<int> waiting) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [for (final seq in waiting) Int64(seq)],
    ),
  ],
);

Entry _ask(
  int seq,
  String title, {
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(DateTime.now().millisecondsSinceEpoch - 60000),
  askPerson: AskPerson(
    id: 'ask$seq',
    title: title,
    wantsText: true,
    state: state,
    answer: state == AskPersonState.ASK_PERSON_STATE_ANSWERED ? 'Done' : '',
  ),
);

/// Every paragraph in Needs you that was cut short.
List<String> _cut(WidgetTester tester) => [
  for (final paragraph in tester.renderObjectList<RenderParagraph>(
    find.descendant(
      of: find.byType(NeedsYouList),
      matching: find.byType(RichText),
    ),
  ))
    if (paragraph.didExceedMaxLines) paragraph.text.toPlainText(),
];

void main() {
  testWidgets('at the largest text on a phone, a row wraps its name and '
      'what it asks, answered or not', (tester) async {
    // Android's phone, which keeps Needs you's list; an iPhone's shows one
    // card a screen (mobile_needs_you_test at the largest text).
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = _ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    const step =
        'Rotate the router password · Step 2 of 4: sign in to the router';
    final harness = Harness(_workspace([3, 7]));
    harness.backend.entriesById['rivet/3'] = _ask(3, step);
    harness.backend.entriesById['rivet/7'] = _ask(7, 'Which region?');
    await harness.show(tester, size: const Size(393, 852));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.text(step, findRichText: true), findsOneWidget);

    // One answered: it stays, saying so, with its agent's whole name.
    harness.backend.entriesById['rivet/7'] = _ask(
      7,
      'Which region?',
      state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
    );
    await tester.tap(find.text('Which region?'));
    await settle(tester);
    await harness.apply(tester, _workspace([3]));
    await settle(tester);
    expect(find.text('Answered'), findsWidgets);
    expect(tester.takeException(), isNull);
    expect(_cut(tester), isEmpty);
  });
}
