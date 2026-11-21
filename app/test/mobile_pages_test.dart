// Usage, the Marketplace and What's new on a phone (#198, phase 3A): each a
// page with Back in a phone header, its tabs a finger's height in a row,
// and nothing cut off at the largest text sizes.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
  ],
  selectedAgentId: 'rivet',
);

Future<Harness> _open(
  WidgetTester tester,
  AppPage page, {
  Size size = const Size(393, 852),
}) async {
  final harness = Harness(_workspace());
  await harness.show(tester, size: size);
  harness.page.value = page;
  await pumpFrames(tester);
  await tester.pump(); // what the page asked for, answered
  await pumpFrames(tester);
  return harness;
}

void main() {
  for (final (page, title) in const [
    (AppPage.usage, 'Usage'),
    (AppPage.marketplace, 'Marketplace'),
    (AppPage.changelog, "What's new"),
  ]) {
    testWidgets('$title is a page with Back, which goes back', (tester) async {
      final harness = await _open(tester, page);
      expect(find.text(title), findsWidgets);
      expect(find.text('Done'), findsNothing);
      final back = find.bySemanticsLabel('Back');
      expect(back, findsOneWidget);
      expect(tester.getSize(back).height, greaterThanOrEqualTo(44));
      await tester.tap(back);
      await pumpFrames(tester);
      expect(harness.page.value, AppPage.agent);
    });
  }

  for (final size in const [Size(320, 568), Size(440, 956)]) {
    testWidgets('at the largest text on a ${size.width.toInt()}pt phone, '
        'nothing is cut off', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      for (final page in const [
        AppPage.usage,
        AppPage.marketplace,
        AppPage.changelog,
      ]) {
        final harness = await _open(tester, page, size: size);
        expect(tester.takeException(), isNull, reason: page.name);
        harness.page.value = AppPage.agent;
        await pumpFrames(tester);
      }
    });
  }
}
