// Photos on a phone (Matt, via Bruno 46443 and 46560; Polpo's
// photo-attach.md): what's chosen in the picker shows at once as "Adding
// photo…" while it's copied for the app, in the message box and under a
// card's field, and Send waits for it; a photo sent shows from this
// device while it uploads and once it's sent, never read back over the
// link; any other picture comes as a preview made for the screen, never
// the whole original.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/ask_card.dart' show AskPersonCard;
import 'package:factory_app/attachments.dart';
import 'package:factory_app/card_files.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
  ],
  models: [Model(id: 'anthropic/claude-opus-5-5', name: 'Opus 5.5')],
  selectedAgentId: 'june',
);

/// Lets real files be read: their sizes as they're added.
Future<void> _io(WidgetTester tester) async {
  for (var i = 0; i < 12; i++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 10)),
    );
  }
  await tester.pump();
}

/// An iPhone, for this test.
void _iPhone() {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
}

void main() {
  late Directory folder;
  late String shot;

  setUp(() {
    folder = Directory.systemTemp.createTempSync('photo-attach');
    shot = '${folder.path}/IMG_8227.jpeg';
    File(shot).writeAsBytesSync(List.filled(3000, 7));
  });

  tearDown(() {
    folder.deleteSync(recursive: true);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/device'), null);
  });

  /// The iPhone's picker, staged: [kinds] on their way as soon as it
  /// closes, their paths once [copied] completes. The pick's arguments.
  List<Object?> picker(List<String> kinds, Completer<List<String>> copied) {
    final calls = <Object?>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/device'), (
          call,
        ) async {
          switch (call.method) {
            case 'pickAttachments':
              calls.add(call.arguments);
              return {'pending': kinds, 'ticket': 7};
            case 'pickedAttachments':
              expect((call.arguments as Map)['ticket'], 7);
              return copied.future;
          }
          return null;
        });
    return calls;
  }

  Future<Harness> conversation(WidgetTester tester, {Transcript? t}) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      t ??
          Transcript(
            agentId: 'june',
            entries: [
              Entry(
                seq: Int64(1),
                user: UserText(text: 'Hello'),
              ),
            ],
          ),
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    if (find.byType(AgentView).evaluate().isEmpty) {
      await tester.tap(find.text('June').first);
      await pumpFrames(tester);
    }
    expect(find.byType(AgentView), findsOneWidget);
    return harness;
  }

  Finder sendButton() => find.bySemanticsLabel('Send');

  testWidgets('the message box: photos chosen show at once as "Adding '
      'photo…", Send waits for them, then each is a chip', (tester) async {
    _iPhone();
    final copied = Completer<List<String>>();
    final calls = picker(['photo', 'photo'], copied);
    final harness = await conversation(tester);
    final field = find.byType(EditableText).last;
    await tester.showKeyboard(field);
    await tester.enterText(field, 'Look at this');
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Attach files'));
    await pumpFrames(tester);
    expect((calls.single! as Map)['staged'], isTrue);
    expect(find.text('Adding photo…'), findsNWidgets(2));
    // Send waits: nothing goes without the photos.
    await tester.tap(sendButton().last, warnIfMissed: false);
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
    // One of the two couldn't be copied.
    copied.complete([shot]);
    await _io(tester);
    expect(find.text('Adding photo…'), findsNothing);
    expect(find.text('IMG_8227.jpeg'), findsOneWidget);
    expect(find.text("One photo wasn't added. Try again."), findsOneWidget);
    expect(harness.backend.attachments.of('june').single.path, shot);
    expect(harness.backend.attachments.adding('june'), isEmpty);
  });

  testWidgets('a photo that could not be copied says so, and nothing '
      'waits', (tester) async {
    _iPhone();
    final copied = Completer<List<String>>();
    picker(['photo'], copied);
    final harness = await conversation(tester);
    await tester.tap(find.bySemanticsLabel('Attach files'));
    await pumpFrames(tester);
    expect(find.text('Adding photo…'), findsOneWidget);
    copied.complete(const []);
    await _io(tester);
    expect(find.text('Adding photo…'), findsNothing);
    expect(find.text("Couldn't add this photo. Try again."), findsOneWidget);
    expect(harness.backend.attachments.of('june'), isEmpty);
  });

  testWidgets("a card's paperclip: \"Adding video…\" under its field, then "
      "the file's chip", (tester) async {
    _iPhone();
    final copied = Completer<List<String>>();
    picker(['video'], copied);
    final harness = Harness(_workspace());
    await harness.show(
      tester,
      size: const Size(393, 852),
      home: SingleChildScrollView(
        child: AskPersonCard(
          AskPerson(
            id: 'ask4',
            title: 'Is the new logo right?',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
            revision: 1,
          ),
          agentId: 'june',
          seq: 4,
        ),
      ),
    );
    await settle(tester);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Attach files').first);
    await pumpFrames(tester);
    final card = cardFilesKey('june', 'ask4');
    expect(find.text('Adding video…'), findsOneWidget);
    expect(harness.backend.attachments.adding(card), ['video']);
    copied.complete([shot]);
    await _io(tester);
    expect(find.text('Adding video…'), findsNothing);
    expect(harness.backend.attachments.of(card).single.name, 'IMG_8227.jpeg');
  });

  testWidgets("an answer sent while a photo is still being added waits "
      'for it, and takes it', (tester) async {
    _iPhone();
    final harness = Harness(_workspace());
    final backend = harness.backend;
    final card = cardFilesKey('june', 'ask4');
    backend.attachments.startAdding(card, ['photo']);
    List<Attachment>? sent;
    await tester.runAsync(() async {
      unawaited(
        CardUploads.of(backend)
            .send(backend, 'june', card, backend.attachments.of(card))
            .then((files) => sent = files),
      );
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(sent, isNull);
      await backend.attachments.add(card, [shot]);
      backend.attachments.doneAdding(card, ['photo']);
      for (var i = 0; i < 50 && sent == null; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
    });
    expect(sent?.single.name, 'IMG_8227.jpeg');
    // Sent from here: where it went is this file on this device.
    expect(
      CardUploads.of(backend).localCopyOf('/uploads/june/IMG_8227.jpeg'),
      shot,
    );
  });

  testWidgets('a photo sent shows from this device as it uploads, then '
      'once sent, never read back over the link', (tester) async {
    _iPhone();
    final harness = await conversation(tester);
    final backend = harness.backend;
    final local = LocalFile(path: shot, name: 'IMG_8227.jpeg', size: 3000);
    backend.outbox.send('june', '', files: [local]);
    await pumpFrames(tester);
    // On its way: the picture itself, from its file here.
    bool fromHere(Widget widget) =>
        widget is Image &&
        widget.image is ResizeImage &&
        (widget.image as ResizeImage).imageProvider is FileImage &&
        ((widget.image as ResizeImage).imageProvider as FileImage).file.path ==
            shot;
    expect(find.byWidgetPredicate(fromHere), findsOneWidget);
    // Uploaded and sent.
    await _io(tester);
    expect(backend.sent.where((m) => m.hasSendMessage()), hasLength(1));
    const path = '/uploads/june/IMG_8227.jpeg';
    expect(backend.outbox.localCopyOf(path), shot);
    // What the backend made of it takes its place: still this file.
    backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(
              attachments: [
                Attachment(
                  path: path,
                  name: 'IMG_8227.jpeg',
                  size: Int64(3000),
                ),
              ],
            ),
          ),
        ],
      ),
    );
    await pumpFrames(tester);
    expect(find.byWidgetPredicate(fromHere), findsOneWidget);
    expect(
      backend.sent.where((m) => m.hasReadFile() && m.readFile.path == path),
      isEmpty,
    );
  });

  testWidgets("a picture sent from elsewhere comes as a preview made for "
      'the screen, never the whole original', (tester) async {
    _iPhone();
    const path = '/uploads/june/IMG_9000.jpeg';
    final harness = Harness(_workspace());
    harness.backend.previews[path] = FileContent(
      path: path,
      data: List.filled(500, 1),
      size: Int64(500),
      sha256: 'abc',
      preview: FilePreview.FILE_PREVIEW_MADE,
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(
              attachments: [
                Attachment(
                  path: path,
                  name: 'IMG_9000.jpeg',
                  size: Int64(2900000),
                ),
              ],
            ),
          ),
        ],
      ),
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    if (find.byType(TranscriptView).evaluate().isEmpty) {
      await tester.tap(find.text('June').first);
      await pumpFrames(tester);
    }
    final reads = [
      for (final m in harness.backend.sent)
        if (m.hasReadFile() && m.readFile.path == path) m.readFile,
    ];
    expect(reads, isNotEmpty);
    for (final read in reads) {
      expect(read.previewWidth, greaterThan(0));
      expect(read.previewHeight, greaterThan(0));
    }
    expect(find.byType(Image), findsWidgets);
  });
}
