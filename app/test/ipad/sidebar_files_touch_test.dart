// A project's Files with a finger on an iPad's wide layouts:
// its touch and hold offers Files, and the focused project's heading keeps
// its Files button in sight, both opening a pane beside the conversation
// as the Mac's hover button does (a page of their own when the agent shown
// is in another project).
import 'package:factory_app/explorer.dart' show FilesPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../files_test.dart'
    show filesButtons, layoutsSent, snapshot, withFilesOf;
import '../harness.dart';
import 'as_ipad.dart';

Future<Harness> _show(
  WidgetTester tester,
  Size size, {
  String focused = '',
}) async {
  asIPad();
  final harness = Harness(snapshot(focused: focused));
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

void _expectFilesOf(Harness harness, String projectId) {
  final sent = layoutsSent(harness);
  expect(sent, hasLength(1), reason: 'a pane beside the conversation');
  expect(sent.single.setLayout.agentId, 'a1');
  expect(
    sent.single.setLayout.root.split.children.last.leaf,
    isA<ViewLeaf>()
        .having((leaf) => leaf.kind, 'kind', 'files')
        .having((leaf) => leaf.path, 'path', projectId),
  );
}

void main() {
  for (final size in const [Size(1194, 834), Size(809, 834)]) {
    testWidgets("at $size, a project's touch and hold offers its Files, "
        'opened beside the conversation of an agent in it, else as a page '
        '(projects-sidebar.md §6)', (tester) async {
      final harness = await _show(tester, size);
      await tester.longPress(find.text('Factory').first);
      await pumpFrames(tester);
      expect(find.text('Project settings…'), findsOneWidget);
      await tester.tap(find.text('Files'));
      await pumpFrames(tester);
      _expectFilesOf(harness, 'p1');
      expect(find.byType(FilesPage), findsNothing);

      await tester.longPress(find.text('Site'));
      await pumpFrames(tester);
      await tester.tap(find.text('Files'));
      await pumpFrames(tester);
      expect(find.byType(FilesPage), findsOneWidget);
      expect(layoutsSent(harness), hasLength(1), reason: 'no pane for it');
    });
  }

  testWidgets("the focused project's heading shows its Files button without "
      'hover, and a tap on it opens them', (tester) async {
    final harness = await _show(tester, const Size(1194, 834), focused: 'p1');
    expect(filesButtons(), findsOneWidget);
    await tester.tap(filesButtons());
    await pumpFrames(tester);
    _expectFilesOf(harness, 'p1');
  });

  testWidgets("the focused project's heading, tapped, offers Files too", (
    tester,
  ) async {
    final harness = await _show(tester, const Size(1194, 834), focused: 'p1');
    await tester.tap(find.text('Factory'));
    await pumpFrames(tester);
    expect(find.text('Project settings…'), findsOneWidget);
    await tester.tap(find.text('Files'));
    await pumpFrames(tester);
    _expectFilesOf(harness, 'p1');
  });

  testWidgets("the heading's Files button takes a finger's tap around it, "
      'a finger tall and wide, looking as it did', (tester) async {
    final harness = await _show(tester, const Size(1194, 834), focused: 'p1');
    final button = tester.getRect(filesButtons());
    expect(button.size, const Size(20, 20), reason: 'as it looks');
    // 28 points before its middle and 11 below: off the glyph, within 44
    // wide, which reaches back before it as the row ends 4 points after it.
    await tester.tapAt(button.center + const Offset(-28, 11));
    await pumpFrames(tester);
    _expectFilesOf(harness, 'p1');
  });

  testWidgets("the Projects heading's + takes a finger's tap around it", (
    tester,
  ) async {
    final harness = await _show(tester, const Size(1194, 834));
    final plus = find.bySemanticsLabel('New project').first;
    final rect = tester.getRect(plus);
    expect(rect.size, const Size(20, 20));
    // At the row's end: all 44 before it, 32 before its middle.
    await tester.tapAt(rect.center + const Offset(-32, 11));
    await pumpFrames(tester);
    // Rivet's conversation, the words waiting in its message box.
    expect(
      harness.backend.sent
          .where((m) => m.hasSelectAgent())
          .map((m) => m.selectAgent.agentId),
      contains('rivet'),
    );
    // Rivet's highlight fades.
    await tester.pump(const Duration(seconds: 6));
  });

  testWidgets('with a pointer alone (a Mac), only the button itself', (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: withFilesOf('p1'), focused: 'p1'));
    await harness.show(tester, size: const Size(1194, 834));
    await pumpFrames(tester);
    // Open, the glyph that closes them.
    final button = tester.getRect(find.bySemanticsLabel('Hide files'));
    await tester.tapAt(button.center + const Offset(-18, 11));
    await pumpFrames(tester);
    expect(layoutsSent(harness), isEmpty);
    expect(
      harness.backend.highlights.glows.map((glow) => glow.key),
      isNot(contains('pane:a1:f1')),
    );
  });
}
