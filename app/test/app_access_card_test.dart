// Rivet's proposal of what an app may read and which agents may use it
// (#333, AppAccessProposal): the card in Rivet's conversation and in Needs
// you, source by source and agent by agent (Fugo's point 8), Approve (all
// or nothing, with the digest shown) and Decline (DecideAppAccess), and each
// decided state.
import 'package:factory_app/app_access_card.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _activity = AppDataSource(
  id: 'activity',
  label: 'Activity',
  kind: AppDataKind.APP_DATA_KIND_ACTIVITY,
  holds:
      'Runs, background tasks, monitors, checklist counts and token use. '
      'No message text.',
);

final _conversations = AppDataSource(
  id: 'conversations',
  label: 'Conversations',
  kind: AppDataKind.APP_DATA_KIND_CONVERSATIONS,
  holds: 'The messages your agents got, with their text.',
  private: true,
);

AppInfo _charts({List<AppGrant> grants = const []}) => AppInfo(
  id: 'charts',
  name: 'Charts',
  tier: AppTier.APP_TIER_CLAKTORY,
  publisher: 'claktory',
  scopes: [
    AppScopeSpec(id: 'view', label: 'See dashboards', tools: ['charts_list']),
    AppScopeSpec(
      id: 'edit',
      label: 'Change dashboards',
      tools: ['charts_put', 'charts_query'],
      readsData: true,
    ),
  ],
  sources: [_activity.deepCopy(), _conversations.deepCopy()],
  grants: grants,
);

Workspace _workspace({List<AppGrant> grants = const []}) => Workspace(
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      role: 'Admin',
      color: 'blue',
      modelAvailable: true,
    ),
    Agent(id: 'mike', name: 'Mike', modelAvailable: true),
  ],
  selectedAgentId: 'rivet',
  apps: [_charts(grants: grants)],
  appsAvailable: true,
);

AppAccessProposal _access({
  AppGrantState state = AppGrantState.APP_GRANT_STATE_PENDING,
  List<AppDataSource>? sources,
  List<AppGrant>? agents,
  List<String> hosts = const [],
  List<String> readPipes = const [],
  List<String> readingPipes = const [],
  String comment = '',
  String error = '',
}) => AppAccessProposal(
  id: 'x1',
  appId: 'charts',
  appName: 'Charts',
  sources: sources ?? [_activity],
  agents:
      agents ??
      [
        AppGrant(agentId: 'mike', scopes: ['view', 'edit']),
      ],
  reason: 'You asked for a chart of what your agents did this week.',
  state: state,
  createdMs: Int64(DateTime(2026, 11, 23, 9).millisecondsSinceEpoch),
  decidedMs: state == AppGrantState.APP_GRANT_STATE_PENDING
      ? Int64.ZERO
      : Int64(DateTime(2026, 11, 23, 9, 5).millisecondsSinceEpoch),
  comment: comment,
  error: error,
  agentsReadPipes: readPipes,
  agentsReadingPipes: readingPipes,
  digest: 'd-1',
  publisher: 'claktory',
  tier: AppTier.APP_TIER_CLAKTORY,
  hosts: hosts,
);

Future<void> _tap(WidgetTester tester, String text) async {
  await tester.ensureVisible(find.text(text));
  await settle(tester);
  await tester.tap(find.text(text));
  await settle(tester);
}

Future<Harness> _card(
  WidgetTester tester,
  AppAccessProposal access, {
  List<AppGrant> grants = const [],
}) async {
  final harness = Harness(_workspace(grants: grants));
  harness.backend.transcript.load(
    Transcript(
      agentId: 'rivet',
      entries: [Entry(seq: Int64(12), appAccess: access)],
    ),
  );
  await harness.show(tester);
  await settle(tester);
  return harness;
}

final _comment = find.descendant(
  of: find.byType(AppAccessCard),
  matching: find.byType(EditableText),
);

void main() {
  test('its line names exactly what it would give', () {
    final workspace = _workspace();
    expect(
      appAccessSummary(_access(agents: []), workspace),
      'Let Charts read Activity?',
    );
    expect(
      appAccessSummary(_access(sources: []), workspace),
      'Let Mike use Charts?',
    );
    expect(
      appAccessSummary(
        _access(sources: [_activity, _conversations]),
        workspace,
      ),
      'Let Charts read Activity and Conversations, and let Mike use it?',
    );
  });

  test('where what it reads could go', () {
    expect(
      appAccessReach('Charts', [], ['Conversations']),
      "Charts can't reach the internet, so what it reads stays on this "
      'computer.',
    );
    expect(
      appAccessReach('Charts', ['api.example.com'], []),
      'Charts can reach api.example.com.',
    );
    expect(
      appAccessReach('Charts', ['api.example.com'], ['Conversations']),
      'Charts can reach api.example.com, so Conversations could leave this '
      'computer.',
    );
  });

  testWidgets('pending: the app, each source with what it holds, where it '
      'could go, each agent with what it could do and read, and why', (
    tester,
  ) async {
    await _card(tester, _access());
    expect(find.text('Rivet suggests'), findsOneWidget);
    expect(
      find.text('Let Charts read Activity, and let Mike use it?'),
      findsOneWidget,
    );
    expect(find.text('Charts · Made by Claktory'), findsOneWidget);
    expect(find.text('Charts could read:'), findsOneWidget);
    expect(find.text('•  Activity'), findsOneWidget);
    expect(find.textContaining('No message text.'), findsOneWidget);
    // Only what it asks: never "everything", never a source not on it.
    expect(find.textContaining('Conversations'), findsNothing);
    expect(find.textContaining('everything'), findsNothing);
    expect(
      find.text(
        "Charts can't reach the internet, so what it reads stays on this "
        'computer.',
      ),
      findsOneWidget,
    );
    expect(find.text('Mike could:'), findsOneWidget);
    expect(find.text('•  See dashboards'), findsOneWidget);
    expect(find.text('•  Change dashboards'), findsOneWidget);
    // Change dashboards reads what Charts reads.
    expect(
      find.text('Through Charts, Mike could also read Activity.'),
      findsOneWidget,
    );
    expect(find.textContaining('replaces'), findsNothing);
    expect(find.text("Rivet's reason"), findsOneWidget);
    expect(
      find.textContaining('a chart of what your agents did'),
      findsOneWidget,
    );
    expect(
      find.text('Approving allows exactly this, and nothing more.'),
      findsOneWidget,
    );
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Decline'), findsOneWidget);
  });

  testWidgets('a private source is marked, and with the internet in reach the '
      'card says it could leave this computer', (tester) async {
    await _card(
      tester,
      _access(
        sources: [_activity, _conversations],
        hosts: ['api.example.com'],
        readPipes: ['mike'],
        readingPipes: ['mike'],
      ),
      grants: [
        AppGrant(agentId: 'mike', scopes: ['view']),
      ],
    );
    expect(find.textContaining('Private'), findsOneWidget);
    expect(
      find.text(
        'Charts can reach api.example.com, so Conversations could leave this '
        'computer.',
      ),
      findsOneWidget,
    );
    expect(
      find.text(
        'Through Charts, Mike could also read Activity and Conversations.',
      ),
      findsOneWidget,
    );
    // Mike has See dashboards now: Change dashboards too replaces it.
    expect(
      find.text('This replaces what Mike can do with Charts now.'),
      findsOneWidget,
    );
    expect(
      find.text(
        "Mike has read a web page, which may hold someone else's instructions.",
      ),
      findsOneWidget,
    );
    // Reading a pipe through the app marks no one any more (#346).
    expect(find.textContaining('would read messages'), findsNothing);
  });

  testWidgets('Approve sends the decision with the digest shown', (
    tester,
  ) async {
    final harness = await _card(tester, _access());
    await _tap(tester, 'Approve');
    expect(harness.last.hasDecideAppAccess(), isTrue);
    final decide = harness.last.decideAppAccess;
    expect(decide.agentId, 'rivet');
    expect(decide.seq, Int64(12));
    expect(decide.approve, isTrue);
    expect(decide.reply, isFalse);
    expect(decide.digest, 'd-1');
  });

  testWidgets('Decline declines', (tester) async {
    final harness = await _card(tester, _access());
    await _tap(tester, 'Decline');
    final decide = harness.last.decideAppAccess;
    expect(decide.approve, isFalse);
    expect(decide.reply, isFalse);
  });

  testWidgets('a typed comment: one Submit, a reply that allows nothing '
      '(#370)', (tester) async {
    final harness = await _card(tester, _access());
    await tester.enterText(_comment, ' Only Activity, please. ');
    await settle(tester);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Decline'), findsNothing);
    expect(
      find.text('Submit sends your words instead of approving.'),
      findsOneWidget,
    );
    await _tap(tester, 'Submit');
    final decide = harness.last.decideAppAccess;
    expect(decide.reply, isTrue);
    // An older backend that ignores reply declines it.
    expect(decide.approve, isFalse);
    expect(decide.digest, isEmpty);
    expect(decide.comment, 'Only Activity, please.');
  });

  testWidgets('Return in the comment is Submit, never Approve', (tester) async {
    final harness = await _card(tester, _access());
    await tester.enterText(_comment, 'Not the conversations.');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final decide = harness.last.decideAppAccess;
    expect(decide.reply, isTrue);
    expect(decide.approve, isFalse);
    expect(decide.comment, 'Not the conversations.');
  });

  testWidgets('a refusal (it changed since) shows, and it can be approved '
      'again', (tester) async {
    final harness = await _card(tester, _access());
    harness.backend.refuse = 'This suggestion changed. Check it again.';
    await _tap(tester, 'Approve');
    expect(
      find.text('This suggestion changed. Check it again.'),
      findsOneWidget,
    );
    harness.backend.refuse = null;
    await _tap(tester, 'Approve');
    expect(
      harness.backend.sent.where((m) => m.hasDecideAppAccess()),
      hasLength(2),
    );
  });

  testWidgets('decided: approved', (tester) async {
    await _card(tester, _access(state: AppGrantState.APP_GRANT_STATE_GRANTED));
    expect(find.textContaining('Approved · '), findsOneWidget);
    expect(find.text('Approve'), findsNothing);
  });

  testWidgets('decided: declined, with the comment', (tester) async {
    await _card(
      tester,
      _access(
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
      _access(
        state: AppGrantState.APP_GRANT_STATE_DECLINED,
        comment: 'Not yet.',
      )..replied = true,
    );
    expect(find.textContaining('Replied'), findsOneWidget);
    expect(find.textContaining('Declined'), findsNothing);
    expect(find.text('Not yet.'), findsOneWidget);
  });

  testWidgets('decided: failed, with why', (tester) async {
    await _card(
      tester,
      _access(
        state: AppGrantState.APP_GRANT_STATE_FAILED,
        error: 'Charts was uninstalled.',
      ),
    );
    expect(
      find.textContaining("Approved, but it couldn't be done"),
      findsOneWidget,
    );
    expect(find.text('Charts was uninstalled.'), findsOneWidget);
  });

  testWidgets('Needs you lists it with its line and its card', (tester) async {
    final harness = Harness(_workspace());
    final entry = Entry(seq: Int64(12), appAccess: _access());
    expect(
      waitingSummary(entry, harness.backend),
      'Let Charts read Activity, and let Mike use it?',
    );
    final card = waitingCard(entry, agentId: 'rivet');
    expect(card, isA<AppAccessCard>());
    expect((card! as AppAccessCard).seq, 12);
    expect(card.key, const ValueKey('x1'));
  });

  group("why it's a card (Fugo, rows 674/675)", () {
    const tainted =
        'Rivet has read messages from another Claktory, which may hold '
        "someone else's instructions, so what it gives needs your approval.";
    const private =
        'Conversations holds what people wrote, so letting Charts read it '
        'needs your approval.';

    for (final phone in [false, true]) {
      testWidgets(
        "a tainted Rivet's card says so first, in Claktory's words, before "
        'anything it shows and before Rivet\'s reason'
        '${phone ? ' (phone)' : ''}',
        (tester) async {
          final harness = Harness(_workspace());
          harness.backend.transcript.load(
            Transcript(
              agentId: 'rivet',
              entries: [
                Entry(
                  seq: Int64(12),
                  appAccess: _access(sources: [_conversations])
                    ..cardRules.addAll([tainted, private]),
                ),
              ],
            ),
          );
          await harness.show(
            tester,
            size: phone ? const Size(390, 844) : const Size(1200, 900),
          );
          await settle(tester);
          await tester.ensureVisible(find.text('Why Claktory asks you'));
          await settle(tester);
          double top(String text) => tester.getTopLeft(find.text(text)).dy;
          expect(find.text(tainted), findsOneWidget);
          expect(find.text(private), findsOneWidget);
          expect(top('Why Claktory asks you'), lessThan(top(tainted)));
          expect(top(tainted), lessThan(top(private)));
          expect(top(private), lessThan(top('Charts could read:')));
          expect(top(private), lessThan(top("Rivet's reason")));
        },
      );
    }

    testWidgets('a card from before (no rules) shows no box', (tester) async {
      await _card(tester, _access());
      expect(find.text('Why Claktory asks you'), findsNothing);
    });

    testWidgets('the events each agent would hear, in the app\'s words', (
      tester,
    ) async {
      final workspace = _workspace();
      workspace.apps.first.events.add(
        AppEventKind(
          kind: 'dashboard_changed',
          label: 'A dashboard changed',
          scope: 'view',
        ),
      );
      final harness = Harness(workspace);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'rivet',
          entries: [
            Entry(
              seq: Int64(12),
              appAccess: _access()
                ..subscriptions.add(
                  AppSubscription(
                    eventKind: 'dashboard_changed',
                    agentIds: ['mike'],
                  ),
                ),
            ),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      expect(
        find.text('Mike would hear: A dashboard changed.'),
        findsOneWidget,
      );
    });
  });
}
