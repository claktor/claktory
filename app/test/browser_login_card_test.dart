// The login card (browser-simple ux.md §3, Lucy; Secco's contract §2): a
// site's first login, the site refusing it, a code next, one of the
// person's own logins that fits, a lookalike, and Dismiss. The fields are
// marked for the device's password autofill.
import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({List<SecretInfo> secrets = const []}) => Workspace(
  agents: [
    Agent(id: 'yukako', name: 'Yukako', color: 'pink', modelAvailable: true),
  ],
  selectedAgentId: 'yukako',
  settings: Settings(userName: 'Matt'),
  vault: VaultState(exists: true, unlocked: true, secrets: secrets),
);

SecretRequest _login({
  SecretRequestState state = SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  SecretLoginOutcome outcome =
      SecretLoginOutcome.SECRET_LOGIN_OUTCOME_UNSPECIFIED,
  String site = 'x.com',
  String fitting = '',
  String lookalike = '',
  bool added = false,
}) => SecretRequest(
  id: 'r1',
  agentId: 'yukako',
  state: state,
  reason: 'To post the launch for you.',
  newSecret: NewSecret(
    name: 'X login',
    variables: [
      SecretVariable(name: 'USERNAME'),
      SecretVariable(name: 'PASSWORD'),
    ],
  ),
  sites: ['https://$site'],
  fillOnly: true,
  login: true,
  loginSite: site,
  loginOutcome: outcome,
  fittingSecretId: fitting.isEmpty ? '' : 's1',
  fittingSecretName: fitting,
  lookalikeSecretName: lookalike.isEmpty ? '' : 'X login',
  lookalikeSite: lookalike,
  added: added,
  givenSecretName: state == SecretRequestState.SECRET_REQUEST_STATE_PENDING
      ? ''
      : 'X login',
  decidedMs: Int64(
    state == SecretRequestState.SECRET_REQUEST_STATE_PENDING ? 0 : 9,
  ),
);

/// The card as the conversation shows it, its request changing as the
/// backend's does.
Future<ValueNotifier<SecretRequest>> _show(
  WidgetTester tester,
  Harness harness,
  SecretRequest request,
) async {
  tester.view.physicalSize = const Size(1000, 1200);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final shown = ValueNotifier(request);
  addTearDown(shown.dispose);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => AppScope(
        backend: harness.backend,
        login: harness.login,
        page: harness.page,
        appBuild: AppBuild(
          version: 'dev',
          bundlePath: '/Applications/Factory.app',
          quit: () {},
          updater: harness.updater,
        ),
        drafts: harness.drafts,
        history: harness.history,
        connections: harness.connections,
        child: Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => SingleChildScrollView(
                padding: const EdgeInsets.all(24),
                child: ValueListenableBuilder(
                  valueListenable: shown,
                  builder: (context, request, _) =>
                      SecretRequestCard(request, key: const ValueKey('r1')),
                ),
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.pump();
  return shown;
}

Finder _field(String label) => find.byWidgetPredicate(
  (widget) => widget is TextField && widget.semanticsLabel == label,
);

Finder _sign(String label) => find.text(label, findRichText: true);

DecideSecretRequest _decided(Harness harness) => harness.backend.sent
    .where((message) => message.hasDecideSecretRequest())
    .single
    .decideSecretRequest;

void main() {
  testWidgets("first login: Claktory's heading, the reason once, the two "
      'fields marked for autofill, Saved as X login; Sign in off until '
      'both are in, then it sends both under the login\'s name', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, _login());
    expect(_sign('Yukako asks to sign in to x.com'), findsOneWidget);
    expect(find.text('To post the launch for you.'), findsOneWidget);
    expect(find.text('Email or username'), findsOneWidget);
    expect(find.text('Password'), findsOneWidget);
    expect(find.text('Saved as X login'), findsOneWidget);
    expect(find.text('Edit'), findsOneWidget);
    expect(
      find.text('Claktory types it on x.com only. Yukako never sees it.'),
      findsOneWidget,
    );
    expect(find.text('Dismiss'), findsOneWidget);
    // Marked for the device's passwords, in one group.
    final user = tester.widget<TextField>(
      _field('Email or username for x.com'),
    );
    final password = tester.widget<TextField>(_field('Password for x.com'));
    expect(user.autofillHints, [AutofillHints.username]);
    expect(password.autofillHints, [AutofillHints.password]);
    expect(
      find.ancestor(
        of: _field('Password for x.com'),
        matching: find.byType(AutofillGroup),
      ),
      findsOneWidget,
    );
    expect(
      tester
          .widget<EditableText>(
            find.descendant(
              of: _field('Password for x.com'),
              matching: find.byType(EditableText),
            ),
          )
          .obscureText,
      isTrue,
    );

    // Off until both are in.
    await tester.tap(find.text('Sign in'));
    await tester.pump();
    expect(
      harness.backend.sent.where((m) => m.hasDecideSecretRequest()),
      isEmpty,
    );
    await tester.enterText(
      _field('Email or username for x.com'),
      ' @claktory ',
    );
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await tester.pump();
    expect(
      harness.backend.sent.where((m) => m.hasDecideSecretRequest()),
      isEmpty,
    );
    await tester.enterText(_field('Password for x.com'), 'hunter2 ');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    final decided = _decided(harness);
    expect(decided.approve, isTrue);
    expect(decided.secretId, isEmpty);
    expect(decided.name, 'X login');
    expect(
      [for (final entry in decided.entries) (entry.variable, entry.value)],
      [('USERNAME', '@claktory'), ('PASSWORD', 'hunter2 ')],
    );
  });

  testWidgets('Edit changes the name only, and it goes with Sign in', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, _login());
    await tester.tap(find.text('Edit'));
    await tester.pump();
    await tester.enterText(_field('Name'), 'X (work)');
    await tester.enterText(_field('Email or username for x.com'), 'me');
    await tester.enterText(_field('Password for x.com'), 'pw');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    expect(_decided(harness).name, 'X (work)');
  });

  testWidgets('signing in, then signed in: how filling it in went', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final shown = await _show(
      tester,
      harness,
      _login(
        state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNING_IN,
        added: true,
      ),
    );
    expect(
      find.textContaining('Signing in to x.com…', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Sign in'), findsNothing);
    shown.value = _login(
      state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNED_IN,
      added: true,
    );
    await tester.pump();
    expect(
      find.textContaining(
        'Signed in to x.com · Saved as X login',
        findRichText: true,
      ),
      findsOneWidget,
    );
  });

  testWidgets('the site refused it: open again, the username kept, the '
      'password cleared, a danger line', (tester) async {
    final harness = Harness(_workspace());
    final shown = await _show(tester, harness, _login());
    await tester.enterText(_field('Email or username for x.com'), '@claktory');
    await tester.enterText(_field('Password for x.com'), 'wrong');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    shown.value = _login(
      state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNING_IN,
      added: true,
    );
    await tester.pump();
    // The backend sends the fields empty: what was typed is this screen's.
    shown.value = _login(
      outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED,
    );
    await tester.pump();
    expect(
      find.text("x.com didn't accept it. Check it and try again."),
      findsOneWidget,
    );
    final user = tester.widget<TextField>(
      _field('Email or username for x.com'),
    );
    final password = tester.widget<TextField>(_field('Password for x.com'));
    expect(user.controller.text, '@claktory');
    expect(password.controller.text, isEmpty);
    // Sign in again sends it again, both fields.
    await tester.enterText(_field('Password for x.com'), 'right');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    final again = harness.backend.sent
        .where((m) => m.hasDecideSecretRequest())
        .last
        .decideSecretRequest;
    expect(
      [for (final entry in again.entries) entry.value],
      ['@claktory', 'right'],
    );
  });

  testWidgets('refused with a saved login: names it, and Sign in again '
      'updates it under its own name', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      _login(outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED)
        ..givenSecretId = 's1'
        ..givenSecretName = 'X (shared)',
    );
    expect(
      find.text("x.com didn't accept your X (shared). Check it and try again."),
      findsOneWidget,
    );
    expect(
      find.text(
        'Signing in updates your X (shared) for every agent that uses it.',
      ),
      findsOneWidget,
    );
    expect(find.text('Edit'), findsNothing);
    await tester.enterText(_field('Email or username for x.com'), 'me');
    await tester.enterText(_field('Password for x.com'), 'pw');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    expect(_decided(harness).name, 'X (shared)');
  });

  testWidgets('asks for a code next: saved, and the code card follows', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      _login(
        state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_CODE_NEXT,
        added: true,
      ),
    );
    expect(
      find.textContaining(
        'Saved as X login. x.com asks for a code next.',
        findRichText: true,
      ),
      findsOneWidget,
    );
  });

  testWidgets('one of yours fits: Use your X login on x.com?, no fields; '
      'Use it gives that login', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          SecretInfo(
            id: 's1',
            name: 'X login',
            sites: ['https://x.com'],
            variables: [
              SecretVariable(name: 'USERNAME'),
              SecretVariable(name: 'PASSWORD'),
            ],
          ),
        ],
      ),
    );
    final shown = await _show(tester, harness, _login(fitting: 'X login'));
    expect(_sign('Use your X login on x.com?'), findsOneWidget);
    expect(find.text('X login · yours'), findsOneWidget);
    expect(find.text('Yukako never sees it.'), findsOneWidget);
    expect(find.text('Email or username'), findsNothing);
    expect(find.text('Password'), findsNothing);
    await tester.tap(find.text('Use it'));
    await pumpFrames(tester);
    final decided = _decided(harness);
    expect(decided.approve, isTrue);
    expect(decided.secretId, 's1');
    expect(decided.entries, isEmpty);
    shown.value = _login(
      state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNED_IN,
      fitting: 'X login',
    );
    await tester.pump();
    expect(
      find.textContaining('Signed in to x.com', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('Saved as', findRichText: true), findsNothing);
  });

  testWidgets('one of yours fits, but you sign in with another: its fields, '
      'saved under a name of its own; and back to yours', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          SecretInfo(id: 's1', name: 'X login', sites: ['https://x.com']),
          SecretInfo(id: 's2', name: 'X login 2'),
        ],
      ),
    );
    await _show(tester, harness, _login(fitting: 'X login'));
    expect(find.text('Use it'), findsOneWidget);
    await tester.tap(find.text('Use a different login…'));
    await tester.pump();
    expect(_sign('Yukako asks to sign in to x.com'), findsOneWidget);
    expect(find.text('Use it'), findsNothing);
    expect(find.text('Saved as X login 3'), findsOneWidget);

    await tester.tap(find.text('Use your X login instead'));
    await tester.pump();
    expect(find.text('Use it'), findsOneWidget);
    expect(find.text('Email or username'), findsNothing);

    await tester.tap(find.text('Use a different login…'));
    await tester.pump();
    await tester.enterText(_field('Email or username for x.com'), 'work');
    await tester.enterText(_field('Password for x.com'), 'pw');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    final decided = _decided(harness);
    expect(decided.secretId, isEmpty);
    expect(decided.name, 'X login 3');
    expect(
      [for (final entry in decided.entries) (entry.variable, entry.value)],
      [('USERNAME', 'work'), ('PASSWORD', 'pw')],
    );
  });

  testWidgets('a lookalike: the first-login card with a danger line above '
      'the fields', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      _login(site: 'x-login.com', lookalike: 'x.com'),
    );
    expect(_sign('Yukako asks to sign in to x-login.com'), findsOneWidget);
    expect(
      find.text(
        'This page is x-login.com, not x.com. Your X login is for x.com.',
      ),
      findsOneWidget,
    );
    expect(
      tester.getTopLeft(find.textContaining('This page is')).dy,
      lessThan(tester.getTopLeft(find.text('Email or username')).dy),
    );
    expect(find.text('Sign in'), findsOneWidget);
  });

  testWidgets('Dismiss, with words: they go as the reason; ended, it says '
      'Dismissed', (tester) async {
    final harness = Harness(_workspace());
    final shown = await _show(tester, harness, _login());
    await tester.enterText(
      find.descendant(
        of: find.byType(SecretRequestCard),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is TextField &&
              widget.semanticsLabel != 'Email or username for x.com' &&
              widget.semanticsLabel != 'Password for x.com',
        ),
      ),
      'Use the work account',
    );
    await tester.pump();
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    final decided = _decided(harness);
    expect(decided.approve, isFalse);
    expect(decided.comment, 'Use the work account');
    expect(decided.entries, isEmpty);
    shown.value = _login(
      state: SecretRequestState.SECRET_REQUEST_STATE_DISMISSED,
    );
    await tester.pump();
    expect(
      find.textContaining('Dismissed', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets("refused by Claktory: why, in danger, and what's typed stays", (
    tester,
  ) async {
    final harness = Harness(_workspace())
      ..backend.refuse = 'The vault is locked.';
    await _show(tester, harness, _login());
    await tester.enterText(_field('Email or username for x.com'), 'me');
    await tester.enterText(_field('Password for x.com'), 'pw');
    await tester.pump();
    await tester.tap(find.text('Sign in'));
    await pumpFrames(tester);
    expect(find.text('The vault is locked.'), findsOneWidget);
    expect(
      tester.widget<TextField>(_field('Password for x.com')).controller.text,
      'pw',
    );
  });

  test('the Needs you row and its outcome say the site', () {
    final workspace = _workspace();
    expect(
      secretRequestSummary(_login(), workspace),
      'Asks to sign in to x.com',
    );
    expect(
      secretRequestSummary(_login(fitting: 'X login'), workspace),
      'Asks to use your X login on x.com',
    );
    expect(
      secretOutcome(
        _login(
          state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
          outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNED_IN,
          added: true,
        ),
        agent: 'Yukako',
      ),
      'Signed in to x.com · Saved as X login',
    );
  });
}
