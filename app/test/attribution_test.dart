// Settings → General's switch for crediting Claktory in what agents
// publish (Workspace.attribution, SetAttribution).

import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

const _label = 'Credit Claktory in what agents publish';
const _note =
    'Pull requests, issues, comments and release notes your agents write on '
    'GitHub and similar sites end with “Made with Claktory”.';
const _next = 'Agents follow this from their next conversation.';

Toggle _toggle(WidgetTester tester) => tester.widget<Toggle>(
  find.byWidgetPredicate((w) => w is Toggle && w.label == _label),
);

void main() {
  testWidgets('the switch shows the setting and sends the change; once '
      'changed here it says when agents follow it, until the page is left', (
    tester,
  ) async {
    final harness = Harness(workspace()..attribution = true);
    await harness.show(tester, size: const Size(1200, 1600));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text(_label));
    expect(find.text(_note), findsOneWidget);
    expect(_toggle(tester).value, isTrue);
    expect(find.text(_next), findsNothing);

    // Changed elsewhere: the switch follows, with nothing said.
    await harness.apply(tester, workspace()..attribution = false);
    expect(_toggle(tester).value, isFalse);
    expect(find.text(_next), findsNothing);

    await tester.tap(find.text(_label));
    await settle(tester);
    expect(harness.last.setAttribution.on, isTrue);
    expect(find.text(_next), findsOneWidget);
    await harness.apply(tester, workspace()..attribution = true);
    expect(_toggle(tester).value, isTrue);

    await tester.tap(find.text(_label));
    await settle(tester);
    expect(harness.last.setAttribution.on, isFalse);

    harness.page.value = AppPage.agent;
    await settle(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text(_label));
    expect(find.text(_next), findsNothing);
  });
}
