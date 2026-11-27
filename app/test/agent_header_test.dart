// The agent's bar over its conversation (agent_view.dart's _Header): the
// title bar's height at ordinary text, and taller rather than cut when
// large text needs it (Sorbet's iPad pass: "Theo / Researcher" spilled over
// the pane below at Accessibility XL). Its controls: agent_menu_test.
import 'package:factory_app/agent_name.dart';
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
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

/// The bar holding it: at least the title bar's height.
Finder get _bar => find
    .ancestor(
      of: _name,
      matching: find.byWidgetPredicate(
        (widget) =>
            widget is Container &&
            widget.constraints?.minHeight == titleBarHeight,
      ),
    )
    .first;

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

  // A 600-point window beside the sidebar leaves the conversation about 312
  // points: the header gives way (agent-view.md §3: the meter goes, then
  // the effort, the role, and the name is cut) rather than overflow; the
  // model control and ⋯ stay.
  testWidgets('in a 600-point window, the header fits: the model and ⋯ '
      'stay', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(600, 800));
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.textContaining('123k'), findsNothing, reason: 'the meter went');
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics &&
            (widget.properties.label ?? '').startsWith('Model: '),
      ),
      findsOneWidget,
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics &&
            (widget.properties.label ?? '').startsWith('More options for '),
      ),
      findsOneWidget,
    );
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('in a wide window, the meter shows its bar beside its words', (
    tester,
  ) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    expect(find.text('123k / 400k'), findsOneWidget);
    // The bar: 40 by 4.
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Container &&
            widget.constraints?.maxWidth == 40 &&
            widget.constraints?.maxHeight == 4,
      ),
      findsOneWidget,
    );
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
