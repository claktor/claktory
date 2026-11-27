// The Map on an iPad, where a finger comes first: a tap
// on an agent shows its card with Close and Open conversation, as on a
// phone, rather than leaving for its conversation; a tap on the empty
// board lets it go; a drag pans without letting go; the key says "Tap".
// The Mac's click still opens the conversation (map_test).
import 'package:factory_app/map.dart';
import 'package:factory_app/map_card.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../map_fixture.dart';
import 'as_ipad.dart';

const _ipad = Size(1366, 1024);

Future<Harness> _showMap(WidgetTester tester) async {
  asIPad();
  final harness = Harness(claktoryWorkspace());
  await harness.show(tester, size: _ipad);
  harness.page.value = AppPage.map;
  await pumpFrames(tester);
  return harness;
}

Finder _chip(String name) =>
    find.descendant(of: find.byType(MapView), matching: find.text(name));

/// A point on the board with nothing on it: just inside its bottom right.
Offset _emptyBoard(WidgetTester tester) =>
    tester.getRect(find.byType(MapView)).bottomRight - const Offset(24, 24);

void main() {
  testWidgets('a tap on an agent shows its card; the empty board lets it go', (
    tester,
  ) async {
    final harness = await _showMap(tester);
    expect(
      find.textContaining('Tap an agent to see everything it has.'),
      findsOneWidget,
    );
    expect(find.textContaining('Hover an agent'), findsNothing);

    // Panned first: showing a card keeps the board where it is.
    await tester.dragFrom(_emptyBoard(tester), const Offset(-60, -40));
    await settle(tester);
    final at = tester.getCenter(_chip('Mike').first);
    await tester.tap(_chip('Mike').first);
    await settle(tester);
    expect(tester.getCenter(_chip('Mike').first), at);
    // Still on the Map, with Mike's card and its buttons.
    expect(harness.page.value, AppPage.map);
    expect(find.byType(MapAgentCard), findsOneWidget);
    expect(find.text('Open conversation'), findsOneWidget);
    expect(find.text('Close'), findsOneWidget);

    // A drag pans the board and keeps the card.
    await tester.dragFrom(_emptyBoard(tester), const Offset(-120, -80));
    await settle(tester);
    expect(find.byType(MapAgentCard), findsOneWidget);

    // A tap on nothing lets it go.
    await tester.tapAt(_emptyBoard(tester));
    await settle(tester);
    expect(find.byType(MapAgentCard), findsNothing);
    expect(harness.page.value, AppPage.map);
  });

  testWidgets('a tap on another agent shows its card instead; Open '
      'conversation opens it', (tester) async {
    final harness = await _showMap(tester);
    await tester.tap(_chip('Mike').first);
    await settle(tester);
    final mike = tester.widget<MapAgentCard>(find.byType(MapAgentCard));

    await tester.tap(_chip('Sam').first);
    await settle(tester);
    expect(find.byType(MapAgentCard), findsOneWidget);
    final sam = tester.widget<MapAgentCard>(find.byType(MapAgentCard));
    expect(sam.details.agent.id, isNot(mike.details.agent.id));
    expect(harness.page.value, AppPage.map);

    await tester.tap(find.text('Open conversation'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'builder');
    expect(harness.page.value, AppPage.agent);
  });
}
