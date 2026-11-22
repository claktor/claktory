// Settings → Checklist: when Claktory follows up on blocked checklist items
// (Settings.checklist), each a choice of durations; a change sends only its
// own field, and a refusal shows under them. A backend without them shows
// no Checklist section.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/widgets.dart' show Size;
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

Future<Harness> _show(WidgetTester tester, {bool checklist = true}) async {
  final snapshot = workspace();
  if (checklist) {
    snapshot.settings.checklist = ChecklistSettings(
      agentBlockedMinutes: 240,
      personBlockedMinutes: 1440,
      externalRetryMinutes: 120,
      reaskMinutes: 1440,
      // Not one of the choices: offered as it is.
      overdueMinutes: 90,
    );
  }
  final harness = Harness(snapshot);
  await harness.show(tester, size: const Size(1200, 1600));
  harness.page.value = AppPage.settings;
  await settle(tester);
  return harness;
}

void main() {
  test('durations in the largest unit they divide into', () {
    expect(minutesLabel(15), '15 minutes');
    expect(minutesLabel(60), '1 hour');
    expect(minutesLabel(90), '90 minutes');
    expect(minutesLabel(240), '4 hours');
    expect(minutesLabel(1440), '1 day');
    expect(minutesLabel(10080), '7 days');
  });

  testWidgets('each interval in effect, changed one at a time', (tester) async {
    final harness = await _show(tester);
    await reveal(tester, find.text('Overdue'));
    expect(find.text('Waiting for an agent'), findsOneWidget);
    expect(find.text('4 hours'), findsOneWidget);
    expect(find.text('2 hours'), findsOneWidget);
    expect(find.text('1 day'), findsNWidgets(2));
    expect(find.text('90 minutes'), findsOneWidget);

    await tester.ensureVisible(find.text('4 hours'));
    await settle(tester);
    await tester.tap(find.text('4 hours'));
    await settle(tester);
    await tester.tap(find.text('8 hours').last);
    await settle(tester);
    expect(
      harness.last.updateSettings,
      UpdateSettings(checklist: ChecklistSettings(agentBlockedMinutes: 480)),
    );

    harness.backend.refuse = 'Choose from 15 minutes to 7 days.';
    await tester.ensureVisible(find.text('90 minutes'));
    await settle(tester);
    await tester.tap(find.text('90 minutes'));
    await settle(tester);
    await tester.tap(find.text('30 minutes').last);
    await settle(tester);
    expect(
      harness.last.updateSettings,
      UpdateSettings(checklist: ChecklistSettings(overdueMinutes: 30)),
    );
    expect(find.text('Choose from 15 minutes to 7 days.'), findsOneWidget);
  });

  testWidgets('a backend without them: no Checklist section', (tester) async {
    await _show(tester, checklist: false);
    expect(find.text('Waiting for an agent'), findsNothing);
    expect(find.byType(ChecklistTimes), findsNothing);
  });
}
