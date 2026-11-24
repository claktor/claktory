import 'dart:io';

import 'package:factory_app/diagnostics_record.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('Android keeps diagnostics beside code_cache, in no_backup', () {
    for (final app in ['com.claktory.app', 'com.claktory.app.dev']) {
      final data = '/data/user/0/$app';
      final folder = diagnosticsFolderFor(
        operatingSystem: 'android',
        temporary: Directory('$data/code_cache'),
      );
      expect(folder?.path, '$data/no_backup/diagnostics');
      // Selecting a folder must not create anything (diagnostics start off).
    }
  });

  test('the same private folder is used if the engine supplies cache', () {
    expect(
      diagnosticsFolderFor(
        operatingSystem: 'android',
        temporary: Directory('/data/user/10/com.claktory.app/cache'),
      )?.path,
      '/data/user/10/com.claktory.app/no_backup/diagnostics',
    );
  });

  test(
    'Android recording persists outside cache and clears only its files',
    () {
      final data = Directory.systemTemp.createTempSync('android_diagnostics');
      addTearDown(() => data.deleteSync(recursive: true));
      final cache = Directory('${data.path}/code_cache')..createSync();
      final private = Directory('${data.path}/no_backup')..createSync();
      final credentials = File('${private.path}/connections.json')
        ..writeAsStringSync('untouched credentials fixture');
      final voice = File('${private.path}/voice-not-sent/kept.json')
        ..parent.createSync(recursive: true)
        ..writeAsStringSync('untouched recording fixture');
      final folder = diagnosticsFolderFor(
        operatingSystem: 'android',
        temporary: cache,
      );
      expect(folder, isNotNull);
      final recorder = DiagnosticsRecorder(folder!)..select('test-endpoint');
      expect(recorder.on, isFalse);
      recorder.add(ConnectionKind.connect);
      expect(folder.existsSync(), isFalse);

      recorder.share(true);
      recorder.add(ConnectionKind.lifecycle, {
        ConnectionKey.app: AppLife.started,
      });
      recorder.add(ConnectionKind.connect, {ConnectionKey.attempt: 1});
      recorder.add(ConnectionKind.connected, {
        ConnectionKey.route: ConnectionRoute.direct,
      });
      recorder.add(ConnectionKind.disconnected, {
        ConnectionKey.why: ConnectionFailure.closed,
      });
      // An app update may clear code_cache; kept diagnostics must survive it.
      cache.deleteSync(recursive: true);
      final after = DiagnosticsRecorder(folder)..select('test-endpoint');
      expect(after.on, isTrue);
      expect(after.waiting.map((event) => event.kind), [
        'lifecycle',
        'connect',
        'connected',
        'disconnected',
      ]);
      after.sent(after.waiting.last.seq);
      expect(after.waitingCount, 0);
      after.add(ConnectionKind.connect);
      after.share(false);
      expect(after.waitingCount, 0);
      expect(folder.listSync().map((entry) => entry.path), [
        '${folder.path}/session',
      ]);
      expect(credentials.readAsStringSync(), 'untouched credentials fixture');
      expect(voice.readAsStringSync(), 'untouched recording fixture');
    },
  );

  test(
    'Apple paths are unchanged; unsupported platforms still do not record',
    () {
      expect(
        diagnosticsFolderFor(
          operatingSystem: 'ios',
          temporary: Directory('/container/tmp'),
        )?.path,
        '/container/Library/Application Support/diagnostics',
      );
      expect(
        diagnosticsFolderFor(
          operatingSystem: 'ios',
          temporary: Directory('/unexpected/cache'),
        ),
        isNull,
      );
      for (final dev in [false, true]) {
        expect(
          diagnosticsFolderFor(
            operatingSystem: 'macos',
            temporary: Directory('/unused'),
            home: '/test-home',
            dev: dev,
          )?.path,
          '/test-home/Library/Application Support/'
          '${dev ? 'Claktory Dev' : 'Claktory'}/diagnostics',
        );
      }
      for (final system in ['linux', 'windows', 'fuchsia']) {
        expect(
          diagnosticsFolderFor(
            operatingSystem: system,
            temporary: Directory('/unused'),
          ),
          isNull,
        );
      }
      expect(
        diagnosticsFolderFor(
          operatingSystem: 'macos',
          temporary: Directory('/unused'),
        ),
        isNull,
      );
    },
  );

  test('onDevice never writes to the real device from a unit test', () {
    expect(diagnosticsFolder(), isNull);
    expect(DiagnosticsRecorder.onDevice(), isNull);
  });
}
