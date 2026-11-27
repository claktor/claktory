// On a phone, the files an agent has open, under Files in its
// ⋯, newest first: the one it most recently showed, or the person most
// recently opened, on top. Panes from before the backend kept the time
// (0) keep their layout order, after the rest.
import 'package:factory_app/mobile/extension_list.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

ViewNode _file(String name, int openedMs) => ViewNode(
  id: name,
  leaf: ViewLeaf(
    kind: 'text',
    path: '/w/$name',
    title: name,
    openedMs: Int64(openedMs),
  ),
);

void main() {
  test('newest first; equal times keep their layout order', () {
    final panes = [
      _file('a.md', 0),
      _file('b.md', 300),
      _file('c.md', 100),
      _file('d.md', 0),
      _file('e.md', 300),
    ];
    expect(
      [for (final pane in newestFirst(panes)) pane.id],
      ['b.md', 'e.md', 'c.md', 'a.md', 'd.md'],
    );
  });

  testWidgets('the ⋯ lists the file shown last first', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final snapshot = workspace();
    snapshot.agents.first.layout = ViewNode(
      id: 's1',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          _file('old.md', 0),
          _file('shown-last.md', 2000),
          _file('opened-before.md', 1000),
        ],
      ),
    );
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
    await pumpFrames(tester);
    expect(find.text('Files'), findsOneWidget);
    double top(String name) => tester.getTopLeft(find.text(name)).dy;
    expect(top('shown-last.md'), lessThan(top('opened-before.md')));
    expect(top('opened-before.md'), lessThan(top('old.md')));

    // An agent shows the old one again: it comes to the top.
    final again = snapshot.deepCopy();
    again.agents.first.layout.split.children[1].leaf.openedMs = Int64(3000);
    harness.backend.workspace.apply(again);
    await pumpFrames(tester);
    expect(top('old.md'), lessThan(top('shown-last.md')));
    expect(tester.takeException(), isNull);
  });

  testWidgets('a file opened from the ⋯ is told to the backend, so it comes '
      'to the top on every device (row 243)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final snapshot = workspace();
    final agentId = snapshot.agents.first.id;
    snapshot.agents.first.layout = ViewNode(
      id: 's1',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          _file('old.md', 0),
          _file('shown-last.md', 2000),
        ],
      ),
    );
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
    await pumpFrames(tester);
    harness.backend.sent.clear();
    await tester.tap(find.text('old.md'));
    await pumpFrames(tester);
    final opened = [
      for (final message in harness.backend.sent)
        if (message.hasPaneOpened()) message.paneOpened,
    ];
    expect(opened, hasLength(1));
    expect(opened.single.agentId, agentId);
    expect(opened.single.paneId, 'old.md');
    expect(tester.takeException(), isNull);
  });
}
