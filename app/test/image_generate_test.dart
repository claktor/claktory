// An agent making an image (#403): image_generate's row says what it's
// making while it works, then shows the picture, read as a preview sized
// for it (#379), which opens in the image viewer; a refusal shows the
// backend's words as they are. On a Mac and on an iPhone.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tool_views.dart' show MadeImage;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show snapshot;
import 'tool_views_test.dart' show call;
import 'viewers_test.dart' show image, png;

const _path = '/Users/me/.factory/agents/a1/images/2026-11-25/c3.png';
const _prompt = 'a red fox in a snowy field, watercolor';

Entry _made({
  ToolCallState state = ToolCallState.TOOL_CALL_STATE_DONE,
  String output = '',
  bool details = true,
}) => call(
  'image_generate',
  {'prompt': _prompt, 'size': '1536x1024'},
  seq: 3,
  state: state,
  output: output,
  details: details
      ? {
          'path': _path,
          'mime': 'image/png',
          'width': 1536,
          'height': 1024,
          'bytes': 2400000,
          'model': 'gpt-image-2',
          'size': '1536x1024',
          'quality': 'auto',
          'prompt': _prompt,
        }
      : null,
);

Future<Harness> _show(
  WidgetTester tester,
  Entry entry, {
  bool phone = false,
}) async {
  if (phone) {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  }
  final harness = Harness(snapshot());
  harness.backend
    ..appliesLayouts = true
    // The backend's preview for the thumbnail, and the original.
    ..previews[_path] = image(_path, await png(tester, 48, 32))
    ..files[_path] = image(_path, await png(tester, 1536, 1024));
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: [entry]));
  await harness.show(
    tester,
    size: phone ? const Size(393, 852) : const Size(1200, 900),
  );
  await pumpFrames(tester);
  // Its bytes decode off the test's clock.
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 50)),
  );
  await pumpFrames(tester);
  return harness;
}

Iterable<ReadFile> _reads(Harness harness) => harness.backend.sent
    .where((message) => message.hasReadFile())
    .map((message) => message.readFile);

void main() {
  testWidgets('while it works: what it is making, and how long it can take, '
      'nothing read yet', (tester) async {
    final harness = await _show(
      tester,
      _made(state: ToolCallState.TOOL_CALL_STATE_RUNNING, details: false),
    );
    expect(find.text('Making an image $_prompt', findRichText: true), findsOne);
    expect(find.text('It can take up to 2 minutes.'), findsOneWidget);
    expect(find.byType(MadeImage), findsNothing);
    expect(_reads(harness), isEmpty);
    expect(find.text('image_generate'), findsNothing, reason: 'its own row');
  });

  testWidgets('made: the picture, read as a preview its size, in its shape; '
      'a click opens it in the image viewer', (tester) async {
    final harness = await _show(tester, _made());
    expect(find.text('Made an image $_prompt', findRichText: true), findsOne);
    expect(find.text('It can take up to 2 minutes.'), findsNothing);
    final read = _reads(harness).single;
    expect(read.path, _path);
    // A preview no bigger than the thumbnail, in this screen's pixels.
    final ratio = tester.view.devicePixelRatio;
    expect(read.previewWidth, (MadeImage.side * ratio).round());
    expect(read.previewHeight, (MadeImage.side * ratio).round());
    final shown = find.descendant(
      of: find.byType(MadeImage),
      matching: find.byType(Image),
    );
    expect(shown, findsOneWidget);
    // 1536×1024 fitted in 320 points: 320 wide, its shape kept.
    final box = tester.getSize(
      find.descendant(
        of: find.byType(MadeImage),
        matching: find.byType(AspectRatio),
      ),
    );
    expect(box.width, MadeImage.side);
    expect(box.height, closeTo(MadeImage.side * 1024 / 1536, 0.5));

    await tester.tap(find.byType(MadeImage));
    await pumpFrames(tester);
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(layout.setLayout.agentId, 'a1');
    final leaves = <ViewLeaf>[];
    void walk(ViewNode node) {
      if (node.hasLeaf()) leaves.add(node.leaf);
      node.split.children.forEach(walk);
    }

    walk(layout.setLayout.root);
    expect(leaves.where((leaf) => leaf.path == _path), hasLength(1));
  });

  testWidgets("refused: the backend's words as they are, no picture", (
    tester,
  ) async {
    const refusal =
        "OpenAI declined to make this image: the description didn't pass "
        'its content rules. Change it and try again.';
    final harness = await _show(
      tester,
      _made(
        state: ToolCallState.TOOL_CALL_STATE_FAILED,
        output: refusal,
        details: false,
      ),
    );
    expect(
      find.text("Couldn't make an image $_prompt", findRichText: true),
      findsOne,
    );
    expect(find.text(refusal), findsOneWidget);
    expect(find.byType(MadeImage), findsNothing);
    expect(_reads(harness), isEmpty);
  });

  testWidgets('a picture that is gone says why, in place of it', (
    tester,
  ) async {
    // Another path: pictures read already are kept for the session.
    const gone = '/Users/me/.factory/agents/a1/images/2026-11-25/c9.png';
    final entry = _made();
    entry.toolCall.details = entry.toolCall.details.replaceAll(_path, gone);
    final harness = Harness(snapshot());
    harness.backend.transcript.load(
      Transcript(agentId: 'a1', entries: [entry]),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('No such file: $gone'), findsOneWidget);
    expect(find.byType(Image), findsNothing);
  });

  testWidgets('on an iPhone: the picture fits the conversation, and a tap '
      'opens it as a page', (tester) async {
    final harness = await _show(tester, _made(), phone: true);
    final shown = find.byType(MadeImage);
    expect(shown, findsOneWidget);
    expect(tester.getRect(shown).right, lessThanOrEqualTo(393));
    await tester.tap(shown);
    await pumpFrames(tester);
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(layout.setLayout.root.toString(), contains(_path));
    // The viewer's page over the conversation shows the original.
    expect(
      find.text('Made an image $_prompt', findRichText: true),
      findsNothing,
    );
  });
}
