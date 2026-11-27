// SetActive says whether this device shows notifications (item 13,
// backend.dart's Backend.notificationPermission): on every new connection
// once the device knows, in use or not, since Notify goes to the
// connections not in use; again when it changes (resendActive). A device
// that never says (a Mac, until it does) sends nothing more than before.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

/// A backend that welcomes and acks everything, keeping each SetActive.
Future<(Backend, List<SetActive>)> _connect() async {
  final said = <SetActive>[];
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  addTearDown(() => server.close(force: true));
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      if (message.hasSetActive()) said.add(message.setActive);
      socket.add(
        (message.hasHello()
                ? ServerMessage(
                    replyTo: message.id,
                    welcome: Welcome(serverVersion: 'test'),
                  )
                : ServerMessage(replyTo: message.id, ack: Ack()))
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
  return (backend, said);
}

Future<void> _settle() =>
    Future<void>.delayed(const Duration(milliseconds: 100));

void main() {
  const denied = NotificationPermission.NOTIFICATION_PERMISSION_DENIED;
  const allowed = NotificationPermission.NOTIFICATION_PERMISSION_ALLOWED;
  tearDown(
    () =>
        Backend.notificationPermission = () =>
            NotificationPermission.NOTIFICATION_PERMISSION_UNSPECIFIED,
  );

  test("a device that doesn't say sends nothing while not in use, as "
      'before', () async {
    final (_, said) = await _connect();
    await _settle();
    expect(said, isEmpty);
  });

  test('a device that knows says so as it connects, not in use too, and '
      'again when it changes', () async {
    var permission = denied;
    Backend.notificationPermission = () => permission;
    final (backend, said) = await _connect();
    await _settle();
    expect(said.single.active, isFalse);
    expect(said.single.notifications, denied);

    permission = allowed;
    backend.resendActive();
    await _settle();
    expect(said.last.active, isFalse);
    expect(said.last.notifications, allowed);

    // In use: said with it.
    backend.setActive(true);
    await _settle();
    expect(said.last.active, isTrue);
    expect(said.last.notifications, allowed);
  });
}
