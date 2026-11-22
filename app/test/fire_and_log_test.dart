// fireAndLog (backend.dart): a request nothing waits on, whose refusal is
// only logged, even when it's a Future of a reply passed as one of void
// (SetActive to a backend from before it, say).
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('a refused request that answers a reply is only logged', () async {
    final uncaught = <Object>[];
    await runZonedGuarded(() async {
      final Future<ServerMessage> reply = Future.error(
        const RequestFailure('Unknown request.'),
      );
      fireAndLog(reply);
      await Future<void>.delayed(Duration.zero);
    }, (error, _) => uncaught.add(error));
    expect(uncaught, isEmpty);
  });

  test('any other failure still surfaces', () async {
    final uncaught = <Object>[];
    await runZonedGuarded(() async {
      fireAndLog(Future<ServerMessage>.error(StateError('a bug')));
      await Future<void>.delayed(Duration.zero);
    }, (error, _) => uncaught.add(error));
    expect(uncaught.single, isA<StateError>());
  });

  test('a request the backend refuses can be caught by a handler that '
      'returns nothing, as most callers do', () async {
    // A backend that welcomes, then refuses everything.
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        socket.add(
          (message.hasHello()
                  ? ServerMessage(
                      replyTo: message.id,
                      welcome: Welcome(serverVersion: 'test'),
                    )
                  : ServerMessage(
                      replyTo: message.id,
                      error: Error(message: 'Unknown request.'),
                    ))
              .writeToBuffer(),
        );
      });
    });
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);

    final uncaught = <Object>[];
    Object? caught;
    await runZonedGuarded(() async {
      await backend.selectAgent('kai').catchError((Object error) {
        caught = error;
      });
      fireAndLog(backend.watchFiles(''));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }, (error, _) => uncaught.add(error));
    expect(caught, isA<RequestFailure>());
    expect(uncaught, isEmpty);
    await backend.close();
    await server.close(force: true);
  });
}
