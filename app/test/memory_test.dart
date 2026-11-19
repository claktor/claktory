import 'package:factory_app/controls.dart';
import 'package:factory_app/memory.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final memoryExtension = Extension(
  id: 'memory',
  name: 'Memory',
  description: 'Its own preferences and notes.',
  icon: 'brain',
);

Workspace workspace() => Workspace(
  agents: [
    Agent(
      id: 'a1',
      name: 'Builder',
      modelAvailable: true,
      extensions: [AgentExtension(id: 'memory', enabled: true)],
      openPanel: 'memory',
    ),
  ],
  extensions: [memoryExtension],
  selectedAgentId: 'a1',
  openPanel: 'memory',
);

final now = DateTime.now();
Int64 at(DateTime time) => Int64(time.millisecondsSinceEpoch);

ExtensionView memoryView(Memory memory) => ExtensionView(
  extensionId: 'memory',
  agentId: 'a1',
  title: 'Memory',
  memory: memory,
);

Memory memory() {
  final today = DateTime(now.year, now.month, now.day, 0, 10);
  final yesterday = today.subtract(const Duration(days: 1));
  return Memory(
    preferences: [
      MemoryPreference(
        id: 1,
        rule: 'Keep replies short',
        why: 'The person reads them between meetings',
        learned: at(yesterday),
        updated: at(today),
      ),
      MemoryPreference(id: 2, rule: 'Run the tests', learned: at(today)),
    ],
    notes: [
      MemoryNote(
        slug: 'deploy-steps',
        summary: 'How to ship a release',
        body: 'Run **release.sh**, then wait for the banner.',
        created: at(yesterday),
        updated: at(yesterday),
      ),
    ],
    changes: [
      MemoryChange(
        time: at(today),
        actor: 'a1',
        kind: MemoryChangeKind.MEMORY_CHANGE_KIND_PREFERENCE_CHANGED,
        preferenceId: 1,
        text: 'Keep replies short',
      ),
      MemoryChange(
        time: at(yesterday),
        kind: MemoryChangeKind.MEMORY_CHANGE_KIND_NOTE_WRITTEN,
        noteSlug: 'deploy-steps',
        text: 'How to ship a release',
      ),
    ],
  );
}

void main() {
  Future<Harness> show(WidgetTester tester, Memory memory) async {
    final harness = Harness(workspace());
    harness.backend.extensionView.apply(memoryView(memory));
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('preferences show their rule, why and when', (tester) async {
    await show(tester, memory());
    expect(find.text('Preferences'), findsOneWidget);
    expect(find.text('#1'), findsOneWidget);
    expect(find.text('Keep replies short'), findsOneWidget);
    expect(
      find.text('Why: The person reads them between meetings'),
      findsOneWidget,
    );
    expect(find.text('Learned yesterday · changed today'), findsOneWidget);
    expect(find.text('Learned today'), findsOneWidget);
  });

  testWidgets('a note opens to its body', (tester) async {
    await show(tester, memory());
    expect(find.text('deploy-steps'), findsOneWidget);
    expect(find.text('How to ship a release'), findsOneWidget);
    expect(find.textContaining('release.sh'), findsNothing);

    await tester.tap(find.text('deploy-steps'));
    await settle(tester);
    expect(find.textContaining('release.sh'), findsOneWidget);
    expect(find.text('Written yesterday'), findsOneWidget);
  });

  testWidgets('a read-only memory is only its notes', (tester) async {
    await show(
      tester,
      Memory(
        readOnly: true,
        notes: [
          MemoryNote(
            slug: 'monitors',
            summary: 'How monitors wake agents',
            body: 'A monitor is a prompt plus **what it waits for**.',
          ),
        ],
      ),
    );
    expect(find.text('Preferences'), findsNothing);
    expect(find.text('No preferences yet.'), findsNothing);
    expect(find.text('Recent changes'), findsNothing);
    expect(find.text('How monitors wake agents'), findsOneWidget);
    await tester.tap(find.text('monitors'));
    await settle(tester);
    expect(find.textContaining('what it waits for'), findsOneWidget);
    expect(find.textContaining('Written'), findsNothing);
  });

  testWidgets('an empty read-only memory says so', (tester) async {
    await show(tester, Memory(readOnly: true));
    expect(find.text('Nothing in it.'), findsOneWidget);
  });

  testWidgets('recent changes say who did what', (tester) async {
    await show(tester, memory());
    expect(find.text('Recent changes'), findsOneWidget);
    expect(
      find.textContaining('Builder changed preference #1: Keep replies short'),
      findsOneWidget,
    );
    expect(
      find.textContaining('You wrote note deploy-steps: How to ship a release'),
      findsOneWidget,
    );
  });

  testWidgets('an empty memory says so', (tester) async {
    await show(tester, Memory());
    expect(find.text('Nothing remembered yet.'), findsOneWidget);
  });

  testWidgets('an empty section says so', (tester) async {
    await show(tester, Memory(notes: memory().notes));
    expect(find.text('No preferences yet.'), findsOneWidget);
    expect(find.text('Recent changes'), findsNothing);
  });

  test('every kind of change reads as a sentence', () {
    expect(
      changeSummary(
        MemoryChange(
          kind: MemoryChangeKind.MEMORY_CHANGE_KIND_NOTE_REMOVED,
          noteSlug: 'old',
        ),
      ),
      'removed note old',
    );
    expect(
      changeSummary(
        MemoryChange(
          kind: MemoryChangeKind.MEMORY_CHANGE_KIND_PREFERENCE_ADDED,
          preferenceId: 3,
          text: 'Ask first',
        ),
      ),
      'added preference #3: Ask first',
    );
  });

  test('Lucide paths parse into the 24-unit grid', () {
    /// The path's true extent, from points along it (getBounds counts
    /// curves' control points too).
    Rect extent(Path path) {
      var rect = Rect.zero;
      var first = true;
      for (final metric in path.computeMetrics()) {
        for (var d = 0.0; d <= metric.length; d += 0.05) {
          final point = metric.getTangentForOffset(d)!.position;
          final dot = Rect.fromLTWH(point.dx, point.dy, 0, 0);
          rect = first ? dot : rect.expandToInclude(dot);
          first = false;
        }
      }
      return rect;
    }

    final brain = extent(
      svgPath(
        'M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588'
        'A4 4 0 1 0 12 18Z',
      ),
    );
    expect(brain.left, closeTo(2, 1));
    expect(brain.right, closeTo(12, 0.01));
    expect(brain.top, closeTo(2, 0.5));
    expect(brain.bottom, closeTo(22, 0.5));
    final lines = extent(svgPath('M3 7h4v2H3z'));
    expect(lines.left, closeTo(3, 0.01));
    expect(lines.right, closeTo(7, 0.01));
    expect(lines.bottom, closeTo(9, 0.01));
  });
}
