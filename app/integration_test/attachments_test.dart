// Files sent with a message, against a real Claktory: a 2.5 MB file and a
// picture upload in chunks, go with the message, and come back in the
// conversation; the backend has them, byte for byte.
//
// On a Mac, against Claktory Dev:
//   cd app && flutter test integration_test/attachments_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true [--dart-define=AGENT=Builder]
import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show waitFor;

const _agent = String.fromEnvironment('AGENT', defaultValue: 'Builder');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('files upload, go with the message and come back', (
    tester,
  ) async {
    final folder = Directory.systemTemp.createTempSync('attach');
    final big = File('${folder.path}/big.bin')
      ..writeAsBytesSync([for (var i = 0; i < 2600000; i++) i % 253]);
    // A 1×1 PNG.
    final picture = File('${folder.path}/dot.png')
      ..writeAsBytesSync(const [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, //
        0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, //
        68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9, 251, 3, 253, 227, 85, //
        242, 156, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
      ]);

    await app.main();
    await waitFor(
      tester,
      find.text(_agent),
      timeout: const Duration(minutes: 1),
    );
    final backend = AppScope.scope(tester.element(find.byType(Sidebar).first))
        .backend;
    final id = backend.workspace.workspace.agents
        .firstWhere((agent) => agent.name == _agent)
        .id;
    await tester.tap(find.text(_agent).first);
    await waitFor(tester, find.byType(EditableText));

    final stamp = DateTime.now().millisecondsSinceEpoch;
    final text = 'Attachment check $stamp: no reply needed.';
    final message = backend.outbox.send(
      id,
      text,
      files: [
        for (final file in [big, picture])
          LocalFile(
            path: file.path,
            name: file.uri.pathSegments.last,
            size: file.lengthSync(),
          ),
      ],
    );
    // On its way, at once, its files uploading.
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text(text), findsOneWidget);
    expect(find.text('big.bin'), findsWidgets);

    final end = DateTime.now().add(const Duration(minutes: 2));
    while (backend.outbox.of(id).contains(message)) {
      if (DateTime.now().isAfter(end)) fail('still on its way');
      expect(message.failure, isNull);
      await tester.pump(const Duration(milliseconds: 200));
    }

    // In the conversation, with its files.
    await waitFor(tester, find.text(text));
    final entry = [
      for (var i = 0; i < backend.transcript.length; i++)
        backend.transcript[i].entry,
    ].lastWhere((entry) => entry.hasUser() && entry.user.text == text);
    final files = entry.user.attachments;
    // Named as sent, or "big 2.bin" when one by that name came today.
    expect(files, hasLength(2));
    expect(files.first.name, startsWith('big'));
    expect(files.last.name, startsWith('dot'));
    expect(find.text(files.first.name), findsOneWidget);
    expect(find.byType(Image), findsWidgets);

    // The backend has them, whole.
    final stored = await backend.readFile(
      files.first.path,
      maxBytes: 4 * 1024 * 1024,
    );
    expect(stored.error, isEmpty);
    expect(stored.size.toInt(), big.lengthSync());
    // ReadFile sends at most its own cap; what it sent matches.
    final bytes = big.readAsBytesSync();
    expect(stored.data, bytes.sublist(0, stored.data.length));
    // ignore: avoid_print
    print('read back ${stored.data.length} of ${stored.size} bytes');
    // ignore: avoid_print
    print('stored at ${files.first.path} and ${files.last.path}');
    folder.deleteSync(recursive: true);
  });
}
