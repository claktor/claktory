import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connection.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

/// A Claktory that moved away (#243): it answers every Hello with
/// ERROR_CODE_MOVED and keeps the connection open, deletes its copy on
/// DeleteMovedCopy with "delete", then closes it.
class _Moved {
  late final HttpServer server;
  var hellos = 0;
  var deletedMs = 0;
  final words = <String>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          hellos++;
          socket.add(
            ServerMessage(
              replyTo: message.id,
              error: Error(
                code: ErrorCode.ERROR_CODE_MOVED,
                message: 'This Claktory moved to hetzner-box.',
                moved: MovedAway(
                  to: 'hetzner-box',
                  toOs: 'linux',
                  atMs: Int64(1000),
                  machine: 'mac-mini',
                  deletedMs: Int64(deletedMs),
                ),
              ),
            ).writeToBuffer(),
          );
        } else if (message.hasDeleteMovedCopy()) {
          final word = message.deleteMovedCopy.typedWord;
          words.add(word);
          if (word != 'delete') {
            socket.add(
              ServerMessage(
                replyTo: message.id,
                error: Error(
                  code: ErrorCode.ERROR_CODE_INVALID_REQUEST,
                  message: 'Type delete to delete the copy.',
                ),
              ).writeToBuffer(),
            );
            return;
          }
          deletedMs = 2000;
          socket.add(
            ServerMessage(replyTo: message.id, ack: Ack()).writeToBuffer(),
          );
          unawaited(socket.close());
        }
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

void main() {
  test('a Claktory that moved away says where, keeps the connection open '
      'for deleting the copy, and says when it was deleted', () async {
    final moved = _Moved();
    await moved.start();
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: FakeTunnels(FakeTunnel(0)),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${moved.server.port}',
    )..connectWithin = const Duration(seconds: 2);
    unawaited(backend.run());
    await until(() => backend.movedAway.value != null, seconds: 20);
    expect(backend.movedAway.value!.to, 'hetzner-box');
    expect(backend.movedAway.value!.deletedMs.toInt(), 0);
    expect(backend.connected.value, isFalse);
    expect(Connection(backend).link, Link.moved);
    expect(Connection(backend).line, 'Moved to hetzner-box');

    // Not given up on for want of a welcome: the same connection takes
    // the request, past the 2s the app waits for a welcome (long enough
    // that a loaded machine answers the first Hello within it).
    final hellos = moved.hellos;
    await Future<void>.delayed(const Duration(milliseconds: 2500));
    expect(moved.hellos, hellos);

    await expectLater(
      backend.deleteMovedCopy('nope'),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          'Type delete to delete the copy.',
        ),
      ),
    );
    await backend.deleteMovedCopy('delete');
    expect(moved.words, ['nope', 'delete']);
    await until(
      () => (backend.movedAway.value?.deletedMs.toInt() ?? 0) > 0,
      seconds: 20,
    );
    expect(moved.hellos, greaterThan(hellos));

    // Another Claktory picked: what this one said goes.
    backend.connectTo(null);
    expect(backend.movedAway.value, isNull);
    await backend.close();
    await moved.close();
  });
}
