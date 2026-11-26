// Matt's MacBook (Bruno 36927): an agent clicked while the connection died
// under it was lost, and the view stayed on the agent before. Now the
// chosen agent shows at once, and the choice goes again by itself once the
// connection is back.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

/// A Claktory with two agents that drops the connection, without
/// answering, at the first SelectAgent while [drop], refuses one for an
/// agent in [refuse], and answers everything else, its Workspace after.
class _Claktory {
  final got = <ClientMessage>[];
  var selected = 'ann';
  var drop = false;
  var dropped = false;
  final refuse = <String>{};

  Workspace get workspace => Workspace(
    agents: [
      Agent(id: 'ann', name: 'Ann'),
      Agent(id: 'bob', name: 'Bob'),
    ],
    selectedAgentId: selected,
  );
}

Future<HttpServer> _serve(_Claktory claktory) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      void send(ServerMessage answer) => socket.add(answer.writeToBuffer());
      void reply(ServerMessage answer) => send(answer..replyTo = message.id);
      if (message.hasHello()) {
        reply(ServerMessage(welcome: Welcome(serverVersion: 'test')));
        // What it has selected still: the agent before.
        send(ServerMessage(workspace: claktory.workspace));
        return;
      }
      claktory.got.add(message);
      if (message.hasSelectAgent()) {
        final id = message.selectAgent.agentId;
        if (claktory.drop && !claktory.dropped) {
          claktory.dropped = true;
          unawaited(socket.close());
          return;
        }
        if (claktory.refuse.contains(id)) {
          return reply(
            ServerMessage(error: Error(message: 'No such agent: $id')),
          );
        }
        claktory.selected = id;
        send(ServerMessage(workspace: claktory.workspace));
      }
      reply(ServerMessage(ack: Ack()));
    });
  });
  return server;
}

void main() {
  late _Claktory claktory;
  late HttpServer server;
  late Backend backend;

  Future<void> until(bool Function() done) async {
    for (var i = 0; i < 500 && !done(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  String shown() => backend.workspace.workspace.selectedAgentId;
  int selects() => claktory.got.where((m) => m.hasSelectAgent()).length;

  setUp(() async {
    claktory = _Claktory();
    server = await _serve(claktory);
    backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    await until(() => backend.workspace.received);
    expect(shown(), 'ann');
  });

  tearDown(() async {
    await backend.close();
    await server.close(force: true);
  });

  test('the chosen agent shows at once', () async {
    final selecting = backend.selectAgent('bob');
    expect(shown(), 'bob', reason: 'before any answer');
    await selecting;
    expect(shown(), 'bob');
    expect(claktory.selected, 'bob');
  });

  test('cut off by the connection, it stays shown, and goes again by '
      'itself once the connection is back, whatever the Workspace before '
      'it says', () async {
    claktory.drop = true;
    await backend.selectAgent('bob');
    expect(shown(), 'bob');
    await until(() => selects() > 1 && claktory.selected == 'bob');
    expect(selects(), 2);
    expect(claktory.selected, 'bob');
    // The reconnect's Workspace said Ann: Bob stayed shown throughout.
    expect(shown(), 'bob');
  });

  test('the latest choice goes, once', () async {
    claktory.drop = true;
    await backend.selectAgent('bob');
    await until(() => !backend.connected.value);
    unawaited(backend.selectAgent('ann').catchError((_) {}));
    await backend.selectAgent('bob');
    await until(() => backend.connected.value && claktory.selected == 'bob');
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(claktory.got.last.selectAgent.agentId, 'bob');
    expect(shown(), 'bob');
  });

  test('refused, the agent before shows again', () async {
    claktory.refuse.add('bob');
    await expectLater(
      backend.selectAgent('bob'),
      throwsA(isA<RequestFailure>()),
    );
    expect(shown(), 'ann');
  });
}
