// The way into People while it's a preview: a People button in the Mac's
// sidebar footer and a People row in the phone's drawer, only where the
// backend offers direct messages (its `dm` extension, FACTORY_DMS_PREVIEW),
// never otherwise.

import 'package:factory_app/people/people_view.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show menuButton;

Workspace _workspace({required bool dms}) => Workspace(
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
  ],
  selectedAgentId: 'rivet',
  extensions: [
    Extension(id: 'memory', name: 'Memory'),
    if (dms) Extension(id: 'dm', name: 'Direct messages'),
  ],
);

void main() {
  for (final dms in [false, true]) {
    testWidgets('on a Mac, People ${dms ? 'shows and opens' : "doesn't show"}'
        ' with direct messages ${dms ? 'on' : 'off'}', (tester) async {
      final harness = Harness(_workspace(dms: dms));
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel(RegExp(r'^More$')).first);
      await pumpFrames(tester);
      final people = find.text('People');
      expect(people, dms ? findsOneWidget : findsNothing);
      if (!dms) return;
      await tester.tap(people);
      await pumpFrames(tester);
      expect(find.byType(PeopleView), findsOneWidget);
    });

    testWidgets('on a phone, the drawer ${dms ? 'has' : "hasn't"} People '
        'with direct messages ${dms ? 'on' : 'off'}', (tester) async {
      final harness = Harness(_workspace(dms: dms));
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      await tester.tap(menuButton());
      await pumpFrames(tester);
      final row = find.descendant(
        of: find.byType(Sidebar),
        matching: find.text('People'),
      );
      if (dms) {
        await tester.scrollUntilVisible(
          row,
          200,
          scrollable: find
              .descendant(
                of: find.byType(Sidebar),
                matching: find.byType(Scrollable),
              )
              .first,
        );
      }
      expect(row, dms ? findsOneWidget : findsNothing);
    });
  }
}
