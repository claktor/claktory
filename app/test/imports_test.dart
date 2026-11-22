import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:factory_app/imports/claude_export.dart';
import 'package:factory_app/imports/import_view.dart';
import 'package:factory_app/imports/import_worker.dart';
import 'package:factory_app/imports/zip.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

// B0a: reading a Claude export on this device. The export's layout is
// provisional (no real export checked yet), so these test the reader's
// limits and the preview against made-up exports.

/// One file in a test ZIP, with what its headers say about it.
class _Entry {
  _Entry(
    this.name,
    List<int> data, {
    this.deflate = false,
    this.flags = 0,
    this.mode,
    this.size,
  }) : data = Uint8List.fromList(data);

  final String name;
  final Uint8List data;
  final bool deflate;
  final int flags;

  /// A Unix mode, made on Unix.
  final int? mode;

  /// The size its headers claim, when it's a lie.
  final int? size;
}

/// A ZIP of [entries]; [count] overrides how many its end record says.
Uint8List _zip(List<_Entry> entries, {int? count}) {
  final out = BytesBuilder();
  final directory = BytesBuilder();
  void u16(BytesBuilder b, int v) =>
      b.add((ByteData(2)..setUint16(0, v, Endian.little)).buffer.asUint8List());
  void u32(BytesBuilder b, int v) =>
      b.add((ByteData(4)..setUint32(0, v, Endian.little)).buffer.asUint8List());
  for (final entry in entries) {
    final name = utf8.encode(entry.name);
    final packed = entry.deflate
        ? ZLibEncoder(raw: true).convert(entry.data)
        : entry.data;
    final size = entry.size ?? entry.data.length;
    final offset = out.length;
    u32(out, 0x04034b50);
    u16(out, 20);
    u16(out, entry.flags);
    u16(out, entry.deflate ? 8 : 0);
    u32(out, 0); // time and date
    u32(out, 0); // CRC: not checked
    u32(out, packed.length);
    u32(out, size);
    u16(out, name.length);
    u16(out, 0);
    out.add(name);
    out.add(packed);

    u32(directory, 0x02014b50);
    u16(directory, entry.mode == null ? 20 : (3 << 8) | 20);
    u16(directory, 20);
    u16(directory, entry.flags);
    u16(directory, entry.deflate ? 8 : 0);
    u32(directory, 0);
    u32(directory, 0);
    u32(directory, packed.length);
    u32(directory, size);
    u16(directory, name.length);
    u16(directory, 0); // extra
    u16(directory, 0); // comment
    u16(directory, 0); // disk
    u16(directory, 0); // internal attributes
    u32(directory, (entry.mode ?? 0) << 16);
    u32(directory, offset);
    directory.add(name);
  }
  final at = out.length;
  final dir = directory.takeBytes();
  out.add(dir);
  u32(out, 0x06054b50);
  u16(out, 0);
  u16(out, 0);
  u16(out, count ?? entries.length);
  u16(out, count ?? entries.length);
  u32(out, dir.length);
  u32(out, at);
  u16(out, 0);
  return out.takeBytes();
}

Map<String, Object?> _conversation(
  String id,
  String name,
  String created, {
  String account = 'me',
  List<(String, String)> messages = const [
    ('human', 'Plan the trip to Lisbon'),
    ('assistant', 'Here is a plan.'),
  ],
}) => {
  'uuid': id,
  'name': name,
  'created_at': created,
  'account': {'uuid': account},
  'chat_messages': [
    for (final (sender, text) in messages) {'sender': sender, 'text': text},
  ],
};

List<int> _json(Object value) => utf8.encode(jsonEncode(value));

final _export = [
  _conversation('a', 'Lisbon trip', '2026-01-02T10:00:00Z'),
  _conversation(
    'b',
    '',
    '2026-03-04T10:00:00Z',
    messages: [
      ('human', '  Why is\nthe sky blue?  '),
      ('assistant', 'Rayleigh scattering.'),
      ('human', 'Thanks'),
    ],
  ),
];

Matcher _fails(ExportProblem problem) =>
    throwsA(isA<ExportFailure>().having((f) => f.problem, 'problem', problem));

ExportPreview _read(Uint8List bytes, [ExportLimits? limits]) =>
    readClaudeExport(bytes, limits: limits ?? const ExportLimits());

Uint8List _with(List<_Entry> more) => _zip([
  _Entry('conversations.json', _json(_export), deflate: true),
  ...more,
]);

void main() {
  group('reading an export', () {
    test('its conversations, newest first, and what it left out', () {
      final bytes = _zip([
        _Entry('conversations.json', _json(_export), deflate: true),
        _Entry('users.json', _json([{}])),
        _Entry('projects.json', _json([])),
        _Entry('audio/1.m4a', [1, 2, 3]),
        _Entry('audio/2.webm', [1, 2, 3]),
        _Entry('audio/', []),
      ]);
      final preview = _read(bytes);
      expect([for (final c in preview.conversations) c.id], ['b', 'a']);
      final sky = preview.conversations.first;
      expect(sky.title, '');
      expect(sky.snippet, 'Why is the sky blue?');
      expect((sky.fromYou, sky.fromClaude, sky.messages), (2, 1, 3));
      expect(
        sky.createdMs,
        DateTime.utc(2026, 3, 4, 10).millisecondsSinceEpoch,
      );
      expect(preview.conversations.last.title, 'Lisbon trip');
      expect(preview.account, isTrue);
      expect((preview.voice, preview.other), (2, 1));
      expect(preview.sha256, hasLength(64));
      expect(
        leftOutLine(preview),
        'Left out: your account details, 2 voice recordings, 1 other file.',
      );
    });

    test('a stored archive, and the conversations JSON on its own', () {
      final stored = _read(
        _zip([_Entry('conversations.json', _json(_export))]),
      );
      expect(stored.conversations, hasLength(2));
      expect(leftOutLine(stored), isNull);
      final bare = _read(Uint8List.fromList(_json(_export)));
      expect(bare.conversations, hasLength(2));
    });

    test('anything else is not a Claude export', () {
      expect(
        () => _read(_zip([_Entry('notes.txt', utf8.encode('hello'))])),
        _fails(ExportProblem.unsupported),
      );
      expect(
        () => _read(Uint8List.fromList(utf8.encode('hello'))),
        _fails(ExportProblem.unsupported),
      );
      expect(
        () => _read(Uint8List.fromList(_json({'conversations': []}))),
        _fails(ExportProblem.unsupported),
      );
      expect(
        () => _read(
          Uint8List.fromList(
            _json([
              {'uuid': 'a'},
            ]),
          ),
        ),
        _fails(ExportProblem.unsupported),
      );
    });

    test('several people’s conversations are refused', () {
      final bytes = _json([
        _conversation('a', 'Mine', '2026-01-01T00:00:00Z'),
        _conversation('b', 'Theirs', '2026-01-01T00:00:00Z', account: 'them'),
      ]);
      expect(
        () => _read(Uint8List.fromList(bytes)),
        _fails(ExportProblem.severalPeople),
      );
    });

    test('a file over 100 MB is refused before it is read', () {
      final bytes = Uint8List.fromList(_json(_export));
      expect(
        () => _read(bytes, ExportLimits(maxInput: bytes.length - 1)),
        _fails(ExportProblem.tooBig),
      );
      expect(const ExportLimits().maxInput, 100 * 1024 * 1024);
    });

    test('too many conversations or messages', () {
      final bytes = Uint8List.fromList(_json(_export));
      expect(
        () => _read(bytes, const ExportLimits(maxConversations: 1)),
        _fails(ExportProblem.tooBig),
      );
      expect(
        () => _read(bytes, const ExportLimits(maxMessages: 4)),
        _fails(ExportProblem.tooBig),
      );
    });

    test('JSON nested past the limit is refused before it is decoded', () {
      final deep = Uint8List.fromList(
        utf8.encode('${'[' * 100000}${']' * 100000}'),
      );
      expect(() => _read(deep), _fails(ExportProblem.unsafe));
      // Brackets inside strings don't count.
      final quoted = Uint8List.fromList(
        _json([
          _conversation(
            'a',
            '[[[[',
            '2026-01-01T00:00:00Z',
            messages: [('human', '{' * 200)],
          ),
        ]),
      );
      expect(_read(quoted).conversations.single.title, '[[[[');
    });

    test('a checkpoint that throws stops reading', () {
      expect(
        () => readClaudeExport(
          _with([]),
          checkpoint: () => throw const ExportCancelled(),
        ),
        throwsA(isA<ExportCancelled>()),
      );
    });
  });

  group('unsafe archives', () {
    void unsafe(Uint8List bytes, [ExportLimits? limits]) =>
        expect(() => _read(bytes, limits), _fails(ExportProblem.unsafe));

    test('paths outside the archive', () {
      for (final name in [
        '../conversations.json',
        'a/../../x',
        '/etc/passwd',
        r'a\..\x',
        'C:x',
        'a//b',
        './a',
        'a\u0000b',
      ]) {
        expect(
          () => BoundedZip.open(
            _zip([
              _Entry(name, [1]),
            ]),
          ),
          throwsA(isA<UnsafeArchive>()),
          reason: name,
        );
        unsafe(
          _with([
            _Entry(name, [1]),
          ]),
        );
      }
    });

    test('symbolic links', () {
      unsafe(_with([_Entry('link', utf8.encode('/etc'), mode: 0xA1FF)]));
      // A plain Unix file is fine.
      expect(
        _read(
          _with([
            _Entry('notes.txt', [1], mode: 0x81A4),
          ]),
        ).other,
        1,
      );
    });

    test('archives inside it', () {
      for (final name in ['inner.zip', 'a/b.tar', 'c.tgz', 'd.gz', 'e.7z']) {
        unsafe(
          _with([
            _Entry(name, [1]),
          ]),
        );
      }
    });

    test('encryption, ZIP64 and other packing', () {
      unsafe(
        _with([
          _Entry('x', [1], flags: 1),
        ]),
      );
      // The end record's count at ZIP64's 0xFFFF.
      unsafe(
        _zip([_Entry('conversations.json', _json(_export))], count: 0xFFFF),
      );
      unsafe(
        _with([
          _Entry('x', [1], size: 0xFFFFFFFF),
        ]),
      );
    });

    test('too many entries', () {
      unsafe(
        _with([
          _Entry('a', [1]),
          _Entry('b', [1]),
        ]),
        const ExportLimits(archive: ArchiveLimits(maxEntries: 2)),
      );
      expect(const ArchiveLimits().maxEntries, 1000);
    });

    test('a bomb: past its ratio, past its limits, or past what it said', () {
      final zeros = List.filled(4 * 1024 * 1024, 0);
      // 4 MB of zeros packs into a few KB: more than 100×.
      unsafe(_zip([_Entry('conversations.json', zeros, deflate: true)]));
      // Within the ratio, but past an entry's limit or the whole's.
      unsafe(
        _with([_Entry('a.bin', List.filled(4096, 7))]),
        const ExportLimits(archive: ArchiveLimits(maxEntry: 4000)),
      );
      unsafe(
        _with([_Entry('a.bin', List.filled(4096, 7))]),
        const ExportLimits(archive: ArchiveLimits(maxExpanded: 4000)),
      );
      // Saying it's small, then expanding far past that.
      unsafe(
        _zip([_Entry('conversations.json', zeros, deflate: true, size: 100)]),
      );
    });

    test('broken archives', () {
      final bytes = _with([]);
      unsafe(Uint8List.sublistView(bytes, 0, bytes.length - 30));
      // Its directory pointing past its end.
      final moved = Uint8List.fromList(bytes);
      ByteData.sublistView(moved)
          .setUint32(moved.length - 6, moved.length, Endian.little);
      unsafe(moved);
    });
  });

  group('reading a file in the background', () {
    late Directory folder;
    setUp(() => folder = Directory.systemTemp.createTempSync('imports_test'));
    tearDown(() => folder.deleteSync(recursive: true));

    String write(List<int> bytes) {
      final file = File('${folder.path}/export.zip')..writeAsBytesSync(bytes);
      return file.path;
    }

    test('answers the preview', () async {
      final preview = await readExportFile(write(_with([]))).result;
      expect(preview.conversations, hasLength(2));
    });

    test('says why it did not', () async {
      await expectLater(
        readExportFile(
          write(
            _with([
              _Entry('../x', [1]),
            ]),
          ),
        ).result,
        _fails(ExportProblem.unsafe),
      );
      await expectLater(
        readExportFile('${folder.path}/missing.zip').result,
        _fails(ExportProblem.unsupported),
      );
      await expectLater(
        readExportFile(
          write(_with([])),
          limits: const ExportLimits(maxInput: 10),
        ).result,
        _fails(ExportProblem.tooBig),
      );
    });

    test('stops when cancelled, or past its deadline', () async {
      final path = write(_with([]));
      final job = readExportFile(path)..cancel();
      await expectLater(job.result, throwsA(isA<ExportCancelled>()));
      await expectLater(
        readExportFile(path, deadline: Duration.zero).result,
        _fails(ExportProblem.unsafe),
      );
    });
  });

  group('the import', () {
    tearDown(() {
      exportFilePicker = null;
      exportReader = readExportFile;
    });

    Future<Harness> show(WidgetTester tester) async {
      final harness = Harness(Workspace());
      var closed = 0;
      await harness.show(
        tester,
        home: Center(
          child: SizedBox(
            // The width of a form in its window (showModal's).
            width: 432,
            height: 760,
            child: ImportExportForm(close: () => closed++),
          ),
        ),
      );
      return harness;
    }

    testWidgets('says how to get the export, then reads the one chosen', (
      tester,
    ) async {
      final job = _Job();
      final opened = <Uri>[];
      openPrivacyPage = (url) async {
        opened.add(url);
        return true;
      };
      addTearDown(() => openPrivacyPage = (_) async => false);
      exportFilePicker = () async => '/tmp/export.zip';
      String? asked;
      exportReader = (path) {
        asked = path;
        return job;
      };
      await show(tester);
      expect(
        find.text('In Claude, open Settings → Privacy and click Export data.'),
        findsOneWidget,
      );
      expect(
        find.textContaining(
          'Claktory reads it on this device and sends '
          'nothing until you choose conversations.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text("Open Claude's privacy settings"));
      expect(opened, [claudePrivacySettings]);

      await tester.tap(find.text('Choose export…'));
      await tester.pump();
      expect(asked, '/tmp/export.zip');
      expect(find.text('Reading your export…'), findsOneWidget);

      // Cancel stops it and goes back.
      await tester.tap(find.text('Cancel'));
      await tester.pump();
      expect(job.cancelled, isTrue);
      expect(find.text('Choose export…'), findsOneWidget);
      expect(find.text('Reading your export…'), findsNothing);
    });

    testWidgets('a preview with none selected; choosing some', (tester) async {
      final job = _Job();
      exportFilePicker = () async => '/tmp/export.zip';
      exportReader = (_) => job;
      await show(tester);
      await tester.tap(find.text('Choose export…'));
      await tester.pump();
      job.done(
        ExportPreview(
          conversations: readClaudeExport(_with([])).conversations,
          sha256: '',
          account: true,
          voice: 3,
          other: 2,
        ),
      );
      await settle(tester);
      expect(
        find.text(
          'Choose the conversations to bring in. None are selected '
          'yet.',
        ),
        findsOneWidget,
      );
      expect(find.text('2 conversations'), findsOneWidget);
      expect(find.text('Lisbon trip'), findsOneWidget);
      // No name: its first message stands in.
      expect(find.text('Why is the sky blue?'), findsOneWidget);
      expect(find.textContaining('3 messages'), findsOneWidget);
      expect(
        find.text(
          'Left out: your account details, 3 voice recordings, 2 other files.',
        ),
        findsOneWidget,
      );
      const own =
          'These are my own conversations, and I can share them with my '
          'agents.';
      expect(find.text(own), findsOneWidget);
      expect(_checked(tester, 'Lisbon trip'), isFalse);

      await tester.tap(find.text('Lisbon trip'));
      await tester.pump();
      expect(find.text('2 conversations · 1 selected'), findsOneWidget);
      expect(find.text('Import 1 conversation'), findsOneWidget);
      expect(_checked(tester, 'Lisbon trip'), isTrue);
      await tester.tap(find.text('Select all'));
      await tester.pump();
      expect(find.text('2 conversations · 2 selected'), findsOneWidget);
      expect(find.text('Import 2 conversations'), findsOneWidget);
      await tester.tap(find.text('Select none'));
      await tester.pump();
      expect(find.text('2 conversations'), findsOneWidget);
      await tester.tap(find.text(own));
      await tester.pump();
      expect(_checked(tester, own), isTrue);
    });

    testWidgets('on a phone, as a sheet, a long export scrolls in its list', (
      tester,
    ) async {
      final job = _Job();
      exportFilePicker = () async => '/tmp/export.zip';
      exportReader = (_) => job;
      final harness = Harness(Workspace());
      await harness.show(
        tester,
        size: const Size(393, 852),
        home: Builder(
          builder: (context) => Center(
            child: GestureDetector(
              onTap: () => showImportExport(context),
              child: const Text('Open'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      // The sheet rises.
      await pumpFrames(tester, const Duration(seconds: 1));
      await tester.tap(find.text('Choose export…'));
      await tester.pump();
      job.done(
        ExportPreview(
          conversations: [
            for (var i = 0; i < 40; i++)
              ConversationPreview(
                id: '$i',
                title: 'Conversation number $i, with a long name to cut',
                createdMs: DateTime(2026, 1, 1).millisecondsSinceEpoch,
                fromYou: 1,
                fromClaude: 1,
                snippet: '',
              ),
          ],
          sha256: '',
        ),
      );
      await settle(tester);
      expect(find.text('40 conversations'), findsOneWidget);
      // Only the rows in view are built.
      expect(find.textContaining('Conversation number'), findsWidgets);
      expect(
        find.text('Conversation number 39, with a long name to cut'),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('a file it can’t read says why, and another can be chosen', (
      tester,
    ) async {
      final job = _Job();
      exportFilePicker = () async => '/tmp/export.zip';
      exportReader = (_) => job;
      await show(tester);
      await tester.tap(find.text('Choose export…'));
      await tester.pump();
      job.fail(const ExportFailure(ExportProblem.severalPeople));
      await settle(tester);
      expect(
        find.text(exportProblemText(ExportProblem.severalPeople)),
        findsOneWidget,
      );
      expect(find.text('Choose export…'), findsOneWidget);
      for (final problem in ExportProblem.values) {
        expect(exportProblemText(problem), isNotEmpty);
      }
    });
  });
}

/// Whether the box [label] is in reads as checked to VoiceOver.
bool _checked(WidgetTester tester, String label) => tester
    .widget<Semantics>(
      find
          .ancestor(
            of: find.text(label),
            matching: find.byWidgetPredicate(
              (w) => w is Semantics && w.properties.checked != null,
            ),
          )
          .first,
    )
    .properties
    .checked!;

class _Job implements ExportJob {
  final _done = Completer<ExportPreview>();
  var cancelled = false;

  @override
  Future<ExportPreview> get result => _done.future;

  @override
  void cancel() {
    cancelled = true;
    if (!_done.isCompleted) _done.completeError(const ExportCancelled());
  }

  void done(ExportPreview preview) => _done.complete(preview);
  void fail(ExportFailure failure) => _done.completeError(failure);
}
