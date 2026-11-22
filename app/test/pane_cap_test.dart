import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({int cap = 4}) => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt', maxOpenPanes: cap),
);

const _note =
    'Once this many panes are open beside a conversation, counting '
    "Settings, the Map and files, agents can't show you more. You can "
    'always open more yourself.';

Future<void> _openDisplay(WidgetTester tester, Harness harness) async {
  await harness.show(tester);
  harness.page.value = AppPage.settings;
  await settle(tester);
  // Settings builds its sections as they scroll into view.
  await tester.scrollUntilVisible(
    find.text('Panes agents can open'),
    200,
    scrollable: find
        .descendant(
          of: find.byType(SettingsView),
          matching: find.byType(Scrollable),
        )
        .first,
  );
  await settle(tester);
}

void main() {
  testWidgets('Settings → Display sets how many panes agents can open', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _openDisplay(tester, harness);
    expect(find.text(_note), findsOneWidget);
    final row = find.ancestor(
      of: find.text('Panes agents can open'),
      matching: find.byType(SettingRow),
    );
    expect(
      find.descendant(of: row, matching: find.text('4')),
      findsOneWidget,
      reason: 'the backend’s limit, 4 until changed',
    );

    await tester.tap(find.descendant(of: row, matching: find.text('4')));
    await settle(tester);
    // It offers 1 to 11: the backend refuses 12 or more.
    final select = tester.widget<Select<int>>(
      find.descendant(of: row, matching: find.byType(Select<int>)),
    );
    expect(select.options.map((option) => option.value), [
      for (var count = 1; count <= 11; count++) count,
    ]);
    await tester.tap(find.text('6').last);
    await settle(tester);
    expect(harness.last.updateSettings, UpdateSettings(maxOpenPanes: 6));

    // It shows what the backend keeps, not what was picked.
    await harness.apply(tester, _workspace(cap: 6));
    expect(find.descendant(of: row, matching: find.text('6')), findsOneWidget);
  });

  testWidgets('a Claktory from before the limit shows no control', (
    tester,
  ) async {
    final harness = Harness(_workspace(cap: 0));
    await harness.show(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    await tester.scrollUntilVisible(
      find.textContaining('A soft glow on what changes'),
      200,
      scrollable: find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await settle(tester);
    expect(find.text('Panes agents can open'), findsNothing);
  });
}
