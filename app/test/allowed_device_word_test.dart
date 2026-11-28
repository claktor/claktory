// ProveAllowedDevice (the no-code setup): the device asks its
// Claktory for the signed word that it's allowed there, to send with its
// approval of a new Claktory; the answer comes back to the call that asked.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

/// The backend's end: welcomes, answers ProveAllowedDevice with
/// [answer]'s word for the request, and records what it was asked.
class _Far {
  _Far(this.answer);

  final ServerMessage Function(ClientMessage) answer;
  late final HttpServer server;
  final asked = <ClientMessage>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
          return;
        }
        asked.add(message);
        socket.add((answer(message)..replyTo = message.id).writeToBuffer());
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

Future<(Backend, _Far)> _connect(
  ServerMessage Function(ClientMessage) answer,
) async {
  final far = _Far(answer);
  await far.start();
  final backend = Backend(
    workspace: WorkspaceStore(),
    transcript: TranscriptStore(),
    explorer: ExplorerStore(),
    extensionView: ExtensionViewStore(),
    tunnels: FakeTunnels(FakeTunnel(0)),
    localEndpoint: 'ws://127.0.0.1:${far.server.port}',
  );
  unawaited(backend.run());
  await until(() => backend.connected.value);
  return (backend, far);
}

void main() {
  test('the word comes back to the call that asked, for its request', () async {
    final (backend, far) = await _connect(
      (message) => ServerMessage(
        allowedDeviceWord: AllowedDeviceWord(
          requestId: message.proveAllowedDevice.requestId,
          allowed: 'signed-word',
        ),
      ),
    );
    expect(await backend.proveAllowedDevice('auth-1'), 'signed-word');
    expect(far.asked.single.proveAllowedDevice.requestId, 'auth-1');
    await backend.close();
    await far.close();
  });

  test("a refusal is the request's failure; a word for another request "
      'is no answer', () async {
    var refuse = true;
    final (backend, far) = await _connect(
      (message) => refuse
          ? ServerMessage(
              error: Error(
                code: ErrorCode.ERROR_CODE_SIGN_IN_REQUIRED,
                message: 'Sign in to your Claktory account first.',
              ),
            )
          : ServerMessage(
              allowedDeviceWord: AllowedDeviceWord(
                requestId: 'another',
                allowed: 'signed-word',
              ),
            ),
    );
    await expectLater(
      backend.proveAllowedDevice('auth-1'),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          'Sign in to your Claktory account first.',
        ),
      ),
    );
    refuse = false;
    await expectLater(
      backend.proveAllowedDevice('auth-1'),
      throwsA(isA<RequestFailure>()),
    );
    await backend.close();
    await far.close();
  });
}
