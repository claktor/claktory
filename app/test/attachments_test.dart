import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/drop.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
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

/// A test in the Mac's window: the only one that takes drops.
void macWidgets(String description, WidgetTesterCallback callback) =>
    testWidgets(
      description,
      callback,
      variant: TargetPlatformVariant.only(TargetPlatform.macOS),
    );

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

  macWidgets('files dropped on the window go in the message box', (
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

  /// The app's window: the shell, with the drop overlay over it all.
  Future<Harness> showWindow(WidgetTester tester, Workspace workspace) async {
    final harness = Harness(workspace);
    await harness.show(
      tester,
      home: const Stack(
        children: [
          Positioned.fill(child: Shell()),
          Positioned.fill(child: WindowDropOverlay()),
        ],
      ),
    );
    return harness;
  }

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    testWidgets("${platform.name}'s window takes no drops: the Mac's channel "
        "is never used, and nothing dims", (tester) async {
      final harness = await showWindow(tester, two());
      expect(harness.backend.workspace.selectedAgent, isNotNull);
      expect(calls, isEmpty);
      // Even if something said files were over it.
      FileDrop.dragged(2);
      await tester.pump();
      expect(find.text('2 files'), findsNothing);
      expect(
        find.descendant(
          of: find.byType(WindowDropOverlay),
          matching: find.byType(IgnorePointer),
        ),
        findsNothing,
      );
      FileDrop.dragged(0);
      await tester.pump();
      expect(calls, isEmpty);
    }, variant: TargetPlatformVariant.only(platform));
  }

  macWidgets("while a card's field has the keys, the overlay says they go "
      'to it (#511)', (tester) async {
    await showWindow(tester, two());
    final owner = Object();
    FileDrop.divert(owner, (_) {}, label: 'your answer to Builder');
    addTearDown(() => FileDrop.undivert(owner));
    await fromWindow(tester, 'filesOver', 2);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your answer to Builder'), findsOneWidget);
    expect(find.text('2 files'), findsOneWidget);
    await fromWindow(tester, 'filesOver', 0);
    await tester.pump(const Duration(milliseconds: 200));
  });

  macWidgets('files dragged over the window dim it all and say where they '
      'go', (tester) async {
    await showWindow(tester, two());
    expect(find.text('Drop to add to your message to Builder'), findsNothing);

    // An older window says only that some are.
    await fromWindow(tester, 'filesOver', true);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your message to Builder'), findsOneWidget);
    expect(find.textContaining('files'), findsNothing);

    await fromWindow(tester, 'filesOver', 3);
    expect(FileDrop.over.value, isTrue);
    expect(find.text('3 files'), findsOneWidget);
    final overlay = find.byType(WindowDropOverlay);
    // Over the whole window, and it never takes the pointer.
    expect(tester.getSize(overlay), const Size(1200, 900));
    expect(
      find.descendant(of: overlay, matching: find.byType(IgnorePointer)),
      findsWidgets,
    );

    // Gone once they leave, after it fades.
    await fromWindow(tester, 'filesOver', 0);
    expect(find.text('Drop to add to your message to Builder'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your message to Builder'), findsNothing);

    // Dropped: gone too, and the files are in the box.
    await fromWindow(tester, 'filesOver', 1);
    await fromWindow(tester, 'filesDropped', [notes]);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your message to Builder'), findsNothing);
    expect(find.text('notes.txt'), findsOneWidget);
  });

  macWidgets('after switching agents, drops still go to the one showing', (
    tester,
  ) async {
    final harness = await showWindow(tester, two());
    // The new box claims the window before the old one lets go of it.
    await harness.apply(tester, two(selected: 'a2'));
    await tester.pump();
    expect(
      calls.where((call) => call.method == 'takeFiles').last.arguments,
      isTrue,
    );
    expect(FileDrop.takesFiles.value, isTrue);
    await fromWindow(tester, 'filesOver', 1);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your message to Writer'), findsOneWidget);
    await fromWindow(tester, 'filesDropped', [notes]);
    expect(harness.backend.attachments.of('a2'), hasLength(1));
    expect(harness.backend.attachments.of('a1'), isEmpty);
  });

  macWidgets("a chat takes no files, and the window says so", (tester) async {
    final harness = await showWindow(
      tester,
      Workspace(
        agents: [Agent(id: 'a1', name: 'Builder', modelAvailable: true)],
        chats: [Chat(id: 'c1', title: 'Trains', modelAvailable: true)],
        selectedAgentId: 'a1',
        selectedChatId: 'c1',
      ),
    );
    harness.backend.chatTranscript.load(Transcript(agentId: 'c1'));
    await tester.pump();
    expect(FileDrop.takesFiles.value, isFalse);
    expect(
      calls.where((call) => call.method == 'takeFiles').last.arguments,
      isFalse,
      reason: 'the window refuses the drop',
    );
    await fromWindow(tester, 'filesOver', 2);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text("Files can't be added to a chat"), findsOneWidget);
    expect(find.textContaining('Drop to add'), findsNothing);
  });

  macWidgets('with no conversation open, the window says to open one', (
    tester,
  ) async {
    final harness = await showWindow(tester, two());
    harness.page.value = AppPage.settings;
    await tester.pump();
    await tester.pump();
    expect(FileDrop.takesFiles.value, isNull);
    await fromWindow(tester, 'filesOver', 1);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Open a conversation to add files'), findsOneWidget);
  });

  macWidgets('the paperclip opens the picker; files stay with their agent', (
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

  macWidgets("a message's files: pictures as thumbnails, others as chips", (
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
