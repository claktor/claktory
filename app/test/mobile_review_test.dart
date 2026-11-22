// A review an agent asks of a file on an iPhone (#275): PhoneRoutes'
// showFile opens the file as a page over the agent's conversation (chosen
// first, the file put in its layout as its `show` would, unless it's
// there), and the viewer's foot says who asks and what, with Dismiss and
// Approve at thumb reach; Approve's sheet takes an optional note.
import 'dart:convert';

import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;

const _plan = '/w/plan.md';

FileContent _file(String path) {
  const body = '# The plan\n\nShip it on Friday.\n';
  return FileContent(
    path: path,
    mime: 'text/markdown',
    size: Int64(utf8.encode(body).length),
    data: utf8.encode(body),
    sha256: 'v1',
  );
}

Entry _ask({String path = _plan}) => Entry(
  seq: Int64(7),
  askPerson: AskPerson(
    id: 'q1',
    title: 'Review the release plan',
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    reviewPath: path,
  ),
);

Workspace _waiting(Workspace workspace, {String selected = 'a1'}) =>
    workspace.deepCopy()
      ..selectedAgentId = selected
      ..agents
          .firstWhere((agent) => agent.id == 'a1')
          .waitingSeqs
          .add(Int64(7));

PhoneRoutes _routes(WidgetTester tester) =>
    tester.widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope)).routes;

void main() {
  Future<Harness> phone(
    WidgetTester tester, {
    ViewNode? layout,
    String selected = 'a1',
    Size size = const Size(393, 852),
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      _waiting(snapshot(layout: layout), selected: selected),
    );
    harness.backend
      ..appliesLayouts = true
      ..files[_plan] = _file(_plan)
      ..entriesById['a1/7'] = _ask();
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: size);
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('showFile opens the file in the layout and pushes it; '
      'the review shows at its foot', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await phone(tester);
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    final layouts = harness.backend.sent.where((m) => m.hasSetLayout());
    expect(layouts, hasLength(1));
    expect(layouts.single.setLayout.root.split.children.last.leaf.path, _plan);
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
    expect(find.text('plan.md'), findsWidgets);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(find.text('Review the release plan'), findsOneWidget);
    final approve = find.bySemanticsLabel('Approve').last;
    final dismiss = find.bySemanticsLabel('Dismiss').last;
    for (final button in [approve, dismiss]) {
      expect(tester.getSize(button).height, greaterThanOrEqualTo(44));
    }
    // Approve last, under the thumb; both above the home indicator.
    expect(
      tester.getCenter(approve).dx,
      greaterThan(tester.getCenter(dismiss).dx),
    );
    expect(tester.getRect(approve).bottom, lessThanOrEqualTo(852 - 34));
    semantics.dispose();
  });

  testWidgets('a pane already open is pushed as it is', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
  });

  testWidgets('another agent showing: it is chosen, and the file pushed '
      'once it shows', (tester) async {
    final harness = await phone(tester, selected: 'rivet');
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasSelectAgent()).single,
      ClientMessage(selectAgent: SelectAgent(agentId: 'a1')),
    );
    expect(find.text('Builder asks you to review this'), findsNothing);
    // The backend chooses it.
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()..selectedAgentId = 'a1',
    );
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
  });

  testWidgets('from Needs you, Back returns to it', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(harness.page.value, AppPage.inbox);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(find.text('Needs you'), findsWidgets);
    expect(harness.page.value, AppPage.inbox);
  });

  testWidgets("Needs you's Review opens the file over it, with the bar; "
      'Back returns to Needs you', (tester) async {
    final harness = await phone(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('Builder').first);
    await pumpFrames(tester);
    await tester.tap(find.text('Review').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(harness.page.value, AppPage.inbox);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(find.text('Review'), findsWidgets);
  });

  testWidgets('Dismiss takes an optional comment too, sent with it', (
    tester,
  ) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    expect(find.text('A comment for Builder, if you like'), findsOneWidget);
    expect(find.text('Builder gets it with your answer.'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Not this week.');
    await pumpFrames(tester);
    await tester.tap(find.text('Dismiss').last);
    await pumpFrames(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isTrue);
    expect(answer.answerAsk.text, 'Not this week.');
    expect(answer.answerAsk.seq, Int64(7));
  });

  testWidgets('Approve takes an optional note in a sheet', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.tap(find.text('Approve'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    expect(find.text('A comment for Builder, if you like'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Go ahead.');
    await tester.tap(find.text('Approve').last);
    await pumpFrames(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isFalse);
    expect(answer.answerAsk.text, 'Go ahead.');
    expect(
      find.text('A comment for Builder, if you like'),
      findsNothing,
      reason: 'the sheet closed',
    );
  });

  testWidgets('no review waits on another file: no bar', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.backend.entriesById['a1/7'] = _ask(path: '/w/other.md');
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(find.text('plan.md'), findsWidgets);
    expect(find.text('Builder asks you to review this'), findsNothing);
  });

  testWidgets('at the largest text on a small phone, stacked, '
      'nothing overflows', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await phone(
      tester,
      layout: beside('text', _plan),
      size: const Size(375, 667),
    );
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final approve = tester.getRect(find.text('Approve'));
    final dismiss = tester.getRect(find.text('Dismiss'));
    expect(approve.bottom, lessThanOrEqualTo(dismiss.top));
  });
}
