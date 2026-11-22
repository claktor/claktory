// Copy on a phone (a message's touch and hold) puts on the clipboard what
// shows: an agent's Markdown without its markers, the way a selection of
// the rendered text copies; the person's own message as they typed it.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'selection_test.dart' show watchClipboard;

void main() {
  Future<List<String>> show(WidgetTester tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final copied = watchClipboard(tester);
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(text: 'Keep **these** stars, please'),
          ),
          Entry(
            seq: Int64(2),
            assistant: AssistantText(
              text: 'The **launch** is today.\n\n- one\n- two',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    return copied;
  }

  Future<void> copy(WidgetTester tester, String words) async {
    await tester.longPress(find.textContaining(words, findRichText: true));
    await pumpFrames(tester);
    await tester.tap(find.text('Copy'));
    await pumpFrames(tester);
  }

  testWidgets("an agent's reply copies as it reads, without Markdown", (
    tester,
  ) async {
    final copied = await show(tester);
    await copy(tester, 'is today');
    expect(copied.last, 'The launch is today.\n\n• one\n• two');
  });

  testWidgets('your own message copies as you typed it', (tester) async {
    final copied = await show(tester);
    await copy(tester, 'stars, please');
    expect(copied.last, 'Keep **these** stars, please');
  });
}
