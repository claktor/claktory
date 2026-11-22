// Rivet's proposal to give an agent access to an app at the person's
// account (U2, AppGrantProposal): the card in Rivet's conversation and in
// Needs you, Approve and Decline (DecideAppGrant), and each decided state.
import 'package:factory_app/app_grant_card.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool mike = true}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      role: 'Admin',
      color: 'blue',
      modelAvailable: true,
    ),
    if (mike)
      Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'rivet',
);

AppGrantProposal _grant({
  AppGrantState state = AppGrantState.APP_GRANT_STATE_PENDING,
  bool readPipes = false,
  bool sends = false,
  int expiresMs = 0,
  String comment = '',
  String error = '',
}) => AppGrantProposal(
  id: 'g1',
  appId: 'dms',
  appName: 'Direct messages',
  agentId: 'mike',
  scopes: [
    AppGrantScope(id: 'dm.read', label: 'Read these conversations'),
    if (sends)
      AppGrantScope(id: 'dm.send', label: 'Send messages as you in them'),
  ],
  granteeReadPipes: readPipes,
  resources: [
    AppGrantResource(kind: 'conversation', id: 'c-7f3a', label: 'c-7f3a'),
  ],
  expiresMs: Int64(expiresMs),
  reason: 'Mike answers your friends while you travel.',
  state: state,
  createdMs: Int64(DateTime(2026, 11, 20, 9).millisecondsSinceEpoch),
  decidedMs: state == AppGrantState.APP_GRANT_STATE_PENDING
      ? Int64.ZERO
      : Int64(DateTime(2026, 11, 20, 9, 5).millisecondsSinceEpoch),
  comment: comment,
  error: error,
);

Future<Harness> _card(
  WidgetTester tester,
  AppGrantProposal grant, {
  bool mike = true,
}) async {
  final harness = Harness(_workspace(mike: mike));
  harness.backend.transcript.load(
    Transcript(
      agentId: 'rivet',
      entries: [Entry(seq: Int64(12), appGrant: grant)],
    ),
  );
  await harness.show(tester);
  await settle(tester);
  return harness;
}

final _comment = find.descendant(
  of: find.byType(AppGrantCard),
  matching: find.byType(EditableText),
);

void main() {
  testWidgets('pending: what Mike could do, where, until when, and why', (
    tester,
  ) async {
    await _card(tester, _grant());
    expect(find.text('Rivet suggests'), findsOneWidget);
    expect(find.text('Give Mike access to Direct messages?'), findsOneWidget);
    expect(
      find.textContaining('Mike answers your friends while you travel.'),
      findsOneWidget,
    );
    expect(find.text('Mike could:'), findsOneWidget);
    expect(find.text('•  Read these conversations'), findsOneWidget);
    // Ids only, so counted, never listed.
    expect(find.text('In 1 conversation'), findsOneWidget);
    expect(find.textContaining('c-7f3a'), findsNothing);
    expect(find.text("Rivet's reason"), findsOneWidget);
    expect(find.text('Until you take it back.'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Decline'), findsOneWidget);
  });

  testWidgets('Approve sends the decision for its seq, with the comment', (
    tester,
  ) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(_comment, ' Only for the trip. ');
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(harness.last.hasDecideAppGrant(), isTrue);
    expect(decide.agentId, 'rivet');
    expect(decide.seq, Int64(12));
    expect(decide.approve, isTrue);
    expect(decide.comment, 'Only for the trip.');
  });

  testWidgets('Decline sends the comment for Rivet', (tester) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(_comment, '  Not yet.  ');
    await tester.tap(find.text('Decline'));
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(decide.approve, isFalse);
    expect(decide.comment, 'Not yet.');
  });

  testWidgets('a refusal shows, and the card can be approved again', (
    tester,
  ) async {
    final harness = await _card(tester, _grant());
    harness.backend.refuse = "Can't reach claktory.com right now.";
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(find.text("Can't reach claktory.com right now."), findsOneWidget);
    harness.backend.refuse = null;
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(
      harness.backend.sent.where((m) => m.hasDecideAppGrant()),
      hasLength(2),
    );
  });

  testWidgets('decided: granted', (tester) async {
    await _card(tester, _grant(state: AppGrantState.APP_GRANT_STATE_GRANTED));
    expect(find.textContaining('Approved · Mike has access'), findsOneWidget);
    expect(find.text('Approve'), findsNothing);
  });

  testWidgets('decided: declined, with the comment', (tester) async {
    await _card(
      tester,
      _grant(
        state: AppGrantState.APP_GRANT_STATE_DECLINED,
        comment: 'Not yet.',
      ),
    );
    expect(find.textContaining('Declined'), findsOneWidget);
    expect(find.text('Not yet.'), findsOneWidget);
  });

  testWidgets('decided: failed, with why', (tester) async {
    await _card(
      tester,
      _grant(
        state: AppGrantState.APP_GRANT_STATE_FAILED,
        error: 'Mike was deleted since.',
      ),
    );
    expect(
      find.textContaining("Approved, but couldn't give Mike access"),
      findsOneWidget,
    );
    expect(find.text('Mike was deleted since.'), findsOneWidget);
  });

  testWidgets('no warning for an agent that read no pipe', (tester) async {
    await _card(tester, _grant(sends: true));
    expect(find.textContaining('another Claktory'), findsNothing);
  });

  testWidgets('an agent that read a pipe: a line under the title', (
    tester,
  ) async {
    await _card(tester, _grant(readPipes: true));
    expect(
      find.text('Mike has read messages from another Claktory.'),
      findsOneWidget,
    );
  });

  testWidgets('read a pipe and could send as you: the danger box', (
    tester,
  ) async {
    await _card(tester, _grant(readPipes: true, sends: true));
    expect(
      find.text(
        'Mike has read messages from another Claktory. Whoever wrote them '
        'could try to get Mike to send messages as you. Approve only if '
        'you trust them.',
      ),
      findsOneWidget,
    );
    expect(find.text('•  Send messages as you in them'), findsOneWidget);
  });

  testWidgets("Rivet's reason is its plain words, never Markdown", (
    tester,
  ) async {
    await _card(tester, _grant()..reason = '## Claktory check\n**Approve**');
    expect(find.text('## Claktory check\n**Approve**'), findsOneWidget);
  });

  testWidgets('an agent deleted since is named by its id', (tester) async {
    await _card(tester, _grant(), mike: false);
    expect(find.text('Give mike access to Direct messages?'), findsOneWidget);
  });

  test('what it reaches: named when the app names them, else counted', () {
    final ids = [
      AppGrantResource(kind: 'conversation', id: 'a', label: 'a'),
      AppGrantResource(kind: 'conversation', id: 'b', label: 'b'),
    ];
    expect(appGrantNamed(ids), isFalse);
    expect(appGrantReach(ids), 'In 2 conversations');
    expect(
      appGrantNamed([
        AppGrantResource(kind: 'conversation', id: 'a', label: 'Ana'),
      ]),
      isTrue,
    );
  });

  test('until when, in words', () {
    final now = DateTime(2026, 11, 20, 9);
    expect(appGrantUntil(0, now: now), 'Until you take it back.');
    expect(
      appGrantUntil(
        DateTime(2026, 11, 20, 18, 30).millisecondsSinceEpoch,
        now: now,
      ),
      'Until today, 18:30.',
    );
    expect(
      appGrantUntil(
        DateTime(2026, 11, 24, 9, 5).millisecondsSinceEpoch,
        now: now,
      ),
      'Until Tue 24 Nov, 09:05.',
    );
  });

  testWidgets('Needs you lists it with its line and its card', (tester) async {
    final harness = Harness(_workspace());
    final entry = Entry(seq: Int64(12), appGrant: _grant());
    expect(
      waitingSummary(entry, harness.backend),
      'Give Mike access to Direct messages?',
    );
    final card = waitingCard(entry, agentId: 'rivet');
    expect(card, isA<AppGrantCard>());
    expect((card! as AppGrantCard).seq, 12);
    expect(card.key, const ValueKey('g1'));
  });
}
