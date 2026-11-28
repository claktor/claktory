// GetChecklist and ListTasks over the backend's connection: their answers
// are the asker's, never the open panel's; an agent with the extension off
// has none; an unknown agent is refused.

import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

/// The backend's end: Builder (a1) has a checklist and Tasks off.
class _Far {
  late final HttpServer server;

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        ServerMessage? answer;
        if (message.hasHello()) {
          answer = ServerMessage(welcome: Welcome(serverVersion: 'test'));
        } else if (message.hasGetChecklist() &&
            message.getChecklist.agentId == 'a1') {
          answer = ServerMessage(
            extensionView: ExtensionView(
              extensionId: 'checklist',
              agentId: 'a1',
              checklist: Checklist(items: [ChecklistItem(id: 3, text: 'Ship')]),
            ),
          );
        } else if (message.hasListTasks() &&
            message.listTasks.agentId == 'a1') {
          answer = ServerMessage(
            extensionView: ExtensionView(extensionId: 'tasks', agentId: 'a1'),
          );
        } else {
          answer = ServerMessage(error: Error(message: 'No agent "gone".'));
        }
        socket.add((answer..replyTo = message.id).writeToBuffer());
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

void main() {
  test('a checklist and tasks read for an agent are the asker\'s', () async {
    final far = _Far();
    await far.start();
    final panel = ExtensionViewStore();
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: panel,
      tunnels: FakeTunnels(FakeTunnel(0)),
      localEndpoint: 'ws://127.0.0.1:${far.server.port}',
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);

    final checklist = await backend.getChecklist('a1');
    expect(checklist?.items.single.text, 'Ship');
    expect(panel.view, isNull);

    expect(await backend.listTasks('a1'), isNull);
    expect(panel.view, isNull);

    await expectLater(
      backend.getChecklist('gone'),
      throwsA(isA<RequestFailure>()),
    );

    await backend.close();
    await far.close();
  });
}
