// Needs you beyond cards: how long each item has waited (#255), a sign-in
// a provider stopped accepting (P2), and locked secrets agents wait for
// (P2, VaultState.agents_waiting).
import 'package:factory_app/identity.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart' show UnlockSecretsForm;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, {List<int> waiting = const []}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({
  List<int> june = const [],
  List<int> omar = const [],
  List<Provider> providers = const [],
  VaultState? vault,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    _agent('june', 'June', waiting: june),
    _agent('omar', 'Omar', waiting: omar),
  ],
  providers: providers,
  vault: vault,
);

Entry _ask(int seq, String title, DateTime at) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at.millisecondsSinceEpoch),
  askPerson: AskPerson(id: 'ask$seq', title: title, wantsText: true),
);

Account _account(String id, String label, {bool expired = false}) => Account(
  id: id,
  label: label,
  plan: 'Claude Max 20x',
  methodId: 'oauth',
  canSignOut: true,
  signInAgain: expired,
);

Provider _claude(List<Account> accounts) =>
    anthropic(signedInWith: 'Anthropic (Claude Pro/Max)', canSignOut: true)
      ..accounts.addAll(accounts);

Future<Harness> _open(WidgetTester tester, Harness harness) async {
  await harness.show(tester);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await settle(tester);
  await settle(tester);
  return harness;
}

/// The color of the span reading [text] in the rich text holding it.
Color? _spanColor(WidgetTester tester, String text) {
  final rich = tester.widget<RichText>(
    find.byWidgetPredicate(
      (widget) =>
          widget is RichText && widget.text.toPlainText().contains(text),
    ),
  );
  Color? found;
  rich.text.visitChildren((span) {
    if (span is TextSpan && (span.text ?? '').contains(text)) {
      found = span.style?.color;
      return false;
    }
    return true;
  });
  return found;
}

void main() {
  testWidgets('each item says how long it has waited, in signal past a day', (
    tester,
  ) async {
    final now = DateTime.now();
    final harness = Harness(_workspace(june: [7], omar: [2]));
    harness.backend.entriesById['june/7'] = _ask(
      7,
      'Which region?',
      now.subtract(const Duration(days: 2, minutes: 5)),
    );
    harness.backend.entriesById['omar/2'] = _ask(
      2,
      'Ship it today?',
      now.subtract(const Duration(minutes: 5)),
    );
    await _open(tester, harness);

    expect(
      find.textContaining('June · 2 days ago', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining('Omar · 5 min ago', findRichText: true),
      findsOneWidget,
    );
    final palette = Palette.of(tester.element(find.byType(InboxView)));
    expect(_spanColor(tester, '2 days ago'), palette.signal);
    expect(_spanColor(tester, '5 min ago'), palette.textFaint);

    expect(
      waitedADay(
        now.subtract(const Duration(hours: 23)).millisecondsSinceEpoch,
        now,
      ),
      isFalse,
    );
    expect(
      waitedADay(
        now.subtract(const Duration(hours: 24)).millisecondsSinceEpoch,
        now,
      ),
      isTrue,
    );
  });

  testWidgets('a sign-in a provider stopped accepting: one click starts it '
      'again and shows it in Settings → Providers', (tester) async {
    final harness = Harness(
      _workspace(
        providers: [
          _claude([
            _account('a1', 'matt@work.test', expired: true),
            _account('a2', 'matt@home.test'),
          ]),
        ],
      ),
    );
    expect(needsYouCount(harness.backend.workspace.workspace), 1);
    await _open(tester, harness);

    expect(find.text('Sign in to Anthropic again'), findsOneWidget);
    expect(
      find.text(
        "matt@work.test's sign-in expired. Agents use your other accounts "
        'meanwhile.',
      ),
      findsOneWidget,
    );

    await tester.tap(find.text('Sign in to Anthropic again'));
    await settle(tester);
    final start = harness.last.startLogin;
    expect(start.providerId, 'anthropic');
    expect(start.methodId, 'oauth');
    expect(start.addAccount, isTrue);
    expect(harness.page.value, AppPage.settings);
    // At the account's row.
    expect(find.text('Sign in again…'), findsOneWidget);
  });

  test("with no other account, agents on its models wait", () {
    final only = _account('a1', 'matt@work.test', expired: true);
    final provider = _claude([only]);
    expect(
      signInAgainText(provider, only),
      "matt@work.test's sign-in expired. Agents on its models wait until you "
      'sign in.',
    );
    // Another expired one doesn't count as one to use.
    final both = _claude([
      only,
      _account('a2', 'matt@home.test', expired: true),
    ]);
    expect(
      signInAgainText(both, both.accounts.first),
      endsWith('wait until you sign in.'),
    );
  });

  testWidgets('locked secrets agents wait for: one click unlocks them with '
      'the recovery key', (tester) async {
    final harness = Harness(
      _workspace(
        vault: VaultState(
          exists: true,
          unlocked: false,
          agentsWaiting: true,
          lockedWhy: 'Unlock them with your recovery key.',
        ),
      ),
    );
    expect(needsYouCount(harness.backend.workspace.workspace), 1);
    await _open(tester, harness);

    expect(
      find.text('Unlock the secrets on this Mac with its recovery key'),
      findsOneWidget,
    );
    expect(find.text('Unlock them with your recovery key.'), findsOneWidget);
    await tester.tap(find.text('Unlock the secrets on this Mac with its recovery key'));
    await settle(tester);
    expect(find.byType(UnlockSecretsForm), findsOneWidget);

    // Locked with nobody waiting: nothing in Needs you.
    expect(
      needsYouCount(
        _workspace(vault: VaultState(exists: true, unlocked: false)),
      ),
      0,
    );
  });
}
