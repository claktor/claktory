// Rivet's settings against a real Claktory: its prompt is Claktory's
// default (or says it was changed), as the backend sends it, and a changed one goes back with Reset
// to default. Nothing is saved. Runs on an iPhone (the Simulator) already
// connected:
//
//   cd app && flutter test integration_test/default_prompt_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/forms.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets("Rivet's prompt is the default, and goes back to it", (
    tester,
  ) async {
    await app.main();
    await waitFor(
      tester,
      find.text('Rivet'),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final rivet = scope.backend.workspace.workspace.agents.firstWhere(
      (a) => a.id == 'rivet',
    );
    expect(rivet.defaultPrompt, isNotEmpty);
    // ignore: avoid_print
    print('the default: ${rivet.prompt.trim() == rivet.defaultPrompt.trim()}');
    await tester.tap(find.text('Rivet').first);
    await waitFor(tester, find.byType(AgentView));
    showModal(
      tester.element(find.byType(AgentView).first),
      (context, close) => AgentSettingsForm(agent: rivet, close: close),
    );
    await waitFor(tester, find.byType(AgentSettingsForm));
    const isDefault =
        "Claktory's default. Each update brings the newest version until you "
        'change it.';
    await tester.scrollUntilVisible(
      find.text(
        rivet.prompt.trim() == rivet.defaultPrompt.trim()
            ? isDefault
            : 'Changed from the default.',
      ),
      300,
      scrollable: find
          .descendant(
            of: find.byType(AgentSettingsForm),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await hold(tester, seconds: 3);
    final prompt = find
        .descendant(
          of: find.byWidgetPredicate(
            (w) =>
                w is TextField &&
                w.placeholder == 'What this agent does and how',
          ),
          matching: find.byType(EditableText),
        )
        .first;
    await tester.enterText(prompt, 'Only set up websites.');
    await tester.pump();
    await waitFor(tester, find.text('Changed from the default.'));
    await hold(tester, seconds: 3);
    await tester.tap(find.text('Reset to default'));
    await waitFor(tester, find.text(isDefault));
    expect(
      tester.widget<EditableText>(prompt).controller.text,
      rivet.defaultPrompt,
    );
    await hold(tester, seconds: 2);
    await tester.tap(find.text('Cancel'));
    await waitFor(tester, find.byType(AgentSettingsForm), gone: true);
  });
}
