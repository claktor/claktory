// A project's Files with a finger on an iPad's wide layouts (#540, Sale):
// its touch and hold offers Files, and the focused project's heading keeps
// its Files button in sight, both opening a pane beside the conversation
// as the Mac's hover button does.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../files_test.dart' show filesButtons, layoutsSent, snapshot;
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
        'opened beside the conversation', (tester) async {
      final harness = await _show(tester, size);
      await tester.longPress(find.text('Site'));
      await pumpFrames(tester);
      expect(find.text('Project settings'), findsOneWidget);
      await tester.tap(find.text('Files'));
      await pumpFrames(tester);
      _expectFilesOf(harness, 'p2');
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
    expect(find.text('Project settings'), findsOneWidget);
    await tester.tap(find.text('Files'));
    await pumpFrames(tester);
    _expectFilesOf(harness, 'p1');
  });
}
