// An iPad's PDFs from a Claktory on another machine (always, on an iPad)
// are kept with the other files this device keeps (#446; Settings →
// Storage counts them and Clear removes them), not in the render cache:
// opened again, the file's version is asked with a one-byte read, and the
// copy shown is held, so neither Clear nor the storage limit takes it from
// under the view.
import 'dart:io';

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/file_cache.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import '../panes_test.dart' show leaf, row, snapshot, split;
import 'as_ipad.dart';
import 'document_native.dart';

void main() {
  late Directory folder;

  setUp(() {
    folder = Directory.systemTemp.createTempSync('kept-ipad-pdf');
    keepsFilesOf = (_) => true;
    fileCacheFolder = () async => folder.path;
  });

  tearDown(() {
    keepsFilesOf = (backend) => backend.target != null;
    FileCache.forgetAllInMemory();
    folder.deleteSync(recursive: true);
  });

  /// Real work on the disk, between frames.
  Future<void> working(WidgetTester tester) async {
    for (var i = 0; i < 30; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 20)),
      );
      await tester.pump();
    }
  }

  List<ReadFile> reads(Harness harness) => [
    for (final message in harness.backend.sent)
      if (message.hasReadFile()) message.readFile,
  ];

  String shownFile(DocumentNative native) =>
      (native.created.last['document']! as Map)['file']! as String;

  Workspace withPdf() => snapshot(
    layout: split('s1', row, [
      leaf('chat', 'chat'),
      leaf('paper', 'pdf', documentPath),
    ]),
  );

  testWidgets('kept with the files this device keeps; opened again, only a '
      'one-byte read; Clear waits until the pane lets it go', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester, asking: false);
    await working(tester);

    final shown = shownFile(native);
    expect(shown, startsWith(folder.path));
    expect(File(shown).readAsBytesSync(), documentPdf().data);
    // A byte first, for its version; then the whole from its start.
    final first = reads(harness);
    expect(first.first.maxBytes.toInt(), 1);
    expect(first.skip(1).first.offset.toInt(), 0);

    // Closed and opened again: the copy kept, after one byte.
    final before = first.length;
    harness.backend.workspace.apply(snapshot());
    await working(tester);
    harness.backend.workspace.apply(withPdf());
    await working(tester);
    expect(reads(harness).skip(before).map((r) => r.maxBytes.toInt()), [1]);
    expect(shownFile(native), shown);

    // Clear while it shows: kept on disk till the pane goes.
    await tester.runAsync(() => FileCache.of(harness.backend)!.clear());
    expect(File(shown).existsSync(), isTrue);
    harness.backend.workspace.apply(snapshot());
    await working(tester);
    expect(File(shown).existsSync(), isFalse);
  });

  testWidgets('a new version replaces the copy kept, and the one before '
      'goes once the new one shows', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester, asking: false);
    await working(tester);
    await native.ready();
    await working(tester);
    final was = shownFile(native);

    final file = documentPdf('a paper on curlews, revised');
    harness.backend
      ..files[documentPath] = file
      ..fileChanged.value = FileChanged(
        path: documentPath,
        sha256: file.sha256,
      );
    await working(tester);
    final reload = native.calls.lastWhere(
      (call) => call.method == 'reloadDocument',
    );
    final now =
        ((reload.arguments as Map)['document'] as Map)['file'] as String;
    expect(now, startsWith(folder.path));
    expect(now, isNot(was));
    expect(File(now).readAsBytesSync(), file.data);
    expect(File(was).existsSync(), isFalse);
  });
}
