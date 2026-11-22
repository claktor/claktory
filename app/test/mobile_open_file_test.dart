// A file sent in a conversation, opened on an iPhone: a tap on its chip,
// or on a picture's thumbnail, pushes its viewer over the conversation (as
// an agent's `show` does there), and Back comes back to the conversation.
// On a Mac the same tap puts it beside the conversation (attachments_test).
import 'package:factory_app/pane_chrome.dart' show PaneChrome;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show snapshot, text;
import 'viewers_test.dart' show png;

/// An iPhone 16 on Builder's conversation, with a message that sent
/// notes.txt and shot.png.
Future<Harness> _phone(WidgetTester tester) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final shot = await png(tester, 3, 2);
  final harness = Harness(snapshot())..backend.appliesLayouts = true;
  harness.backend.files['/scratch/notes.txt'] = text(
    '/scratch/notes.txt',
    'The notes.',
  );
  harness.backend.files['/scratch/shot.png'] = FileContent(
    path: '/scratch/shot.png',
    mime: 'image/png',
    data: shot,
    size: Int64(shot.length),
  );
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(
            text: 'Have a look',
            attachments: [
              Attachment(
                path: '/scratch/notes.txt',
                name: 'notes.txt',
                size: Int64(10),
              ),
              Attachment(
                path: '/scratch/shot.png',
                name: 'shot.png',
                size: Int64(shot.length),
              ),
            ],
          ),
        ),
      ],
    ),
  );
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  await harness.show(tester, size: const Size(393, 852));
  await _settle(tester);
  expect(find.text('Have a look').hitTestable(), findsOneWidget);
  expect(find.byType(PaneChrome), findsNothing);
  return harness;
}

/// Frames, and the picture's fetch and decoding, which are real work.
Future<void> _settle(WidgetTester tester) async {
  for (var i = 0; i < 4; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 50)),
    );
    await pumpFrames(tester);
  }
}

/// What the app last asked the agent's layout to be.
String _layout(Harness harness) => harness.backend.sent
    .lastWhere((message) => message.hasSetLayout())
    .setLayout
    .root
    .toString();

Future<void> _back(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('Back').last);
  await _settle(tester);
  expect(find.byType(PaneChrome), findsNothing);
  expect(find.text('Have a look').hitTestable(), findsOneWidget);
}

void main() {
  testWidgets("a sent file's chip pushes its viewer; Back returns", (
    tester,
  ) async {
    final harness = await _phone(tester);
    await tester.tap(find.text('notes.txt'));
    await _settle(tester);
    // In the agent's layout, as its `show` puts it, and in front here.
    expect(_layout(harness), contains('/scratch/notes.txt'));
    expect(find.byType(PaneChrome), findsOneWidget);
    expect(find.text('The notes.'), findsOneWidget);
    expect(find.text('Have a look').hitTestable(), findsNothing);
    await _back(tester);
  });

  testWidgets("a sent picture's thumbnail pushes its viewer; Back returns", (
    tester,
  ) async {
    final harness = await _phone(tester);
    expect(find.byType(Image), findsOneWidget);
    await tester.tap(find.byType(Image));
    await _settle(tester);
    expect(_layout(harness), contains('/scratch/shot.png'));
    expect(find.byType(PaneChrome), findsOneWidget);
    expect(find.text('Have a look').hitTestable(), findsNothing);
    await _back(tester);
  });
}
