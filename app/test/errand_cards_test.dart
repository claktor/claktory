// An errand's other cards (#435 W8b, Lucy's needs-you-cards.md §3.6; Fugo
// 40593, 40694; Ghiaccio 43832): the agreement card (the site's words
// fenced as theirs, Accept or words as a reply), the two-step card (Approve
// or Can't do this, no reply) and the recovery-codes card (the codes asked
// for on screen only, Copy, I saved them or Keep in Secrets, nothing else).
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
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

Entry _card(
  ErrandHandoffKind kind, {
  required String title,
  String body = '',
  String agreement = '',
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  String answer = '',
  String origin = 'https://play.google.com',
}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: title,
    body: body,
    state: state,
    revision: 1,
    answer: answer,
    decidedMs: Int64(state == AskPersonState.ASK_PERSON_STATE_PENDING ? 0 : 9),
    handoff: ErrandHandoff(
      errandId: 'e1',
      kind: kind,
      origin: origin,
      seq: Int64(4),
      agreement: agreement,
    ),
  ),
);

Entry _agreement() => _card(
  ErrandHandoffKind.ERRAND_HANDOFF_KIND_AGREEMENT,
  title: 'play.google.com asks you to agree',
  agreement:
      'I agree to the Google Play Developer Distribution Agreement and the '
      'Developer Program Policies.',
);

Entry _twoStep() => _card(
  ErrandHandoffKind.ERRAND_HANDOFF_KIND_TWO_STEP,
  title: 'github.com wants two-step sign-in turned on',
  origin: 'https://github.com',
);

Entry _codes({
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  String answer = '',
}) => _card(
  ErrandHandoffKind.ERRAND_HANDOFF_KIND_RECOVERY_CODES,
  title: 'github.com recovery codes',
  body: 'Claktory keeps them, sealed in its vault, until you choose.',
  origin: 'https://github.com',
  state: state,
  answer: answer,
);

Future<Harness> _show(
  WidgetTester tester,
  Entry card, {
  void Function(Harness)? before,
}) async {
  final harness = Harness(_workspace());
  before?.call(harness);
  harness.backend.transcript.load(Transcript(agentId: 'june', entries: [card]));
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  await settle(tester);
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

Finder get _field => _inCard(
  find.descendant(
    of: find.byType(DecisionComment),
    matching: find.byType(EditableText),
  ),
);

void main() {
  group('agreement', () {
    testWidgets("Claktory's heading, the site's words fenced as theirs; "
        'Accept ticks the box with no words', (tester) async {
      final harness = await _show(tester, _agreement());
      expect(find.text("June's errand needs your OK"), findsOneWidget);
      expect(
        find.text('play.google.com asks you to agree', findRichText: true),
        findsOneWidget,
      );
      expect(find.text("The site's words"), findsOneWidget);
      expect(
        find.textContaining('Developer Distribution Agreement'),
        findsOneWidget,
      );
      expect(
        find.text('Claktory checks only this box on play.google.com.'),
        findsOneWidget,
      );
      expect(find.text("Can't do this"), findsOneWidget);
      await tester.tap(find.text('Accept'));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect((answer.dismiss, answer.reply, answer.text), (false, false, ''));
    });

    testWidgets('words typed: Submit, a reply that ticks nothing', (
      tester,
    ) async {
      final harness = await _show(tester, _agreement());
      await tester.enterText(_field, 'Which agreement is this?');
      await tester.pump();
      expect(find.text('Accept'), findsNothing);
      expect(
        find.text('Submit sends your words instead of accepting.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Submit'));
      await settle(tester);
      expect(harness.last.answerAsk.reply, isTrue);
      expect(harness.last.answerAsk.text, 'Which agreement is this?');
    });

    testWidgets('accepted, it says so; the page changed, nothing ticked', (
      tester,
    ) async {
      await _show(
        tester,
        _agreement()
          ..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED
          ..askPerson.decidedMs = Int64(9),
      );
      expect(find.text("June's errand needed your OK"), findsOneWidget);
      expect(
        find.textContaining('Accepted', findRichText: true),
        findsOneWidget,
      );
    });
  });

  group('two-step', () {
    testWidgets('Approve or Can\'t do this; words go only with Can\'t do '
        'this, never as a reply', (tester) async {
      final harness = await _show(tester, _twoStep());
      expect(
        find.text(
          'github.com wants two-step sign-in turned on',
          findRichText: true,
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'June may turn it on, with codes by text message or email only.',
        ),
        findsOneWidget,
      );
      expect(
        find.text("Why not? June sees this only with Can't do this."),
        findsOneWidget,
      );
      await tester.enterText(_field, 'Not on this account');
      await tester.pump();
      // The buttons stay: no Submit.
      expect(find.text('Submit'), findsNothing);
      expect(find.text('Approve'), findsOneWidget);
      expect(
        find.text("Your words go only with Can't do this."),
        findsOneWidget,
      );
      // Return sends nothing.
      final before = harness.backend.sent.length;
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await settle(tester);
      expect(harness.backend.sent.length, before);
      await tester.tap(find.text("Can't do this"));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect(
        (answer.dismiss, answer.reply, answer.text),
        (true, false, 'Not on this account'),
      );
    });

    testWidgets('Approve lets the agent turn it on', (tester) async {
      final harness = await _show(tester, _twoStep());
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(harness.last.answerAsk.dismiss, isFalse);
    });
  });

  group('recovery codes', () {
    testWidgets('the codes are asked for as it shows, two to a row; no '
        'field, Dismiss or reply; I saved them answers', (tester) async {
      final harness = await _show(
        tester,
        _codes(),
        before: (harness) => harness.backend.recoveryCodes = [
          'a8f2-91kd',
          'c3m9-2wle',
          'k1vv-o3ds',
          '7bq1-x0pe',
        ],
      );
      expect(
        find.text("June's errand saved recovery codes"),
        findsOneWidget,
      );
      expect(
        find.text('github.com recovery codes', findRichText: true),
        findsOneWidget,
      );
      final asked = harness.backend.sent
          .where((m) => m.hasShowRecoveryCodes())
          .single
          .showRecoveryCodes;
      expect((asked.agentId, asked.seq.toInt()), ('june', 4));
      for (final code in ['a8f2-91kd', 'c3m9-2wle', 'k1vv-o3ds', '7bq1-x0pe']) {
        expect(find.text(code), findsOneWidget);
      }
      expect(
        tester.getTopLeft(find.text('k1vv-o3ds')).dy,
        tester.getTopLeft(find.text('a8f2-91kd')).dy,
        reason: 'two to a row',
      );
      expect(_inCard(find.byType(DecisionComment)), findsNothing);
      expect(find.text('Dismiss'), findsNothing);
      expect(
        find.text(
          "They're deleted when this card closes. Save them somewhere "
          'safe, or keep them in Secrets.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('I saved them'));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect((answer.dismiss, answer.text), (false, ''));
    });

    testWidgets('Keep in Secrets asks Claktory to keep them; refused, the '
        'card says why and still waits', (tester) async {
      final harness = await _show(
        tester,
        _codes(),
        before: (harness) => harness.backend.recoveryCodes = ['a8f2-91kd'],
      );
      harness.backend.refuse = "Claktory's vault is locked.";
      await tester.tap(find.text('Keep in Secrets'));
      await settle(tester);
      expect(harness.last.hasKeepRecoveryCodes(), isTrue);
      expect(find.text("Claktory's vault is locked."), findsOneWidget);
      expect(
        tester
            .widget<Button>(find.widgetWithText(Button, 'Keep in Secrets'))
            .onPressed,
        isNotNull,
      );
    });

    testWidgets('gone: the reason, and Show asks again', (tester) async {
      final harness = await _show(
        tester,
        _codes(),
        before: (harness) =>
            harness.backend.recoveryCodesGone = 'These codes are gone.',
      );
      expect(find.text('These codes are gone.'), findsOneWidget);
      harness.backend
        ..recoveryCodesGone = null
        ..recoveryCodes = ['a8f2-91kd'];
      await tester.tap(find.text('Show'));
      await settle(tester);
      expect(find.text('a8f2-91kd'), findsOneWidget);
    });

    testWidgets('ended: You saved them, or kept in Secrets; never the codes', (
      tester,
    ) async {
      final harness = await _show(
        tester,
        _codes(state: AskPersonState.ASK_PERSON_STATE_ANSWERED),
      );
      expect(
        find.textContaining('You saved them', findRichText: true),
        findsOneWidget,
      );
      expect(
        harness.backend.sent.where((m) => m.hasShowRecoveryCodes()),
        isEmpty,
      );
    });

    testWidgets('kept: which secret holds them', (tester) async {
      const kept =
          'Kept in Secrets as github.com recovery codes 2, given to no agent.';
      await _show(
        tester,
        _codes(state: AskPersonState.ASK_PERSON_STATE_ANSWERED, answer: kept),
      );
      expect(
        find.textContaining('Kept in Secrets', findRichText: true),
        findsWidgets,
      );
      expect(find.text(kept), findsOneWidget);
    });

    testWidgets("Claktory's state leads, a locked vault's in danger; the "
        "agent's words as its own", (tester) async {
      const locked =
          "Claktory's vault is locked, so it keeps them only until it "
          'restarts: save them now.';
      await _show(
        tester,
        _codes()
          ..askPerson.body =
              '$locked\n\nSaved the codes GitHub showed.\n\n'
              'Press I saved them once you have.'
          ..askPerson.handoff.what = 'Saved the codes GitHub showed.',
        before: (harness) => harness.backend.recoveryCodes = ['a8f2-91kd'],
      );
      final state = tester.widget<Text>(find.text(locked));
      expect(
        state.style!.color,
        Palette.of(tester.element(find.text(locked))).danger,
      );
      expect(
        find.text('June: “Saved the codes GitHub showed.”'),
        findsOneWidget,
      );
      expect(find.textContaining('Press I saved them'), findsNothing);
    });
  });
}
