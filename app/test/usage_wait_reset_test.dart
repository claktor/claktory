// Use a reset… on the wait line: when a spent account has a
// usage reset that refills the limit an agent waits on, the line offers it,
// before Continue on another provider.
import 'package:factory_app/agent_view.dart' show WorkingLine;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/usage_wait.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'usage_test.dart' show later, weekly;

const _said = 'Waiting for your Claude usage to reset at Fri 18:57';

UsageReset _reset(String id, String label, List<String> windows) => UsageReset(
  id: id,
  label: label,
  detail: 'Refills your limit.',
  left: 1,
  usable: true,
  windowIds: windows,
);

final _weeklyReset = _reset('g1', 'Weekly reset', ['weekly']);
final _fableReset = _reset('g2', 'Fable reset', ['weekly_fable']);
final _fiveHourReset = _reset('g3', '5-hour reset', ['five_hour']);

/// Claude's account [who] with its weekly limit at [percent] (and Fable's
/// own at [fable]), with [resets].
Account _account(
  String who,
  double percent,
  List<UsageReset> resets, {
  double fable = 0,
}) => Account(
  id: who,
  label: who,
  usage: weekly(percent)
    ..windows.add(
      UsageWindow(
        id: 'weekly_fable',
        label: 'Weekly (Fable)',
        usedPercent: fable,
        resetsAt: later(const Duration(days: 2)),
      ),
    )
    ..resets.addAll(resets),
);

Workspace _waiting(List<Account> accounts, {String model = ''}) {
  final agent = builder()
    ..usageWait = _said
    ..usageWaitAlternative = UsageWaitAlternative(
      providerName: 'OpenRouter',
      modelName: 'Claude Opus 4.8',
      sameModel: true,
    );
  if (model.isNotEmpty) agent.model = model;
  return workspace(agents: [agent])..providers.first.accounts.addAll(accounts);
}

Finder _inLine(String text) =>
    find.descendant(of: find.byType(WorkingLine), matching: find.text(text));

void main() {
  group('which resets help', () {
    List<String> labels(Workspace workspace, String model) => [
      for (final option in waitResets(workspace, model)) option.reset.label,
    ];

    test('one that refills a limit reached that the model draws on', () {
      final spent = _waiting([
        _account('a@x', 100, [_weeklyReset, _fableReset, _fiveHourReset]),
      ]);
      // Opus waits on the weekly limit; the 5-hour one has room, and
      // Fable's own isn't Opus's.
      expect(labels(spent, 'anthropic/claude-opus-5-5'), ['Weekly reset']);
      expect(labels(spent, 'openai-codex/gpt-5.5'), isEmpty);
    });

    test("Fable's own limit only for a Fable model", () {
      final fable = _waiting([
        _account('a@x', 40, [_weeklyReset, _fableReset], fable: 100),
      ]);
      expect(labels(fable, 'anthropic/claude-fable-5'), ['Fable reset']);
      expect(labels(fable, 'anthropic/claude-opus-5-5'), isEmpty);
    });

    test("one that doesn't say refills every limit; none left, none", () {
      final spent = _waiting([
        _account('a@x', 100, [
          _reset('g4', 'Full reset', []),
          _weeklyReset.deepCopy()..left = 0,
        ]),
      ]);
      expect(labels(spent, 'anthropic/claude-opus-5-5'), ['Full reset']);
    });
  });

  testWidgets('Use a reset… comes before Continue on OpenRouter and asks '
      'before using it', (tester) async {
    final harness = Harness(
      _waiting([
        _account('a@x', 100, [_weeklyReset]),
      ]),
    );
    harness.backend.usageResetAnswer = UsageResetAnswer(
      reset: true,
      message: 'Your weekly limit is reset.',
    );
    await harness.show(tester);
    await settle(tester);
    expect(
      tester.getRect(_inLine('Use a reset…')).right,
      lessThan(tester.getRect(_inLine('Continue on OpenRouter')).left),
    );
    await tester.tap(_inLine('Use a reset…'));
    await settle(tester);
    expect(find.text('Use a weekly reset?'), findsOneWidget);
    await tester.tap(find.text('Use reset'));
    await settle(tester);
    final used = harness.last.useUsageReset;
    expect(used.providerId, 'anthropic');
    expect(used.accountId, 'a@x');
    expect(used.resetId, 'g1');
  });

  testWidgets('with several, which one is asked first', (tester) async {
    final harness = Harness(
      _waiting([
        _account('a@x', 100, [_weeklyReset]),
        _account('b@x', 100, [_reset('g5', 'Full reset', [])]),
      ]),
    );
    await harness.show(tester);
    await settle(tester);
    await tester.tap(_inLine('Use a reset…'));
    await settle(tester);
    expect(find.text('Weekly reset · a@x'), findsOneWidget);
    await tester.tap(find.text('Full reset · b@x'));
    await settle(tester);
    expect(find.text('Use a full reset?'), findsOneWidget);
  });

  testWidgets('no reset that would help, no Use a reset…', (tester) async {
    final harness = Harness(
      _waiting([
        _account('a@x', 100, [_fiveHourReset]),
      ]),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inLine('Continue on OpenRouter'), findsOneWidget);
    expect(_inLine('Use a reset…'), findsNothing);
  });

  testWidgets('a wait for the network (O2) offers no reset, only Try now '
      'and Stop', (tester) async {
    final start = _waiting([
      _account('a@x', 100, [_weeklyReset]),
    ]);
    start.agents.single
      ..usageWait = 'Waiting for the network…'
      ..clearUsageWaitAlternative();
    final harness = Harness(start);
    await harness.show(tester);
    await settle(tester);
    expect(_inLine('Waiting for the network…'), findsOneWidget);
    expect(_inLine('Use a reset…'), findsNothing);
    expect(_inLine('Try now'), findsOneWidget);
    await tester.tap(_inLine('Try now'));
    await tester.pump();
    expect(harness.last.tryAgainNow.agentId, 'a1');
  });
}
