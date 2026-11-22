// The agent's bar over its conversation (agent_view.dart's _Header): the
// title bar's height at ordinary text, and taller rather than cut when
// large text needs it (Sorbet's iPad pass: "Theo / Researcher" spilled over
// the pane below at Accessibility XL).
import 'package:factory_app/agent_name.dart';
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  agents: [
    Agent(
      id: 'theo',
      name: 'Theo',
      role: 'Researcher',
      color: 'green',
      modelAvailable: true,
    ),
  ],
  selectedAgentId: 'theo',
  settings: Settings(userName: 'Matt'),
);

/// The agent's name over its role in its bar (the sidebar's row has one
/// too).
Finder get _name => find.descendant(
  of: find.byType(AgentView),
  matching: find.byType(NameOverRole),
);

/// The bar holding it.
Finder get _bar =>
    find.ancestor(of: _name, matching: find.byType(Container)).first;

void main() {
  testWidgets('the title bar’s height at ordinary text', (tester) async {
    await Harness(_workspace()).show(tester, size: const Size(1366, 1024));
    await settle(tester);
    expect(tester.getSize(_bar).height, titleBarHeight);
  });

  for (final scale in [2.0, 3.1]) {
    testWidgets('taller, never cut, at ${scale}x text on an iPad', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await Harness(_workspace()).show(tester, size: const Size(1366, 1024));
      await settle(tester);
      expect(tester.getSize(_bar).height, greaterThan(titleBarHeight));
      // The name and role fit inside it.
      final bar = tester.getRect(_bar);
      final name = tester.getRect(_name);
      expect(bar.top, lessThanOrEqualTo(name.top));
      expect(bar.bottom, greaterThanOrEqualTo(name.bottom));
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
  }
}
