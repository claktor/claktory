import 'dart:io';
import 'dart:typed_data';

import 'package:factory_app/backend.dart' show localPort;
import 'package:factory_app/local_socket.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// A WebSocket echo server on a Unix socket of its own, and what reached
/// it.
Future<(String, List<Object>)> _echo() async {
  final folder = await Directory.systemTemp.createTemp('sock');
  addTearDown(() => folder.delete(recursive: true));
  final path = '${folder.path}/app.sock';
  final server = await HttpServer.bind(
    InternetAddress(path, type: InternetAddressType.unix),
    0,
  );
  addTearDown(() => server.close(force: true));
  final heard = <Object>[];
  server.listen((request) async {
    heard.add(request.uri);
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      heard.add(frame as Object);
      socket.add(frame);
    });
  });
  return (path, heard);
}

void main() {
  test(
    "a debug build or a test talks to the dev Claktory, not the person's",
    () {
      // Tests run in debug mode: only a release build defaults to ~/.factory.
      expect(showsDevClaktory, isTrue);
      expect(
        localHome(environment: {'HOME': '/Users/me'}),
        '/Users/me/.factory-dev',
      );
      expect(
        localHome(environment: {'HOME': '/Users/me', 'FACTORY_HOME': '/q/h'}),
        '/q/h',
      );
    },
  );

  test("Claktory's socket is in .factory in the home folder the user "
      'database says, never by the environment (Fugo)', () async {
    expect(
      await localClaktorySocket(dev: false, userHome: () async => '/Users/me'),
      '/Users/me/.factory/app.sock',
    );
    // Too long for a socket: in the temporary folder the native side says.
    final long = '/Users/${'m' * 100}';
    expect(
      await localClaktorySocket(
        dev: false,
        userHome: () async => long,
        userTemp: () async => '/var/folders/x/T/',
      ),
      localSocketPath('$long/.factory', temp: '/var/folders/x/T/'),
    );
    await expectLater(
      localClaktorySocket(
        dev: false,
        userHome: () async => long,
        userTemp: () async => null,
      ),
      throwsA(isA<LocalHomeUnknown>()),
    );
    // Nothing said: not looked for anywhere else.
    for (final none in [null, '', 'relative']) {
      await expectLater(
        localClaktorySocket(dev: false, userHome: () async => none),
        throwsA(isA<LocalHomeUnknown>()),
      );
    }
    // Claktory Dev goes by its environment.
    expect(await localClaktorySocket(dev: true), localSocketPath(localHome()));
  });

  test("its socket is in its data folder, or in the user's temporary folder "
      'when that path is too long for a socket', () {
    expect(
      localSocketPath('/Users/me/.factory'),
      '/Users/me/.factory/app.sock',
    );
    final long = '/Users/me/${'a' * 100}';
    final path = localSocketPath(long, temp: '/var/folders/x/T/');
    // sha256 of the home path's bytes, its first 16 hex digits, as
    // factoryd's authority::socket_path makes it (`shasum -a 256`).
    expect(path, '/var/folders/x/T/claktory-fbcaaecdc9cf8b04/app.sock');
    expect(localSocketPath(long, temp: '/var/folders/x/T'), path);
    // 103 bytes fit; 104 don't.
    final fits = '/${'b' * (103 - '/app.sock'.length - 1)}';
    expect(localSocketPath(fits), '$fits/app.sock');
    expect(localSocketPath('${fits}c', temp: '/t'), startsWith('/t/claktory-'));
  });

  test(
    'it talks WebSocket over the socket, once its backend is checked',
    () async {
      final (path, _) = await _echo();
      final checked = <Socket>[];
      final socket = await connectLocalSocket(
        path: path,
        check: (socket) async {
          checked.add(socket);
          return null;
        },
      );
      expect(checked, hasLength(1));
      socket.add([1, 2, 3]);
      expect(await socket.first, [1, 2, 3]);
      await socket.close();
    },
  );

  test("a backend that isn't Claktory's own is told nothing (Fugo's layer "
      '2)', () async {
    final (path, heard) = await _echo();
    await expectLater(
      connectLocalSocket(path: path, check: (_) async => 'not Claktory'),
      throwsA(anything),
    );
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(heard, isEmpty, reason: 'not even the WebSocket upgrade');
  });

  group("the Mac app's check", () {
    TestWidgetsFlutterBinding.ensureInitialized();
    final asked = <Map<Object?, Object?>>[];
    setUp(() {
      asked.clear();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(const MethodChannel('factory/device'), (
            call,
          ) async {
            if (call.method != 'checkBackendPeer') return null;
            asked.add(call.arguments as Map<Object?, Object?>);
            return asked.length == 1 ? null : 'it runs as another user (0)';
          });
    });
    tearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          ),
    );

    test("gives the native side the peer's credentials and audit token, "
        'read off the connection, and goes by its answer', () async {
      final (path, heard) = await _echo();
      final socket = await connectLocalSocket(path: path);
      await socket.close();
      final uid = int.parse(
        (Process.runSync('id', ['-u']).stdout as String).trim(),
      );
      final credentials = asked.single['credentials'] as Uint8List;
      final token = asked.single['token'] as Uint8List;
      expect(credentials, hasLength(76));
      // xucred: its version, then the peer's user: this test's own.
      expect(
        ByteData.sublistView(credentials).getUint32(4, Endian.little),
        uid,
      );
      expect(token, hasLength(32));
      // audit_token_t: the peer's user is its second word.
      expect(ByteData.sublistView(token).getUint32(4, Endian.little), uid);
      expect(asked.single['dev'], isTrue);

      heard.clear();
      await expectLater(connectLocalSocket(path: path), throwsA(anything));
      expect(heard, isEmpty);
    });
  });

  test("Windows's debug build or a test talks to the dev Claktory, not the "
      "person's", () {
    // Tests run in debug mode: only a release build defaults to 7420.
    expect(localPort, '7421');
  });

  // A Mac reaches its own over the socket; only Windows's loopback
  // connection (backend.dart's localPort) names a port.
  test('nothing else in the app names the real port', () {
    final named = [
      for (final file in Directory('lib').listSync(recursive: true))
        if (file is File &&
            file.path.endsWith('.dart') &&
            !file.path.contains('/protocol/') &&
            !file.path.endsWith('/backend.dart') &&
            file.readAsLinesSync().any(
              (line) =>
                  !line.trimLeft().startsWith('//') && line.contains('7420'),
            ))
          file.path,
    ];
    expect(named, isEmpty);
  });
}
