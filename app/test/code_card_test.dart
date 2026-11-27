// The code card: a site an errand is on asks for a code sent to the
// person. Claktory's heading with the site, one field for the code only,
// Can't do this… with its reasons and Fill in; the code never reaches the
// agent, a draft or the card once sent.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/dictation.dart' show Dictation;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'june',
);

Entry _card({
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  bool numeric = false,
  String answer = '',
  String actionError = '',
}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    // The agent's words: never the card's heading.
    title: 'Enter the code Google sent',
    body: 'Google wants a code.',
    state: state,
    revision: 1,
    code: true,
    answer: answer,
    actionError: actionError,
    decidedMs: Int64(state == AskPersonState.ASK_PERSON_STATE_PENDING ? 0 : 9),
    handoff: ErrandHandoff(
      errandId: 'e1',
      kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_CODE,
      origin: 'https://accounts.google.com',
      seq: Int64(4),
      numeric: numeric,
    ),
  ),
);

Future<Harness> _show(
  WidgetTester tester,
  Entry card, {
  bool phone = false,
}) async {
  if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(_workspace());
  harness.backend.transcript.load(Transcript(agentId: 'june', entries: [card]));
  await harness.show(
    tester,
    size: phone ? const Size(393, 852) : const Size(1200, 900),
  );
  await settle(tester);
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

Finder get _code => _inCard(find.byType(EditableText));

void main() {
  testWidgets("Claktory's heading with the site, never the agent's words; "
      'one field for the code, no comment, mic or files', (tester) async {
    await _show(tester, _card());
    expect(find.text('June needs a code'), findsOneWidget);
    expect(
      find.text('accounts.google.com asks for a code', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('Enter the code Google sent'), findsNothing);
    expect(find.textContaining('Google wants a code'), findsNothing);
    expect(
      find.text(
        'Sent to your phone or email. Claktory types it into that page '
        'once, and June never sees it.',
      ),
      findsOneWidget,
    );
    expect(_code, findsOneWidget);
    expect(_inCard(find.byType(DecisionComment)), findsNothing);
    expect(_inCard(find.byType(Dictation)), findsNothing);
    expect(
      find.text('Claktory fills it in on accounts.google.com.'),
      findsOneWidget,
    );
    // Off until typed; on a Mac the field waits for a click (rule 4).
    expect(
      tester.widget<Button>(find.widgetWithText(Button, 'Fill in')).onPressed,
      isNull,
    );
    expect(
      FocusManager.instance.primaryFocus?.context?.widget,
      isNot(isA<EditableText>()),
    );
    final field = tester.widget<EditableText>(_code);
    expect(field.autocorrect, isFalse);
    expect(field.enableSuggestions, isFalse);
    expect(field.smartDashesType, SmartDashesType.disabled);
    expect(field.smartQuotesType, SmartQuotesType.disabled);
    expect(field.autofillHints, [AutofillHints.oneTimeCode]);
    expect(field.keyboardType, TextInputType.visiblePassword);
  });

  testWidgets('spaces and dashes pasted with it go, at most 16; Return '
      'fills it in, and the field empties', (tester) async {
    final harness = await _show(tester, _card());
    await tester.showKeyboard(_code);
    tester.testTextInput.updateEditingValue(
      const TextEditingValue(text: '482 9-13'),
    );
    await tester.pump();
    expect(tester.widget<EditableText>(_code).controller.text, '482913');
    tester.testTextInput.updateEditingValue(
      const TextEditingValue(text: 'ABCDEFGHIJKLMNOPQRST'),
    );
    await tester.pump();
    expect(
      tester.widget<EditableText>(_code).controller.text,
      'ABCDEFGHIJKLMNOP',
    );
    await tester.enterText(_code, '482913');
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(
      (answer.agentId, answer.seq.toInt(), answer.text, answer.dismiss),
      ('june', 4, '482913', false),
    );
    expect(tester.widget<EditableText>(_code).controller.text, isEmpty);
  });

  testWidgets("the on-screen keyboard's Done fills it in, never while the "
      'field is empty (Prosciutto 44920)', (tester) async {
    final harness = await _show(tester, _card());
    await tester.showKeyboard(_code);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await tester.showKeyboard(_code);
    await tester.enterText(_code, '482913');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    expect(harness.last.answerAsk.text, '482913');
  });

  testWidgets('Fill in sends the code; set aside after a change, the card '
      'never shows it', (tester) async {
    final harness = await _show(tester, _card());
    await tester.enterText(_code, '777111');
    await tester.pump();
    await tester.tap(find.text('Fill in'));
    await settle(tester);
    expect(harness.last.answerAsk.text, '777111');
    expect(find.textContaining('777111'), findsNothing);
  });

  testWidgets("Can't do this… offers reasons; a reason ends it with that", (
    tester,
  ) async {
    final harness = await _show(tester, _card());
    await tester.tap(find.text("Can't do this…"));
    await settle(tester);
    expect(find.text('Not now'), findsOneWidget);
    expect(find.text('In my own words…'), findsOneWidget);
    await tester.tap(find.text('No code arrived'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect((answer.dismiss, answer.text), (true, 'No code arrived'));
  });

  testWidgets("In my own words: the card asks what's in the way, and Can't "
      'do this sends the words; Back returns to the code', (tester) async {
    final harness = await _show(tester, _card());
    await tester.tap(find.text("Can't do this…"));
    await settle(tester);
    await tester.tap(find.text('In my own words…'));
    await settle(tester);
    expect(find.text("What's in the way? June sees this."), findsOneWidget);
    expect(
      find.text('Nothing is filled in. June gets your words.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Back'));
    await settle(tester);
    expect(find.text('Fill in'), findsOneWidget);
    await tester.tap(find.text("Can't do this…"));
    await settle(tester);
    await tester.tap(find.text('In my own words…'));
    await settle(tester);
    await tester.enterText(
      _inCard(
        find.descendant(
          of: find.byType(DecisionComment),
          matching: find.byType(EditableText),
        ),
      ),
      'The phone is at home',
    );
    await tester.pump();
    await tester.tap(find.text("Can't do this"));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect((answer.dismiss, answer.text), (true, 'The phone is at home'));
  });

  testWidgets('answered: Filled in on the site, never the code; the page '
      "changed: said in danger; can't: with the reason", (tester) async {
    await _show(tester, _card(state: AskPersonState.ASK_PERSON_STATE_ANSWERED));
    expect(find.text('June needed a code'), findsOneWidget);
    expect(
      find.textContaining(
        'Filled in on accounts.google.com',
        findRichText: true,
      ),
      findsOneWidget,
    );
    expect(_code, findsNothing);
  });

  testWidgets('the page changed: the card ends saying so', (tester) async {
    await _show(
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        actionError: 'The field is gone.',
      ),
    );
    expect(
      find.textContaining(codePageChanged, findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('The field is gone.'), findsNothing);
  });

  testWidgets("couldn't: with the reason", (tester) async {
    await _show(
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_DISMISSED,
        answer: 'No code arrived',
      ),
    );
    expect(
      find.textContaining("Couldn't fill it in", findRichText: true),
      findsOneWidget,
    );
    expect(find.text('No code arrived'), findsOneWidget);
  });

  testWidgets('on an iPhone it takes the cursor as it opens, the number pad '
      "when the page's field is numeric", (tester) async {
    await _show(tester, _card(numeric: true), phone: true);
    final field = tester.widget<EditableText>(_code);
    expect(field.focusNode.hasFocus, isTrue);
    expect(field.keyboardType, TextInputType.number);
    expect(field.textCapitalization, TextCapitalization.characters);
  });
}
