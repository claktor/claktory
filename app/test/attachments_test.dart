import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show Image;
import 'package:flutter_test/flutter_test.dart';

import 'drafts_test.dart' show two;
import 'harness.dart';

/// The Mac's window sends [method] to the app, as when files are dropped.
Future<void> fromWindow(
  WidgetTester tester,
  String method,
  Object? args,
) async {
  await TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage(
        'factory/window',
        const StandardMethodCodec().encodeMethodCall(MethodCall(method, args)),
        (_) {},
      );
  await files(tester);
}

/// Lets the files' sizes be read from disk.
Future<void> files(WidgetTester tester) async {
  for (var i = 0; i < 3; i++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
  }
  await tester.pump();
}

void main() {
  late Directory folder;
  late String notes;
  late String photo;
  final calls = <MethodCall>[];

  setUp(() {
    folder = Directory.systemTemp.createTempSync('attachments');
    notes = '${folder.path}/notes.txt';
    photo = '${folder.path}/screen.png';
    File(notes).writeAsStringSync('x' * 2048);
    File(photo).writeAsBytesSync(List.filled(10, 1));
    calls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/window'), (
          call,
        ) async {
          calls.add(call);
          if (call.method == 'pickFiles') return [photo];
          return null;
        });
  });

  tearDown(() {
    folder.deleteSync(recursive: true);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/window'), null);
  });

  test('which files show as pictures', () {
    expect(isImageName('screen.PNG'), isTrue);
    expect(isImageName('photo.heic'), isTrue);
    expect(isImageName('notes.txt'), isFalse);
    expect(isImageName('Makefile'), isFalse);
  });

  testWidgets('files dropped on the window go in the message box', (
    tester,
  ) async {
    final harness = Harness(two());
    await harness.show(tester);
    // The window takes files while a message box shows.
    expect(
      calls.where((call) => call.method == 'takeFiles').last.arguments,
      isTrue,
    );

    await fromWindow(tester, 'filesOver', true);
    expect(FileDrop.over.value, isTrue);
    await fromWindow(tester, 'filesDropped', [notes, folder.path]);
    await tester.pump();
    expect(FileDrop.over.value, isFalse);
    expect(find.text('notes.txt'), findsOneWidget);
    expect(find.text('2.0 KB'), findsOneWidget);
    // A folder isn't attached, and the box says so.
    expect(harness.backend.attachments.of('a1'), hasLength(1));
    expect(find.textContaining('attached'), findsOneWidget);

    // The same file twice is there once.
    await fromWindow(tester, 'filesDropped', [notes]);
    await tester.pump();
    expect(find.text('notes.txt'), findsOneWidget);

    // × takes it out.
    await tester.tap(find.bySemanticsLabel('Remove notes.txt'));
    await tester.pump();
    expect(find.text('notes.txt'), findsNothing);
    expect(harness.backend.attachments.of('a1'), isEmpty);
  });

  testWidgets('the paperclip opens the picker; files stay with their agent', (
    tester,
  ) async {
    final harness = Harness(two());
    await harness.show(tester);
    await tester.tap(
      find.byWidgetPredicate(
        (widget) => widget is GlyphButton && widget.label == 'Attach files',
      ),
    );
    await files(tester);
    expect(calls.map((call) => call.method), contains('pickFiles'));
    expect(find.text('screen.png'), findsOneWidget);

    // Another agent's box has its own.
    await harness.apply(tester, two(selected: 'a2'));
    expect(find.text('screen.png'), findsNothing);
    await harness.apply(tester, two(selected: 'a1'));
    expect(find.text('screen.png'), findsOneWidget);
  });

  testWidgets("a message's files: pictures as thumbnails, others as chips", (
    tester,
  ) async {
    // A 1×1 PNG.
    const png = [
      137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, //
      1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, //
      65, 84, 120, 156, 99, 248, 15, 4, 0, 9, 251, 3, 253, 227, 85, 242, //
      156, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    final harness = Harness(two());
    harness.backend.files['/scratch/shot.png'] = FileContent(
      path: '/scratch/shot.png',
      data: png,
      size: Int64(png.length),
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
                  size: Int64(2048),
                ),
                Attachment(
                  path: '/scratch/shot.png',
                  name: 'shot.png',
                  size: Int64(png.length),
                ),
                Attachment(
                  path: '/scratch/gone.png',
                  name: 'gone.png',
                  size: Int64(10),
                ),
              ],
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await tester.pump();
    expect(find.text('Have a look'), findsOneWidget);
    expect(find.text('notes.txt'), findsOneWidget);
    expect(find.text('2.0 KB'), findsOneWidget);
    // The picture fetched shows itself; one that can't be read is a chip.
    expect(find.byType(Image), findsOneWidget);
    expect(find.text('shot.png'), findsNothing);
    expect(find.text('gone.png'), findsOneWidget);

    // A click opens it beside the conversation.
    await tester.tap(find.text('notes.txt'));
    await tester.pump();
    expect(
      harness.last.setLayout.root.toString(),
      contains('/scratch/notes.txt'),
    );
  });
}
