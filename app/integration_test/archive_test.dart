// Archiving a project and bringing it back, on an iPhone (the Simulator)
// already connected to a real Claktory: a long press on the project opens
// its settings, Archive project hides it and its agents, and Unarchive under
// Archived brings them back.
//
//   cd app && flutter test integration_test/archive_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true --dart-define=PROJECT=Factory \
//     [--dart-define=PAIRING_CODE='factory://…']
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _project = String.fromEnvironment('PROJECT', defaultValue: 'Factory');
const _code = String.fromEnvironment('PAIRING_CODE');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a project archived, then unarchived', (tester) async {
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
    // Paired before with a key this Claktory no longer knows: the new code.
    for (var i = 0; i < 100 && _code.isNotEmpty; i++) {
      if (find.textContaining('allow this device').evaluate().isNotEmpty ||
          find.text(_project).evaluate().isNotEmpty) {
        break;
      }
      await tester.pump(const Duration(milliseconds: 200));
    }
    if (_code.isNotEmpty &&
        find.textContaining('allow this device').evaluate().isNotEmpty) {
      AppScope.switchFactory(
        tester.element(find.byType(Sidebar).first),
        parsePairingCode(_code),
      );
      await tester.pump(const Duration(seconds: 1));
    }
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.pump();
      await tester.tap(find.text('Continue'));
    }
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 2),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    final workspace = backend.workspace;
    // Until the first snapshot comes (and it's allowed in, when new).
    await waitFor(
      tester,
      find.text(_project),
      timeout: const Duration(minutes: 2),
    );
    final project = workspace.workspace.projects.firstWhere(
      (p) => p.name == _project,
    );
    bool archived() => workspace.workspace.projects
        .firstWhere((p) => p.id == project.id)
        .archived;
    expect(archived(), isFalse);
    // The list of everything, not a focused project.
    if (workspace.workspace.focusedProjectId.isNotEmpty) {
      await backend.focusProject('');
    }
    final agents = [
      for (final agent in workspace.workspace.agents)
        if (agent.projectId == project.id) agent.name,
    ];
    await waitFor(tester, find.text(_project));
    await tester.longPress(find.text(_project).first);
    await waitFor(tester, find.text('Archive project'));
    await tester.ensureVisible(find.text('Archive project'));
    await hold(tester, seconds: 2);
    await tester.tap(find.text('Archive project'));
    await waitFor(tester, find.text('Archived (1)'));
    expect(archived(), isTrue);
    expect(find.text(_project), findsNothing);
    for (final name in agents) {
      expect(find.text(name), findsNothing, reason: '$name is hidden');
    }
    await hold(tester, seconds: 2);

    await tester.tap(find.text('Archived (1)'));
    await waitFor(tester, find.text('Unarchive'));
    await hold(tester, seconds: 2);
    await tester.tap(find.text('Unarchive'));
    await waitFor(tester, find.text('Archived (1)'), gone: true);
    expect(archived(), isFalse);
    expect(find.text(_project), findsOneWidget);
    await hold(tester, seconds: 2);
  });
}
