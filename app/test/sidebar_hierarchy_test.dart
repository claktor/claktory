// The sidebar's nesting is who reports to whom (#234): reports sit under
// their lead, a drop onto an agent makes the dragged one its report, a drop
// between rows at its own level only reorders, and the drag says which as
// it goes. VoiceOver hears "reports to", an admin lead included; a refusal
// says why; deep trees stop indenting.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  String lead = '',
  String project = 'p1',
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  leadId: lead,
  // The backend nests an agent under its lead when that's in the same
  // place: the two never disagree.
  parentId: project.isEmpty || lead == 'rivet' ? '' : lead,
  modelAvailable: true,
);

Workspace _workspace({List<Agent> extra = const []}) => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    _agent('rivet', 'Rivet', project: ''),
    _agent('mike', 'Mike', lead: 'rivet'),
    _agent('sam', 'Sam', lead: 'mike'),
    _agent('lena', 'Lena', lead: 'mike'),
    _agent('solo', 'Solo'),
    ...extra,
  ],
);

Future<Harness> _show(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Rect _row(WidgetTester tester, String name) => tester.getRect(
  find.ancestor(of: find.text(name), matching: find.byType(Row)).first,
);

/// Drags [name] to [target] and, before letting go, reads what its
/// feedback says the drop will do.
Future<String?> _drag(
  WidgetTester tester,
  String name,
  Offset target, {
  PointerDeviceKind kind = PointerDeviceKind.mouse,
}) async {
  final from = tester.getCenter(find.text(name).first);
  final gesture = await tester.startGesture(from, kind: kind);
  await tester.pump(
    kind == PointerDeviceKind.touch
        ? const Duration(milliseconds: 600)
        : const Duration(milliseconds: 50),
  );
  for (var i = 1; i <= 10; i++) {
    await gesture.moveTo(Offset.lerp(from, target, i / 10)!);
    await tester.pump(const Duration(milliseconds: 16));
  }
  String? says;
  for (final text in ['Reports to ', 'Moves to ']) {
    final found = find.textContaining(text);
    if (found.evaluate().isNotEmpty) {
      says = (tester.widget<Text>(found.first)).data;
    }
  }
  await gesture.up();
  await settle(tester);
  return says;
}

void main() {
  testWidgets("reports sit under their lead; an admin's report at the top", (
    tester,
  ) async {
    await _show(tester, _workspace());
    final mike = tester.getTopLeft(find.text('Mike')).dx;
    expect(tester.getTopLeft(find.text('Sam')).dx, greaterThan(mike));
    expect(tester.getTopLeft(find.text('Solo')).dx, mike);
  });

  testWidgets('onto an agent: its report, and the drag says so first', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    final says = await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    expect(says, 'Reports to Sam');
    final arrange = harness.last.arrangeAgent;
    expect(arrange.agentId, 'solo');
    expect(arrange.parentId, 'sam');
  });

  testWidgets('between rows at its own level: a reorder, said as nothing', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    // Lena before Sam, both Mike's.
    final says = await _drag(
      tester,
      'Lena',
      _row(tester, 'Sam').topCenter + const Offset(0, 3),
    );
    expect(says, isNull);
    final arrange = harness.last.arrangeAgent;
    expect(arrange.parentId, 'mike', reason: 'the same lead');
    expect(arrange.beforeId, 'sam');
  });

  testWidgets('between rows at another level: that level\'s lead, or none', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    // Sam before Solo, at the project's top level: out from under Mike.
    var says = await _drag(
      tester,
      'Sam',
      _row(tester, 'Solo').topCenter + const Offset(0, 3),
    );
    expect(says, 'Reports to no one');
    expect(harness.last.arrangeAgent.parentId, '');
    expect(harness.last.arrangeAgent.beforeId, 'solo');

    // Solo after Sam, among Mike's reports.
    says = await _drag(
      tester,
      'Solo',
      _row(tester, 'Sam').bottomCenter - const Offset(0, 3),
    );
    expect(says, 'Reports to Mike');
    expect(harness.last.arrangeAgent.parentId, 'mike');
  });

  testWidgets('by touch too: hold, then drag', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await _show(tester, _workspace());
    final says = await _drag(
      tester,
      'Solo',
      _row(tester, 'Lena').center,
      kind: PointerDeviceKind.touch,
    );
    expect(says, 'Reports to Lena');
    expect(harness.last.arrangeAgent.parentId, 'lena');
  });

  testWidgets("in the phone's drawer: hold, then drag onto an agent", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace()..selectedAgentId = 'solo');
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    Rect inDrawer(String name) => tester.getRect(
      find.ancestor(of: find.text(name).last, matching: find.byType(Row)).first,
    );
    final from = inDrawer('Sam').center;
    final gesture = await tester.startGesture(
      from,
      kind: PointerDeviceKind.touch,
    );
    await tester.pump(const Duration(milliseconds: 600));
    final to = inDrawer('Lena').center;
    for (var i = 1; i <= 10; i++) {
      await gesture.moveTo(Offset.lerp(from, to, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(find.text('Reports to Lena'), findsOneWidget);
    await gesture.up();
    await settle(tester);
    expect(harness.last.arrangeAgent.agentId, 'sam');
    expect(harness.last.arrangeAgent.parentId, 'lena');
  });

  testWidgets('VoiceOver hears whom each agent reports to, an admin lead too', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, _workspace());
    expect(
      find.bySemanticsLabel(RegExp(r'Sam[\s\S]*reports to Mike')),
      findsWidgets,
    );
    expect(
      find.bySemanticsLabel(RegExp(r'Mike[\s\S]*reports to Rivet')),
      findsWidgets,
    );
    expect(
      find.bySemanticsLabel(RegExp(r'Solo[\s\S]*reports to')),
      findsNothing,
    );
    semantics.dispose();
  });

  testWidgets('a lead the backend refuses says why', (tester) async {
    final harness = await _show(tester, _workspace());
    harness.backend.refuse = "Mike reports to Sam, so it can't lead it.";
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    await settle(tester);
    expect(find.text("Couldn't move Solo"), findsOneWidget);
    expect(
      find.text("Mike reports to Sam, so it can't lead it."),
      findsOneWidget,
    );
    harness.backend.refuse = null;
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.text("Couldn't move Solo"), findsNothing);
  });

  testWidgets('a deep tree stops indenting after four levels', (tester) async {
    await _show(
      tester,
      _workspace(
        extra: [
          _agent('a', 'Ann', lead: 'sam'),
          _agent('b', 'Bea', lead: 'a'),
          _agent('c', 'Cal', lead: 'b'),
          _agent('d', 'Dee', lead: 'c'),
        ],
      ),
    );
    double x(String name) => tester.getTopLeft(find.text(name)).dx;
    // Mike, Sam, Ann, Bea, Cal: each further in; Dee lines up with Cal.
    expect(x('Sam'), greaterThan(x('Mike')));
    expect(x('Ann'), greaterThan(x('Sam')));
    expect(x('Bea'), greaterThan(x('Ann')));
    expect(x('Cal'), greaterThan(x('Bea')));
    expect(x('Dee'), x('Cal'));
  });
}
