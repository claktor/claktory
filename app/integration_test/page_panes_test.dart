// Settings as a pane, against a real Claktory, from an iPhone (the
// Simulator): the backend keeps a "settings" pane in an agent's layout, as
// a Mac's app puts it there, and sends it to every device; the phone, which
// shows Settings as a screen of its own, leaves it out of the ⋯ sheet's
// files. It holds the layout for HOLD seconds (20 unless given), so the Mac's
// Claktory Dev can be looked at meanwhile, then closes the pane.
//
//   cd app && flutter test integration_test/page_panes_test.dart -d <iPhone> \
//     --dart-define=FACTORY_DEV=true --dart-define=PAIRING_CODE='factory://…' \
//     --dart-define=AGENT=Builder [--dart-define=HOLD=20]
import 'package:factory_app/layout.dart' show chatOnly, leaves;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/panes.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Builder');
const _hold = int.fromEnvironment('HOLD', defaultValue: 20);

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a Settings pane is kept by the backend; a phone leaves it out', (
    tester,
  ) async {
    await app.main();
    await tester.pump(const Duration(seconds: 1));
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(_code, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE=…');
      for (var i = 0; i < 50; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await tester.pump(const Duration(milliseconds: 200));
      }
      await tester.enterText(find.byType(EditableText), _code);
      await tester.pump();
      await tester.tap(find.text('Connect'));
    }
    await waitFor(
      tester,
      find.text(_agent),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    final id = backend.workspace.workspace.agents
        .firstWhere((a) => a.name == _agent)
        .id;
    List<String> kinds() {
      final agent = backend.workspace.agent(id)!;
      return [
        for (final leaf in leaves(
          agent.hasLayout() ? agent.layout : chatOnly(),
        ))
          leaf.leaf.kind,
      ];
    }

    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 100 && !done(); i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(done(), isTrue);
    }

    await backend.selectAgent(id);
    if (kinds().length > 1) await backend.setLayout(id, null);
    await until(() => kinds().length == 1);

    // As a Mac's cog does: Settings right of the conversation.
    await backend.setLayout(id, withPage(chatOnly(), AppPage.settings));
    await until(() => kinds().contains('settings'));
    expect(kinds(), ['chat', 'settings']);

    // The phone: its conversation, with no Settings file under ⋯.
    await tester.tap(find.text(_agent).first);
    await waitFor(tester, find.text('Send'));
    await tester.tap(find.bySemanticsLabel('Extensions'));
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('Settings'), findsNothing);
    await hold(tester, seconds: 2);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await tester.pump(const Duration(seconds: 1));

    // Settings on a phone is still its own screen.
    scope.page.value = AppPage.settings;
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(SettingsView), findsOneWidget);
    expect(find.text('Done'), findsOneWidget);
    scope.page.value = AppPage.agent;

    // Meanwhile the Mac's app shows the pane: look at it now.
    // ignore: avoid_print
    print('HOLDING: Settings pane in $_agent\'s layout for $_hold s');
    await hold(tester, seconds: _hold);

    // Closed (the pane's ×), the backend has the conversation alone.
    await backend.setLayout(id, null);
    await until(() => kinds().length == 1);
  });
}
