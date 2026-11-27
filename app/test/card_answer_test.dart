// A card's answer as data (#514): one draft per card while the app runs,
// and a dock that takes what a card publishes after the frame, only when it
// changed.
import 'package:factory_app/app_grant_card.dart';
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/card_answer.dart';
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/keep_awake_card.dart';
import 'package:factory_app/pairing_card.dart';
import 'package:factory_app/pipe_start.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/secrets.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// [card] in a dock, as focus mode's stage shows it, for item [itemKey].
Future<ValueNotifier<CardAnswer?>> _docked(
  WidgetTester tester,
  Harness harness,
  Widget card, {
  String? itemKey,
}) async {
  final answer = ValueNotifier<CardAnswer?>(null);
  addTearDown(answer.dispose);
  await harness.show(
    tester,
    home: AnswerDock(
      answer: answer,
      itemKey: itemKey,
      child: SingleChildScrollView(child: card),
    ),
  );
  await tester.pump();
  return answer;
}

void main() {
  testWidgets("a card's draft is the same field wherever it shows, and "
      'cleared once sent', (tester) async {
    final harness = Harness(Workspace());
    final drafts = CardDrafts.of(harness.backend);
    expect(CardDrafts.of(harness.backend), same(drafts));
    final key = cardKey('kai', 'q7');
    expect(key, 'ask/kai/q7');
    final field = drafts.field(key)..text = 'Not yet';
    expect(drafts.field(key), same(field));
    expect(drafts.field(cardKey('kai', 'q8')).text, isEmpty);
    drafts.clear(key);
    expect(field.text, isEmpty);
  });

  testWidgets('the dock takes an answer after the frame, only when it '
      'changed, and null as the card goes', (tester) async {
    final answer = ValueNotifier<CardAnswer?>(null);
    addTearDown(answer.dispose);
    var heard = 0;
    answer.addListener(() => heard++);
    late AnswerDock dock;
    await tester.pumpWidget(
      AnswerDock(
        answer: answer,
        child: Builder(
          builder: (context) {
            dock = AnswerDock.of(context)!;
            return const SizedBox();
          },
        ),
      ),
    );
    CardAnswer card({bool busy = false}) => CardAnswer(
      key: 'ask/kai/q7',
      docked: true,
      yes: 'Approve',
      no: 'Dismiss',
      busy: busy,
      onYes: busy ? null : () async {},
    );
    AnswerDock.publish(dock, card());
    // Never mid-build: after the frame.
    expect(answer.value, isNull);
    await tester.pump();
    expect(answer.value?.yes, 'Approve');
    expect(heard, 1);
    // The same again: nothing for the dock to redraw.
    AnswerDock.publish(dock, card());
    await tester.pump();
    expect(heard, 1);
    AnswerDock.publish(dock, card(busy: true));
    await tester.pump();
    expect(answer.value?.onYes, isNull);
    expect(heard, 2);
    AnswerDock.publish(dock, null);
    await tester.pump();
    expect(answer.value, isNull);
  });

  testWidgets("keep awake's answer: its two buttons, stamped with the "
      "dock's item, and a refusal thrown", (tester) async {
    final harness = Harness(Workspace());
    final answer = await _docked(
      tester,
      harness,
      const KeepAwakeCard(),
      itemKey: 'keepAwake',
    );
    expect(answer.value?.key, 'keepAwake');
    expect(answer.value?.docked, isFalse);
    expect(answer.value?.yes, 'Keep it awake');
    expect(answer.value?.no, 'Let it sleep');
    await answer.value!.onYes!();
    expect(harness.last.setKeepAwake.on, isTrue);
    await tester.pump();
    harness.backend.refuse = "Can't keep this Mac awake.";
    await expectLater(
      answer.value!.onNo!(null),
      throwsA(isA<RequestFailure>()),
    );
    await tester.pump();
    expect(answer.value?.error, "Can't keep this Mac awake.");
  });

  testWidgets('a card going takes its answer out of the dock', (tester) async {
    final harness = Harness(Workspace());
    final shown = ValueNotifier(true);
    addTearDown(shown.dispose);
    final answer = await _docked(
      tester,
      harness,
      ValueListenableBuilder(
        valueListenable: shown,
        builder: (context, on, _) =>
            on ? const KeepAwakeCard() : const SizedBox(),
      ),
    );
    expect(answer.value?.key, 'keepAwake');
    shown.value = false;
    await tester.pump();
    await tester.pump();
    expect(answer.value, isNull);
  });

  group("an app grant's answer", () {
    final grant = AppGrantProposal(
      id: 'g1',
      appId: 'dms',
      appName: 'Direct messages',
      agentId: 'mike',
      scopes: [AppGrantScope(id: 'dm.read', label: 'Read messages')],
      reason: 'Mike answers while you travel.',
      state: AppGrantState.APP_GRANT_STATE_PENDING,
    );
    Harness harness() => Harness(
      Workspace(
        agents: [
          Agent(
            id: 'rivet',
            name: 'Rivet',
            role: 'Admin',
            modelAvailable: true,
          ),
          Agent(id: 'mike', name: 'Mike', modelAvailable: true),
        ],
      ),
    );

    testWidgets("its no sends the reason chosen with what's typed", (
      tester,
    ) async {
      final h = harness();
      final answer = await _docked(
        tester,
        h,
        AppGrantCard(grant, agentId: 'rivet', seq: 12),
      );
      expect(answer.value?.key, 'agent:rivet/12');
      expect((answer.value?.yes, answer.value?.no), ('Approve', 'Decline'));
      expect(answer.value?.typed, isFalse);
      await answer.value!.onNo!('Not now');
      final decide = h.last.decideAppGrant;
      expect(decide.approve, isFalse);
      expect(decide.comment, 'Not now');
      expect(decide.seq.toInt(), 12);
    });

    testWidgets("typed, it's Submit: a reply", (tester) async {
      final h = harness();
      final answer = await _docked(
        tester,
        h,
        AppGrantCard(grant, agentId: 'rivet', seq: 12),
      );
      await tester.enterText(find.byType(EditableText).last, 'Only Ana');
      await tester.pump();
      await tester.pump();
      expect(answer.value?.typed, isTrue);
      await answer.value!.onSubmit!();
      expect(h.last.decideAppGrant.reply, isTrue);
      expect(h.last.decideAppGrant.comment, 'Only Ana');
    });
  });

  group("a pipe invite's answer", () {
    Harness harness() => Harness(
      Workspace(
        projects: [Project(id: 'p2', name: 'Website')],
        agents: [
          Agent(
            id: 'rivet',
            name: 'Rivet',
            role: 'Admin',
            modelAvailable: true,
          ),
        ],
      ),
    );
    final start = PipeStart(
      id: 'start1',
      agentId: 'rivet',
      kind: PipeStartKind.PIPE_START_KIND_INVITE,
      projectId: 'p2',
      name: 'Launch with Acme',
      purpose: 'Launch dates',
      state: PipeStartState.PIPE_START_STATE_WAITING,
    );

    testWidgets('Make invite keeps the dock on the card, for its code', (
      tester,
    ) async {
      final h = harness();
      final answer = await _docked(tester, h, PipeStartCard(start));
      expect(answer.value?.yes, 'Make invite');
      expect(answer.value?.keep, isFalse);
      final made = answer.value!.onYes!();
      await tester.pump();
      await tester.pump();
      // Kept from the moment it's asked for, before the card stops waiting.
      expect(answer.value?.keep, isTrue);
      await made;
      await tester.pump();
      await tester.pump();
      expect(h.last.hasCreatePipeInvite(), isTrue);
      expect(answer.value?.keep, isTrue);
      expect(answer.value?.onYes, isNull);
    });

    testWidgets('Dismiss moves on, with the reason', (tester) async {
      final h = harness();
      final answer = await _docked(tester, h, PipeStartCard(start));
      await answer.value!.onNo!('Do it yourself');
      expect(h.last.dismissPipeStart.comment, 'Do it yourself');
      await tester.pump();
      expect(answer.value?.keep, isFalse);
    });
  });

  testWidgets("a device's Allow: only with its number typed", (tester) async {
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
        remote: RemoteAccess(
          pendingDevices: [
            PendingDevice(deviceId: 'd1', name: 'iPad', number: '0427'),
          ],
        ),
      ),
    );
    final answer = await _docked(
      tester,
      harness,
      DevicePairingCard(
        DevicePairing(
          id: 'p1',
          agentId: 'rivet',
          deviceId: 'd1',
          name: 'iPad',
          state: DevicePairingState.DEVICE_PAIRING_STATE_ASKING,
        ),
      ),
    );
    expect((answer.value?.yes, answer.value?.no), ('Allow', "Don't allow"));
    expect(answer.value?.onYes, isNull);
    await tester.enterText(find.byType(EditableText), '0427');
    await tester.pump();
    await tester.pump();
    expect(answer.value?.onYes, isNotNull);
    await answer.value!.onYes!();
    expect(harness.last.approveDevice.number, '0427');
  });

  testWidgets("a secret request's secrets are its choices, numbered in "
      'place; choosing sends nothing, and yes gives the one chosen', (
    tester,
  ) async {
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
        vault: VaultState(
          exists: true,
          unlocked: true,
          secrets: [
            SecretInfo(
              id: 'cf',
              name: 'CF_DNS',
              variables: [SecretVariable(name: 'CLOUDFLARE_API_TOKEN')],
            ),
            SecretInfo(
              id: 'gh',
              name: 'GITHUB',
              variables: [SecretVariable(name: 'GITHUB_TOKEN')],
            ),
          ],
        ),
      ),
    );
    final answer = await _docked(
      tester,
      harness,
      SecretRequestCard(
        SecretRequest(
          id: 'r2',
          agentId: 'june',
          newSecret: NewSecret(
            name: 'Claktory DNS',
            variables: [SecretVariable(name: 'CLOUDFLARE_API_TOKEN')],
          ),
          state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
          fittingSecretId: 'cf',
        ),
        shownIn: 'june',
      ),
    );
    expect(answer.value?.yes, 'Give CF_DNS to June');
    expect(
      [for (final c in answer.value!.choices) (c.label, c.chosen)],
      [('CF_DNS', true), ('GITHUB', false)],
    );
    // The list in place, numbered for 1 to 9; no Give another… sheet.
    expect(find.text('Or give one of yours'), findsOneWidget);
    expect(find.text('Give another…'), findsNothing);
    answer.value!.choices[1].choose();
    await tester.pump();
    await tester.pump();
    expect(
      harness.backend.sent.where((m) => m.hasDecideSecretRequest()),
      isEmpty,
    );
    expect(answer.value!.choices[1].chosen, isTrue);
    await answer.value!.onYes!();
    expect(harness.last.decideSecretRequest.secretId, 'gh');
  });

  group("a question's answer", () {
    Harness harness() => Harness(
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
      ),
    );
    AskPerson approval() => AskPerson(
      id: 'q7',
      title: 'Ship it?',
      state: AskPersonState.ASK_PERSON_STATE_PENDING,
      revision: 1,
    );

    testWidgets("in a dock, its field, yes and no are the dock's, and what's "
        "typed is the card's draft", (tester) async {
      final h = harness();
      final answer = await _docked(
        tester,
        h,
        AskPersonCard(approval(), agentId: 'june', seq: 7),
        itemKey: 'agent:june/7',
      );
      final now = answer.value!;
      expect(now.docked, isTrue);
      expect((now.yes, now.no), ('Approve', 'Dismiss'));
      expect(
        now.field,
        same(CardDrafts.of(h.backend).field(cardKey('june', 'q7'))),
      );
      expect(now.filesKey, cardKey('june', 'q7'));
      // The card draws neither.
      expect(find.text('Approve'), findsNothing);
      expect(find.byType(DecisionComment), findsNothing);
      now.field!.text = 'Only the docs';
      await tester.pump();
      await tester.pump();
      expect(answer.value?.typed, isTrue);
      await answer.value!.onSubmit!();
      expect(h.last.answerAsk.reply, isTrue);
      expect(h.last.answerAsk.text, 'Only the docs');
      expect(now.field!.text, isEmpty);
    });

    testWidgets("its no sends the reason chosen", (tester) async {
      final h = harness();
      final answer = await _docked(
        tester,
        h,
        AskPersonCard(approval(), agentId: 'june', seq: 7),
      );
      await answer.value!.onNo!('Not now');
      expect(h.last.answerAsk.dismiss, isTrue);
      expect(h.last.answerAsk.text, 'Not now');
    });

    testWidgets("what's typed on the card outside a dock is the same draft", (
      tester,
    ) async {
      final h = harness();
      await h.show(
        tester,
        home: SingleChildScrollView(
          child: AskPersonCard(approval(), agentId: 'june', seq: 7),
        ),
      );
      await settle(tester);
      await tester.enterText(find.byType(EditableText).last, 'Wait for Ana');
      await settle(tester);
      expect(
        CardDrafts.of(h.backend).field(cardKey('june', 'q7')).text,
        'Wait for Ana',
      );
    });

    testWidgets("Execute's yes takes two presses from a key, and its warnings "
        "ask once more", (tester) async {
      final h = harness();
      final answer = await _docked(
        tester,
        h,
        AskPersonCard(
          AskPerson(
            id: 'q8',
            title: 'Clean the build',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
            command: AskCommand(
              command: 'sudo rm -rf build',
              warnings: [
                CommandWarning(
                  kind: CommandWarningKind.COMMAND_WARNING_KIND_ADMIN,
                  text: 'Runs as administrator (sudo).',
                ),
              ],
            ),
          ),
          agentId: 'june',
          seq: 8,
        ),
      );
      expect(answer.value?.pressTwice, isTrue);
      expect(answer.value?.docked, isFalse);
      expect(answer.value?.yes, 'Execute');
      await expectLater(answer.value!.onYes!(), throwsA(isA<AnswerNotSent>()));
      expect(h.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
      await tester.pump();
      await tester.pump();
      expect(answer.value?.yes, 'Run anyway');
      await answer.value!.onYes!();
      expect(h.last.answerAsk.confirmed, isTrue);
    });
  });
}
