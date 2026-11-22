// A project's folders go in the order the person drags them to (#248):
// a folder heading drags, and a drop on another's upper or lower half puts
// it before or after that one; VoiceOver has Move up and Move down. A
// project's pipes sit right under its heading, within reach however many
// agents and folders follow.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Claktory',
      path: '/w',
      folders: [
        ProjectFolder(id: 'app', name: 'App', path: '/w/app'),
        ProjectFolder(id: 'research', name: 'Research', path: '/w/research'),
        ProjectFolder(id: 'ops', name: 'Ops', path: '/w/ops'),
      ],
    ),
  ],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
    Agent(
      id: 'sam',
      name: 'Sam',
      projectId: 'p1',
      folderId: 'app',
      modelAvailable: true,
    ),
    Agent(
      id: 'iris',
      name: 'Iris',
      projectId: 'p1',
      folderId: 'research',
      modelAvailable: true,
    ),
    Agent(
      id: 'arjun',
      name: 'Arjun',
      projectId: 'p1',
      folderId: 'ops',
      modelAvailable: true,
    ),
  ],
  pipes: [
    Pipe(
      id: 'pipe1',
      name: 'Test pipe',
      projectId: 'p1',
      state: PipeState.PIPE_STATE_OPEN,
    ),
  ],
);

Future<Harness> _show(WidgetTester tester) async {
  final harness = Harness(_workspace());
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Rect _heading(WidgetTester tester, String name) => tester.getRect(
  find.ancestor(of: find.text(name), matching: find.byType(Row)).first,
);

/// Drags the heading [name] to [target] and lets go.
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
  testWidgets("a project's pipes sit right under its heading", (tester) async {
    await _show(tester);
    final pipe = tester.getTopLeft(find.text('Test pipe')).dy;
    expect(pipe, lessThan(tester.getTopLeft(find.text('Mike')).dy));
    expect(pipe, lessThan(tester.getTopLeft(find.text('App')).dy));
  });

  testWidgets('Ops dragged above App goes first', (tester) async {
    final harness = await _show(tester);
    await _drag(
      tester,
      'Ops',
      _heading(tester, 'App').topCenter + const Offset(0, 3),
    );
    final update = harness.last.updateFolder;
    expect(update.projectId, 'p1');
    expect(update.folderId, 'ops');
    expect(update.beforeId, 'app');
    expect(update.hasName(), isFalse);
    expect(update.hasPath(), isFalse);
  });

  testWidgets("below the last folder's agents: last", (tester) async {
    final harness = await _show(tester);
    // App's group ends under its agent, Sam.
    await _drag(
      tester,
      'App',
      tester.getBottomLeft(find.text('Arjun')) + const Offset(40, 2),
    );
    expect(harness.last.updateFolder.folderId, 'app');
    expect(harness.last.updateFolder.beforeId, '');
  });

  testWidgets('dropped where it already is: nothing sent', (tester) async {
    final harness = await _show(tester);
    final before = harness.backend.sent.length;
    // Research on App's lower half is where it is now.
    await _drag(
      tester,
      'Research',
      tester.getBottomLeft(find.text('Sam')) + const Offset(40, -2),
    );
    expect(
      harness.backend.sent.skip(before).where((m) => m.hasUpdateFolder()),
      isEmpty,
    );
  });

  testWidgets('by touch too: hold, then drag', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await _show(tester);
    await _drag(
      tester,
      'Ops',
      _heading(tester, 'Research').topCenter + const Offset(0, 3),
      kind: PointerDeviceKind.touch,
    );
    expect(harness.last.updateFolder.folderId, 'ops');
    expect(harness.last.updateFolder.beforeId, 'research');
  });

  testWidgets('a tap still folds the folder', (tester) async {
    final harness = await _show(tester);
    await tester.tap(find.text('Ops'));
    await settle(tester);
    expect(harness.last.hasSetFolderCollapsed(), isTrue);
    expect(harness.last.setFolderCollapsed.folderId, 'ops');
  });

  testWidgets('VoiceOver: Move up and Move down', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await _show(tester);
    final node = tester.getSemantics(
      find.bySemanticsLabel(RegExp('^Hide agents in Research')),
    );
    final labels = [
      for (final id in node.getSemanticsData().customSemanticsActionIds ?? [])
        CustomSemanticsAction.getAction(id)!.label,
    ];
    expect(labels, containsAll(['Move up', 'Move down']));
    final up = node.getSemanticsData().customSemanticsActionIds!.firstWhere(
      (id) => CustomSemanticsAction.getAction(id)!.label == 'Move up',
    );
    node.owner!.performAction(node.id, SemanticsAction.customAction, up);
    await settle(tester);
    expect(harness.last.updateFolder.folderId, 'research');
    expect(harness.last.updateFolder.beforeId, 'app');

    // App is first: only Move down.
    final first = tester.getSemantics(
      find.bySemanticsLabel(RegExp('^Hide agents in App')),
    );
    final firstLabels = [
      for (final id in first.getSemanticsData().customSemanticsActionIds ?? [])
        CustomSemanticsAction.getAction(id)!.label,
    ];
    expect(firstLabels, isNot(contains('Move up')));
    expect(firstLabels, contains('Move down'));
    semantics.dispose();
  });

  testWidgets('a move the backend refuses says why', (tester) async {
    final harness = await _show(tester);
    harness.backend.refuse = 'That folder no longer exists.';
    await _drag(
      tester,
      'Ops',
      _heading(tester, 'App').topCenter + const Offset(0, 3),
    );
    await settle(tester);
    expect(find.text("Couldn't move Ops"), findsOneWidget);
    expect(find.text('That folder no longer exists.'), findsOneWidget);
    harness.backend.refuse = null;
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.text("Couldn't move Ops"), findsNothing);
  });
}
