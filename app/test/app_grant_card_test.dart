// Rivet's proposal to give an agent access to an app at the person's
// account (U2, AppGrantProposal): the card in Rivet's conversation and in
// Needs you, Approve and Decline (DecideAppGrant), and each decided state.
import 'package:factory_app/app_grant_card.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/people/contacts.dart';
import 'package:factory_app/people/people_view.dart' show contactsApi;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
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
  List<String> conversations = const ['c-7f3a'],
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
    for (final id in conversations)
      AppGrantResource(kind: 'conversation', id: id, label: id),
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

/// People's friends on this device: Ana, whose conversation is c-7f3a;
/// a failure when [friends] is null.
void _friends(List<Map<String, Object?>>? friends) {
  contactsApi = (_) => ContactsApi(
    UserAppsSession(
      DeviceAccount(key: 'ab' * 32, email: 'matt@example.com'),
      nonce: () => 'n',
      call: (method, path, [body]) async {
        if (friends == null) {
          throw const AccountServiceUnreachable('offline');
        }
        return path.startsWith('/v1/apps/dms/contacts')
            ? {'contacts': friends}
            : <String, Object?>{};
      },
    ),
  );
}

const _ana = {
  'id': 'f1',
  'name': 'Ana',
  'avatar_seed': 'aaaa',
  'tag': 'K7QF2M',
  'conversation_id': 'c-7f3a',
  'generation': 3,
  'since_ms': 1,
};

Button _approve(WidgetTester tester) => tester.widget<Button>(
  find.ancestor(of: find.text('Approve'), matching: find.byType(Button)),
);

void main() {
  setUp(() => _friends([_ana]));
  tearDown(() => contactsApi = null);

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
    // Whose conversation, by People's friends here; never its id.
    expect(find.text('Only in:'), findsOneWidget);
    expect(find.text('•  Ana · K7QF2M'), findsOneWidget);
    expect(find.textContaining('c-7f3a'), findsNothing);
    expect(find.text("Rivet's reason"), findsOneWidget);
    expect(find.text('Until you take it back.'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Decline'), findsOneWidget);
  });

  testWidgets(
    "a conversation this device can't find: said, and Approve is off",
    (tester) async {
      final harness = await _card(
        tester,
        _grant(conversations: ['c-7f3a', 'c-0000']),
      );
      expect(find.text('•  Ana · K7QF2M'), findsOneWidget);
      expect(
        find.text("•  A conversation this device can't find"),
        findsOneWidget,
      );
      expect(find.textContaining('c-0000'), findsNothing);
      expect(
        find.text(
          "You can't approve this: one of these conversations isn't among "
          'yours on this device.',
        ),
        findsOneWidget,
      );
      expect(_approve(tester).onPressed, isNull);
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(harness.backend.sent.where((m) => m.hasDecideAppGrant()), isEmpty);
      // Decline still answers.
      await tester.tap(find.text('Decline'));
      await settle(tester);
      expect(harness.last.decideAppGrant.approve, isFalse);
    },
  );

  testWidgets("friends that can't be read: Approve off until Try again", (
    tester,
  ) async {
    _friends(null);
    final harness = await _card(tester, _grant());
    // Unchecked, never called unknown: it may well be the person's.
    expect(
      find.text("•  A conversation that can't be checked right now"),
      findsOneWidget,
    );
    expect(find.textContaining("can't find"), findsNothing);
    expect(_approve(tester).onPressed, isNull);
    _friends([_ana]);
    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(find.text('•  Ana · K7QF2M'), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(harness.last.decideAppGrant.approve, isTrue);
  });

  testWidgets('Approve sends the decision for its seq', (tester) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(harness.last.hasDecideAppGrant(), isTrue);
    expect(decide.agentId, 'rivet');
    expect(decide.seq, Int64(12));
    expect(decide.approve, isTrue);
    expect(decide.reply, isFalse);
    expect(decide.comment, isEmpty);
  });

  testWidgets('Decline declines', (tester) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Decline'));
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(decide.approve, isFalse);
    expect(decide.reply, isFalse);
  });

  testWidgets('a typed comment: one Submit, a reply that gives nothing '
      '(#370)', (tester) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(_comment, ' Only Ana, and only reading. ');
    await settle(tester);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Decline'), findsNothing);
    expect(
      find.text(
        'Nothing is given. Rivet gets your comment and can suggest it again.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Submit'));
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(decide.reply, isTrue);
    // An older backend that ignores reply declines it.
    expect(decide.approve, isFalse);
    expect(decide.comment, 'Only Ana, and only reading.');

    // Cleared, the buttons are back.
    await tester.enterText(_comment, '  ');
    await settle(tester);
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Submit'), findsNothing);
  });

  testWidgets('Return in the comment is Submit, never Approve', (tester) async {
    final harness = await _card(tester, _grant());
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(_comment, 'Read only, please.');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final decide = harness.last.decideAppGrant;
    expect(decide.reply, isTrue);
    expect(decide.approve, isFalse);
    expect(decide.comment, 'Read only, please.');
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

  testWidgets('decided: replied, with the reply', (tester) async {
    await _card(
      tester,
      _grant(state: AppGrantState.APP_GRANT_STATE_DECLINED, comment: 'Not yet.')
        ..replied = true,
    );
    expect(find.textContaining('Replied'), findsOneWidget);
    expect(find.textContaining('Declined'), findsNothing);
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
      find.text(
        "Mike has read a web page, which may hold someone else's instructions.",
      ),
      findsOneWidget,
    );
  });

  testWidgets('read a pipe and could send as you: the danger box', (
    tester,
  ) async {
    await _card(tester, _grant(readPipes: true, sends: true));
    expect(
      find.text(
        "Mike has read a web page, which may hold someone else's "
        'instructions. Whoever wrote it could try to get Mike to send '
        'messages as you. Approve only if you expected this.',
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

  testWidgets("what reorders Rivet's reason shows by its code", (tester) async {
    await _card(tester, _grant()..reason = 'Approve evil\u202Etxt.exe');
    expect(find.text(r'Approve evil\u{202E}txt.exe'), findsOneWidget);
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
