// Projects go in the order the person drags them to (friend bug #12), as
// a project's folders do: a project's row drags, and a drop on another
// project's upper or lower half puts it before or after that one
// (UpdateProject.before_id); a finger holds first, and a hold let go
// without moving is still the project's menu. VoiceOver has Move up and
// Move down. Archived projects keep their place in the backend's list.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [
    Project(id: 'p1', name: 'Alpha', path: '/w/alpha'),
    Project(id: 'p2', name: 'Beta', path: '/w/beta'),
    Project(id: 'p3', name: 'Gamma', path: '/w/gamma', archived: true),
    Project(id: 'p4', name: 'Delta', path: '/w/delta'),
  ],
  agents: [
    Agent(id: 'ana', name: 'Ana', projectId: 'p1', modelAvailable: true),
    Agent(id: 'ben', name: 'Ben', projectId: 'p2', modelAvailable: true),
    Agent(id: 'gil', name: 'Gil', projectId: 'p3', modelAvailable: true),
    Agent(id: 'dan', name: 'Dan', projectId: 'p4', modelAvailable: true),
  ],
);

Future<Harness> _show(WidgetTester tester) async {
  final harness = Harness(_workspace());
  await harness.show(tester);
  await settle(tester);
  return harness;
}

/// The project [name]'s row.
Rect _row(WidgetTester tester, String name) => tester.getRect(
  find.ancestor(of: find.text(name), matching: find.byType(Row)).first,
);

/// Drags the project [name]'s row to [target] and lets go.
Future<void> _drag(
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
  await gesture.up();
  await settle(tester);
}

void main() {
  testWidgets('Delta dragged above Alpha goes first', (tester) async {
    final harness = await _show(tester);
    await _drag(
      tester,
      'Delta',
      _row(tester, 'Alpha').topCenter + const Offset(0, 3),
    );
    final update = harness.last.updateProject;
    expect(update.projectId, 'p4');
    expect(update.beforeId, 'p1');
    expect(update.hasName(), isFalse);
    expect(update.hasArchived(), isFalse);
  });

  testWidgets("below the last project's agents: last", (tester) async {
    final harness = await _show(tester);
    await _drag(
      tester,
      'Alpha',
      tester.getBottomLeft(find.text('Dan')) + const Offset(40, 2),
    );
    expect(harness.last.updateProject.projectId, 'p1');
    expect(harness.last.updateProject.beforeId, '');
  });

  testWidgets('an archived project keeps its place', (tester) async {
    final harness = await _show(tester);
    // Under Beta: before Gamma, archived, which stays between them.
    await _drag(
      tester,
      'Delta',
      tester.getBottomLeft(find.text('Ben')) + const Offset(40, -2),
    );
    expect(harness.last.updateProject.projectId, 'p4');
    expect(harness.last.updateProject.beforeId, 'p3');
  });

  testWidgets('dropped where it already is: nothing sent', (tester) async {
    final harness = await _show(tester);
    final before = harness.backend.sent.length;
    // Beta on Alpha's lower half is where it is now.
    await _drag(
      tester,
      'Beta',
      tester.getBottomLeft(find.text('Ana')) + const Offset(40, -2),
    );
    expect(
      harness.backend.sent.skip(before).where((m) => m.hasUpdateProject()),
      isEmpty,
    );
  });

  testWidgets('a click still opens the project', (tester) async {
    final harness = await _show(tester);
    await tester.tap(find.text('Beta'));
    await settle(tester);
    expect(harness.last.focusProject.projectId, 'p2');
  });

  group('by touch', () {
    void onIOS() {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
    }

    testWidgets('hold, then drag', (tester) async {
      onIOS();
      final harness = await _show(tester);
      await _drag(
        tester,
        'Delta',
        _row(tester, 'Beta').topCenter + const Offset(0, 3),
        kind: PointerDeviceKind.touch,
      );
      expect(harness.last.updateProject.projectId, 'p4');
      expect(harness.last.updateProject.beforeId, 'p2');
    });

    testWidgets('a hold let go where it was is the menu', (tester) async {
      onIOS();
      final harness = await _show(tester);
      final gesture = await tester.startGesture(
        tester.getCenter(find.text('Beta')),
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 600));
      await gesture.up();
      await settle(tester);
      expect(find.text('Project settings'), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasUpdateProject()), isEmpty);
    });
  });

  testWidgets('VoiceOver: Move up and Move down', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await _show(tester);
    List<String?> labels(SemanticsNode node) => [
      for (final id in node.getSemanticsData().customSemanticsActionIds ?? [])
        CustomSemanticsAction.getAction(id)!.label,
    ];
    SemanticsNode node(String name) =>
        tester.getSemantics(find.text(name).first);

    final delta = node('Delta');
    expect(labels(delta), contains('Move up'));
    expect(labels(delta), isNot(contains('Move down')));
    final up = delta.getSemanticsData().customSemanticsActionIds!.firstWhere(
      (id) => CustomSemanticsAction.getAction(id)!.label == 'Move up',
    );
    delta.owner!.performAction(delta.id, SemanticsAction.customAction, up);
    await settle(tester);
    expect(harness.last.updateProject.projectId, 'p4');
    expect(harness.last.updateProject.beforeId, 'p2');

    // Alpha is first: only Move down, which puts it after Beta.
    final alpha = node('Alpha');
    expect(labels(alpha), isNot(contains('Move up')));
    final down = alpha.getSemanticsData().customSemanticsActionIds!.firstWhere(
      (id) => CustomSemanticsAction.getAction(id)!.label == 'Move down',
    );
    alpha.owner!.performAction(alpha.id, SemanticsAction.customAction, down);
    await settle(tester);
    expect(harness.last.updateProject.projectId, 'p1');
    expect(harness.last.updateProject.beforeId, 'p3');
    semantics.dispose();
  });

  testWidgets('a move the backend refuses says why', (tester) async {
    final harness = await _show(tester);
    harness.backend.refuse = 'That project no longer exists.';
    await _drag(
      tester,
      'Delta',
      _row(tester, 'Alpha').topCenter + const Offset(0, 3),
    );
    await settle(tester);
    expect(find.text("Couldn't move Delta"), findsOneWidget);
    expect(find.text('That project no longer exists.'), findsOneWidget);
    harness.backend.refuse = null;
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.text("Couldn't move Delta"), findsNothing);
  });
}
