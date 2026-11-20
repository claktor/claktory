import 'package:factory_app/controls.dart' show Field, Select, showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'map_fixture.dart';

/// Matt's Claktory with its leads, Rivet an admin agent.
Workspace _leads() {
  final workspace = claktoryWorkspace(leads: true);
  final rivet = workspace.agents.firstWhere((agent) => agent.id == 'rivet');
  rivet.extensions
      .firstWhere((extension) => extension.id == 'agents')
      .options['admin'] = OptionValue(
    on: true,
  );
  return workspace;
}

/// Opens [id]'s settings, scrolled to its Lead.
Future<Harness> _settings(WidgetTester tester, String id) async {
  final workspace = _leads();
  final harness = Harness(workspace);
  await harness.show(tester, size: const Size(1200, 1100));
  await settle(tester);
  final agent = workspace.agents.firstWhere((agent) => agent.id == id);
  showModal(
    tester.element(find.text('Mike').first),
    (context, close) => AgentSettingsForm(agent: agent, close: close),
  );
  await settle(tester);
  await tester.ensureVisible(find.text('Lead'));
  await settle(tester);
  return harness;
}

/// The Lead field's select.
Finder _lead() => find.descendant(
  of: find.ancestor(of: find.text('Lead'), matching: find.byType(Field)),
  matching: find.byType(Select<String>),
);

void main() {
  test('a lead is another agent in its project or an admin agent, never one '
      'that reports to it', () {
    final workspace = _leads();
    List<String> candidates(String id, String project) => [
      for (final agent in leadCandidates(workspace, id, project)) agent.id,
    ];
    // Mike leads the Claktory project, so none of it can lead him; Rivet,
    // an admin agent elsewhere, can.
    expect(candidates('coordinator', 'factory'), ['rivet']);
    // Priya can report to anyone in the project, Mike included, or Rivet.
    final priya = candidates('frontend', 'factory');
    expect(priya, contains('coordinator'));
    expect(priya, contains('frontend-2'));
    expect(priya, isNot(contains('frontend')));
    expect(priya, isNot(contains('disk-scout')));
    expect(priya.last, 'rivet');
    // Moved to Disk: Disk Scout or Rivet.
    expect(candidates('frontend', 'disk'), ['disk-scout', 'rivet']);
  });

  testWidgets('Lead shows the lead and sends a new one, or None', (
    tester,
  ) async {
    final harness = await _settings(tester, 'frontend');
    final select = tester.widget<Select<String>>(_lead());
    expect(select.value, 'coordinator');
    expect([
      for (final option in select.options) option.label,
    ], containsAllInOrder(['None', 'Mike', 'Kenji', 'Rivet (Admin)']));
    expect(
      find.text(
        "Who this agent reports to. Its lead can see how it's "
        'set up.',
      ),
      findsOneWidget,
    );

    select.onChanged('frontend-2');
    await settle(tester);
    await tester.ensureVisible(find.text('Save'));
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.agentId, 'frontend');
    expect(update.leadId, 'frontend-2');
    expect(update.hasName(), isFalse);
    expect(update.hasProjectId(), isFalse);
  });

  testWidgets('None clears it', (tester) async {
    final harness = await _settings(tester, 'frontend');
    tester.widget<Select<String>>(_lead()).onChanged('');
    await settle(tester);
    await tester.ensureVisible(find.text('Save'));
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.hasLeadId(), isTrue);
    expect(harness.last.updateAgent.leadId, '');
  });

  testWidgets('moving to another project drops a lead that no longer fits', (
    tester,
  ) async {
    final harness = await _settings(tester, 'frontend');
    // Moved to Disk, where Mike can't lead it: Lead shows None.
    final project = find.descendant(
      of: find.ancestor(of: find.text('Project'), matching: find.byType(Field)),
      matching: find.byType(Select<String>),
    );
    tester.widget<Select<String>>(project).onChanged('disk');
    await settle(tester);
    expect(tester.widget<Select<String>>(_lead()).value, '');
    await tester.ensureVisible(find.text('Save'));
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.projectId, 'disk');
    expect(update.leadId, '');
  });

  testWidgets("the backend's refusal shows as it comes", (tester) async {
    final harness = await _settings(tester, 'frontend');
    harness.backend.refuse =
        "Priya can't report to Kenji, because Kenji already reports to Priya";
    tester.widget<Select<String>>(_lead()).onChanged('frontend-2');
    await settle(tester);
    await tester.ensureVisible(find.text('Save'));
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(
      find.textContaining("Kenji already reports to Priya"),
      findsOneWidget,
    );
  });
}
