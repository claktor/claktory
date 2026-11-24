// Recordings kept on the device until their Claktory has them (#401): what
// each is for, kept beside it; read back after a relaunch, voice messages
// to their outboxes and dictations for their fields; what's left of one
// never kept swept; and the ones no field has, listed in Settings → Voice
// to play or delete.
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart' show RemoteFactory;
import 'package:factory_app/scope.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show io;

void main() {
  late Directory kept;

  setUp(() {
    kept = Directory.systemTemp.createTempSync('kept');
    final was = keptVoiceFolder;
    keptVoiceFolder = () async => kept.path;
    addTearDown(() {
      keptVoiceFolder = was;
      if (kept.existsSync()) kept.deleteSync(recursive: true);
    });
  });

  /// A recording in its own folder, with what it's for beside it.
  KeptVoice keep(
    String id, {
    KeptKind kind = KeptKind.dictation,
    String target = 'ask/june/ask7',
    String? claktory,
    String label = '',
    bool save = true,
  }) {
    final folder = Directory('${kept.path}/$id')..createSync();
    final path = '${folder.path}/Voice.m4a';
    File(path).writeAsBytesSync([1, 2, 3, 4]);
    final voice = KeptVoice(
      path: path,
      duration: const Duration(seconds: 65),
      claktory: claktory ?? keptClaktoryKey(null),
      kind: kind,
      target: target,
      label: label,
      made: DateTime(2026, 11, 24, 9),
      clientId: 'client-$id',
      uploadId: 'upload-$id',
      requestId: 'request-$id',
    );
    if (save) {
      File('${folder.path}/${KeptVoice.recordName}')
          .writeAsStringSync(jsonEncode(voice.toJson()));
    }
    return voice;
  }

  test("what a recording is for is kept beside it, and read back", () async {
    final voice = keep('1', save: false)..uploaded = true;
    await voice.save();
    final read = KeptVoice.read(voice.folder)!;
    expect(read.path, voice.path);
    expect(read.duration, const Duration(seconds: 65));
    expect(read.kind, KeptKind.dictation);
    expect(read.target, 'ask/june/ask7');
    expect(read.clientId, 'client-1');
    expect(read.uploadId, 'upload-1');
    expect(read.requestId, 'request-1');
    expect(read.uploaded, isTrue);
    expect(read.made, DateTime(2026, 11, 24, 9));

    await voice.delete();
    expect(voice.folder.existsSync(), isFalse);
  });

  test("a recording's folder left empty (the app closed while recording, "
      "Sale's row 86) is swept after a minute, at a launch or when a "
      'recording starts; one just made stays', () async {
    Directory empty(String id, {bool old = true}) {
      final folder = Directory('${kept.path}/$id')..createSync();
      if (old) {
        // Two minutes old.
        final at = DateTime.now().subtract(const Duration(minutes: 2));
        String two(int n) => n.toString().padLeft(2, '0');
        Process.runSync('touch', [
          '-t',
          '${at.year}${two(at.month)}${two(at.day)}${two(at.hour)}'
              '${two(at.minute)}.${two(at.second)}',
          folder.path,
        ]);
      }
      return folder;
    }

    final left = empty('1');
    final fresh = empty('2', old: false);
    final recording = empty('3');
    File('${recording.path}/Voice.m4a').writeAsBytesSync([1, 2, 3]);
    await sweepEmptyRecordings();
    expect(left.existsSync(), isFalse);
    expect(fresh.existsSync(), isTrue, reason: 'maybe starting now');
    expect(recording.existsSync(), isTrue, reason: 'maybe recording now');

    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
    );
    addTearDown(backend.close);
    final atLaunch = empty('4');
    await KeptVoices.of(backend).load(keptClaktoryKey(null));
    expect(atLaunch.existsSync(), isFalse);
    expect(fresh.existsSync(), isTrue);
    expect(recording.existsSync(), isTrue);
  });

  test('a voice message keeps its typed text and files, and a voice note '
      'left in its box goes back to the box after a relaunch (#425)', () async {
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
    );
    addTearDown(backend.close);
    final sent = keep('1', kind: KeptKind.agent, target: 'june', save: false)
      ..text = 'Look at the login page'
      ..files = [
        const KeptFile(
          path: '/tmp/shot.png',
          name: 'shot.png',
          size: 9,
          uploadId: 'upload-shot',
        ),
      ];
    await sent.save();
    final read = KeptVoice.read(sent.folder)!;
    expect(read.text, 'Look at the login page');
    expect(read.files.single.uploadId, 'upload-shot');
    expect(read.draft, isFalse);
    final note = keep('2', kind: KeptKind.agent, target: 'mike', save: false)
      ..draft = true;
    await note.save();
    final chatNote = keep(
      '3',
      kind: KeptKind.chat,
      target: 'chat:c1',
      save: false,
    )..draft = true;
    await chatNote.save();

    await KeptVoices.of(backend).load(keptClaktoryKey(null));
    final message = backend.outbox.of('june').single;
    expect(message.text, 'Look at the login page');
    expect(message.files.map((file) => file.uploadId), [
      'upload-1',
      'upload-shot',
    ]);
    final notes = VoiceNotes.of(backend);
    expect(notes.note('mike')?.clientId, 'client-2');
    expect(notes.note('chat:c1')?.clientId, 'client-3');
    expect(backend.outbox.of('mike'), isEmpty, reason: 'not sent');
    expect(backend.chatOutbox.of('chat:c1'), isEmpty);
    expect(await keptVoiceCount(keptClaktoryKey(null)), 3);

    // Taken out of its box, gone from the device.
    await notes.discard('mike');
    expect(note.folder.existsSync(), isFalse);
  });

  test('a record whose recording is gone, or that names a file elsewhere, '
      'is no recording', () {
    final gone = keep('1');
    File(gone.path).deleteSync();
    expect(KeptVoice.read(gone.folder), isNull);
    final elsewhere = keep('2', save: false);
    File('${elsewhere.folder.path}/${KeptVoice.recordName}')
        .writeAsStringSync('{"kind": "agent", "file": "../1/Voice.m4a"}');
    expect(KeptVoice.read(elsewhere.folder), isNull);
  });

  test("after a relaunch: voice messages back to their outboxes, dictations "
      "kept for their fields, another Claktory's left alone, and what's "
      'left of a recording never kept swept', () async {
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
    );
    addTearDown(backend.close);
    keep('1', kind: KeptKind.agent, target: 'june');
    keep('2', kind: KeptKind.chat, target: 'chat-1');
    keep('3', label: 'Your answer to June');
    keep('4', claktory: 'another');
    final stray = Directory('${kept.path}/5')..createSync();
    // Two hours old.
    Process.runSync('touch', ['-t', '202601010000', stray.path]);
    final recent = Directory('${kept.path}/6')..createSync();

    final voices = KeptVoices.of(backend);
    await voices.load(keptClaktoryKey(null));

    final message = backend.outbox.of('june').single;
    expect(message.clientId, 'client-1');
    expect(message.files.single.uploadId, 'upload-1');
    expect(message.voice, const Duration(seconds: 65));
    expect(message.waiting, isTrue, reason: 'not connected yet');
    expect(backend.chatOutbox.of('chat-1').single.clientId, 'client-2');
    expect(voices.loose.single.label, 'Your answer to June');
    expect(voices.holds('ask/june/ask7'), isTrue);
    expect(stray.existsSync(), isFalse);
    expect(recent.existsSync(), isTrue, reason: 'maybe recording now');
    expect(Directory('${kept.path}/4').existsSync(), isTrue);

    // A field takes it; let go, it's listed again.
    final taken = voices.take('ask/june/ask7')!;
    expect(voices.loose, isEmpty);
    voices.release(taken);
    expect(voices.loose, [taken]);
  });

  test("a Claktory's key names it, not its address: another at the same "
      "address never gets its recordings", () {
    RemoteFactory at(String fingerprint, {String endpointId = ''}) =>
        RemoteFactory(
          code: 'factory://code-$fingerprint',
          host: '192.168.1.20',
          port: 7420,
          token: 't',
          fingerprint: fingerprint,
          name: 'mac-mini',
          endpointId: endpointId,
        );
    expect(keptClaktoryKey(at('aa')), isNot(keptClaktoryKey(at('bb'))));
    expect(keptClaktoryKey(at('aa')), keptClaktoryKey(at('aa')));
    expect(
      keptClaktoryKey(at('aa', endpointId: 'e1')),
      keptClaktoryKey(at('bb', endpointId: 'e1')),
      reason: 'through the tunnel, its endpoint id names it',
    );
    expect(keptClaktoryKey(null), isNot(keptClaktoryKey(at('aa'))));
  });

  test("forgetting a Claktory deletes its recordings, and only its", () async {
    final mine = keep('1', claktory: 'mine');
    final other = keep('2', claktory: 'other');
    await forgetKeptVoices('mine');
    expect(mine.folder.existsSync(), isFalse);
    expect(other.folder.existsSync(), isTrue);
  });

  testWidgets('Settings → Voice lists an answer not sent whose card has '
      'gone: to play, or delete', (tester) async {
    keep('1', target: 'ask/june/gone', label: 'Your answer to June: Ship it?');
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
        speechModel: SpeechModel(
          state: SpeechModelState.SPEECH_MODEL_STATE_READY,
        ),
      ),
    );
    harness.page.value = AppPage.settings;
    await harness.show(tester, size: const Size(1200, 1600));
    await io(tester);
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Not sent'));
    await pumpFrames(tester);
    expect(find.text('Your answer to June: Ship it?'), findsOneWidget);
    expect(find.textContaining('1:05 · '), findsOneWidget);
    expect(find.bySemanticsLabel('Play'), findsOneWidget);

    await tester.tap(find.text('Delete').last);
    await pumpFrames(tester);
    expect(find.text('Delete this recording?'), findsOneWidget);
    await tester.tap(find.text('Delete').last);
    await io(tester);
    await pumpFrames(tester);
    expect(find.text('Not sent'), findsNothing);
    expect(Directory('${kept.path}/1').existsSync(), isFalse);
    expect(
      harness.backend.sent
          .where((m) => m.hasCancelUpload())
          .single
          .cancelUpload
          .uploadId,
      'upload-1',
    );
  });
}
