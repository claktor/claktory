// The iPhone's Home Screen quick action, Needs you (#514 §7.2): its
// subtitle says how many wait, and a tap opens Needs you.
import 'package:factory_app/mobile/quick_actions.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace(List<int> waiting) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      modelAvailable: true,
      waitingSeqs: [for (final seq in waiting) Int64(seq)],
    ),
  ],
  selectedAgentId: 'june',
);

void main() {
  final ios = TargetPlatformVariant.only(TargetPlatform.iOS);

  /// What the app told the native side, and whether a tap launched it.
  (List<MethodCall>, Future<void> Function()) channel(
    WidgetTester tester, {
    bool launched = false,
  }) {
    final calls = <MethodCall>[];
    final messenger = tester.binding.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(quickActionsChannel, (call) async {
      calls.add(call);
      return call.method == 'listen' ? launched : null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(quickActionsChannel, null),
    );
    // A tap while the app runs.
    Future<void> tap() => messenger.handlePlatformMessage(
      quickActionsChannel.name,
      quickActionsChannel.codec.encodeMethodCall(
        const MethodCall('open', 'needs-you'),
      ),
      (_) {},
    );
    return (calls, tap);
  }

  List<Object?> said(List<MethodCall> calls) => [
    for (final call in calls)
      if (call.method == 'set') (call.arguments as Map)['subtitle'],
  ];

  testWidgets('it says how many wait, kept as they change', (tester) async {
    final (calls, _) = channel(tester);
    final harness = Harness(_workspace([3, 7]));
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(said(calls).last, '2 waiting');
    expect(
      calls.lastWhere((call) => call.method == 'set').arguments,
      containsPair('title', 'Needs you'),
    );

    harness.backend.workspace.apply(_workspace([]));
    await pumpFrames(tester);
    expect(said(calls).last, '', reason: 'nothing waits: no subtitle');
    // Said once per change, not on every build.
    final count = said(calls).length;
    harness.backend.workspace.apply(_workspace([]));
    await pumpFrames(tester);
    expect(said(calls), hasLength(count));
  }, variant: ios);

  testWidgets('a tap opens Needs you', (tester) async {
    final (_, tap) = channel(tester);
    final harness = Harness(_workspace([3]));
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    await tap();
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
  }, variant: ios);

  testWidgets('one that launched the app opens Needs you once it runs', (
    tester,
  ) async {
    channel(tester, launched: true);
    final harness = Harness(_workspace([3]));
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
  }, variant: ios);

  test('its words', () {
    expect(quickActionSubtitle(0), '');
    expect(quickActionSubtitle(1), '1 waiting');
    expect(quickActionSubtitle(12), '12 waiting');
  });
}
