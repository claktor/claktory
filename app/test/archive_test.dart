import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show workspace;
import 'harness.dart';

/// The test workspace with Site (p2, Writer's project) archived, and
/// [selected] selected.
Workspace _archived({String selected = 'a1'}) {
  final snapshot = workspace()..selectedAgentId = selected;
  snapshot.projects[1].archived = true;
  return snapshot;
}

List<UpdateProject> _updates(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasUpdateProject()) message.updateProject,
];

void main() {
  testWidgets('an archived project and its agents leave the sidebar, and '
      'Archived (N) brings them back', (tester) async {
    final harness = Harness(_archived());
    await harness.show(tester);
    await settle(tester);
    expect(find.text('Factory'), findsOneWidget);
    expect(find.text('Site'), findsNothing);
    expect(find.text('Writer'), findsNothing);
    expect(find.text('Archived (1)'), findsOneWidget);

    await tester.tap(find.text('Archived (1)'));
    await settle(tester);
    expect(find.text('Site'), findsOneWidget);
    expect(find.text('Writer'), findsNothing, reason: 'its agents stay away');
    await tester.tap(find.text('Unarchive'));
    await settle(tester);
    final update = _updates(harness).single;
    expect(update.projectId, 'p2');
    expect(update.archived, isFalse);

    // Unarchived, the backend sends it back as it was.
    await harness.apply(tester, workspace());
    await settle(tester);
    expect(find.text('Archived (1)'), findsNothing);
    expect(find.text('Site'), findsOneWidget);
    expect(find.text('Writer'), findsOneWidget);
  });

  testWidgets('Archive project in its settings archives it at once', (
    tester,
  ) async {
    final harness = Harness(workspace());
    await harness.show(tester);
    await settle(tester);
    // A right click opens its settings.
    await tester.tap(find.text('Site'), buttons: 2);
    await settle(tester);
    expect(find.byType(ProjectSettingsForm), findsOneWidget);
    expect(
      find.textContaining('Hides the project and its agents from the sidebar'),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Archive project'));
    await pumpFrames(tester);
    await tester.tap(find.text('Archive project'));
    await settle(tester);
    final update = _updates(harness).single;
    expect(update.projectId, 'p2');
    expect(update.archived, isTrue);
    expect(update.hasName(), isFalse);
    expect(find.byType(ProjectSettingsForm), findsNothing);
  });

  testWidgets("on a phone, a long press offers a project's settings", (
    tester,
  ) async {
    final harness = Harness(workspace()..selectedAgentId = '');
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.longPress(find.text('Site'));
    await settle(tester);
    await tester.tap(find.text('Project settings'));
    await settle(tester);
    expect(find.byType(ProjectSettingsForm), findsOneWidget);
    expect(find.text('Archive project'), findsOneWidget);
  });

  testWidgets("an archived project's agent says so above its message box, "
      'with Unarchive', (tester) async {
    final harness = Harness(_archived(selected: 'a3'));
    await harness.show(tester);
    await settle(tester);
    expect(
      find.text(
        "Site is archived, so its agents don't work until you unarchive it. "
        'Messages you send wait until then.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Unarchive'));
    await settle(tester);
    expect(_updates(harness).single.archived, isFalse);

    await harness.apply(tester, workspace()..selectedAgentId = 'a3');
    await settle(tester);
    expect(find.textContaining('is archived'), findsNothing);
  });

  test("an agent's archived project", () {
    final snapshot = _archived();
    Agent named(String id) => snapshot.agents.firstWhere((a) => a.id == id);
    expect(archivedProjectOf(snapshot, named('a3'))?.id, 'p2');
    expect(archivedProjectOf(snapshot, named('a1')), isNull);
    expect(archivedProjectOf(snapshot, named('rivet')), isNull);
  });
}
