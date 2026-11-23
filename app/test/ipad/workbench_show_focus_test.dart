// What an agent shows comes into sight on an iPad (Zucchero's row 111): in
// a window that shows fewer panes than the layout has, a pane the agent's
// show adds, or puts in another's place, becomes the one in front, so it
// never lands among the hidden ones ("Not shown") while the agent is told
// it's showing. Nothing of this is the agent's layout: no SetLayout is sent.
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, layoutsSent, row, snapshot, split;
import 'as_ipad.dart';

Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

/// An iPad mini upright: one pane at a time beside the rail.
const _mini = Size(744, 1133);

Future<(Harness, WorkbenchFocus)> _usageInFront(WidgetTester tester) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(
    snapshot(
      layout: split('s1', row, [
        leaf('chat', 'chat'),
        leaf('set', 'settings'),
        leaf('use', 'usage'),
      ]),
    ),
  );
  await harness.show(tester, size: _mini, home: _window(focus));
  await pumpFrames(tester);
  focus.focus('a1', 'use');
  await pumpFrames(tester);
  return (harness, focus);
}

void main() {
  testWidgets("a file the agent's show adds comes into sight", (tester) async {
    final (harness, focus) = await _usageInFront(tester);
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('set', 'settings'),
      leaf('use', 'usage'),
      leaf('pdf', 'pdf', '/w/field-guide.pdf'),
    ]);
    await harness.apply(tester, snapshot(layout: layout));
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'pdf');
    expect(focus.projectionOf('a1')?.hidden, isNot(contains('pdf')));
    expect(layoutsSent(harness), isEmpty);
  });

  testWidgets('a file shown in place of the oldest pane comes into sight', (
    tester,
  ) async {
    final (harness, focus) = await _usageInFront(tester);
    // Settings, opened first, gives its place (and its id) to the file.
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('set', 'pdf', '/w/field-guide.pdf'),
      leaf('use', 'usage'),
    ]);
    await harness.apply(tester, snapshot(layout: layout));
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'set');
    expect(focus.projectionOf('a1')?.hidden, isNot(contains('set')));
  });

  testWidgets('the layout changing without a new pane keeps the focus', (
    tester,
  ) async {
    final (harness, focus) = await _usageInFront(tester);
    // The same panes, the other way round.
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('use', 'usage'),
      leaf('set', 'settings'),
    ]);
    await harness.apply(tester, snapshot(layout: layout));
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'use');
  });

  testWidgets('while a stroke is down on a document, it stays in front', (
    tester,
  ) async {
    final (harness, focus) = await _usageInFront(tester);
    final drawing = focus.documents.drawing as ValueNotifier<bool>;
    drawing.value = true;
    addTearDown(() => drawing.value = false);
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('set', 'settings'),
      leaf('use', 'usage'),
      leaf('pdf', 'pdf', '/w/field-guide.pdf'),
    ]);
    await harness.apply(tester, snapshot(layout: layout));
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'use');
  });

  testWidgets("a pane already open, shown again by the agent, comes into "
      'sight (Zucchero\'s row 111, the show of a hidden pane)', (tester) async {
    final (harness, focus) = await _usageInFront(tester);
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('set', 'settings'),
      leaf('use', 'usage'),
    ]);
    expect(focus.projectionOf('a1')?.hidden, contains('set'));
    // What the backend sends for a show of a file already open: it points
    // at the pane, even with highlights off.
    harness.backend.highlights.apply(
      Highlight(
        targets: [HighlightTarget(kind: 'pane', id: 'a1:set')],
      ),
    );
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'set');
    expect(focus.projectionOf('a1')?.hidden, isNot(contains('set')));
    expect(layoutsSent(harness), isEmpty);
    // Its callout's time runs out.
    await tester.pump(const Duration(seconds: 6));
  });

  testWidgets("a pane's glow here (its show card tapped) brings it to the "
      'front; another agent\'s pane changes nothing', (tester) async {
    final (harness, focus) = await _usageInFront(tester);
    final layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('set', 'settings'),
      leaf('use', 'usage'),
    ]);
    await harness.apply(tester, snapshot(layout: layout)..highlights = true);
    await pumpFrames(tester);
    final color = Palette.of(tester.element(find.byType(Shell))).signal;
    harness.backend.highlights.glow('pane:someone:set', [color]);
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'use');
    harness.backend.highlights.glow('pane:a1:set', [color]);
    await pumpFrames(tester);
    expect(focus.focusedIn('a1', layout), 'set');
    await tester.pump(const Duration(seconds: 2));
  });
}
