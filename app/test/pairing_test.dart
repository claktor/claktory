import 'dart:async';
import 'dart:io';

import 'package:factory_app/connections.dart';
import 'package:factory_app/pairing.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;

final _token = 'c' * 64;
final _code = 'factory://?t=$_token&name=mac-mini&e=${'a' * 64}';

/// A backend that answers a Hello with PairingPending and keeps the
/// connection open; [say] sends it what it says next.
Future<(HttpServer, Future<void> Function(ServerMessage) say)>
pendingServer() async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  final socket = Completer<WebSocket>();
  var hello = Int64.ZERO;
  server.listen((request) async {
    final ws = await WebSocketTransformer.upgrade(request);
    ws.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      if (message.hasHello()) {
        hello = message.id;
        ws.add(
          ServerMessage(
            replyTo: hello,
            pairingPending: PairingPending(
              deviceId: 'd9',
              name: 'iPhone',
              machine: "Matt's Mac mini",
              expiresMs: Int64(DateTime.now().millisecondsSinceEpoch + 600000),
            ),
          ).writeToBuffer(),
        );
        if (!socket.isCompleted) socket.complete(ws);
      }
    });
  });
  Future<void> say(ServerMessage message) async =>
      (await socket.future).add((message..replyTo = hello).writeToBuffer());
  return (server, say);
}

Future<void> until(bool Function() done) async {
  for (var i = 0; i < 100 && !done(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  test("the pairing number is the backend's example", () {
    expect(
      pairingNumber(
        backendId: 'a' * 64,
        deviceId: 'b' * 64,
        token: '0123456789abcdef',
      ),
      '0722',
    );
    // Another device's key gives other digits.
    expect(
      pairingNumber(
        backendId: 'a' * 64,
        deviceId: 'c' * 64,
        token: '0123456789abcdef',
      ),
      isNot('0722'),
    );
  });

  test('a device waiting to be allowed shows the number, waits past the '
      'welcome timeout, and connects once allowed', () async {
    final (server, say) = await pendingServer();
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..connectWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.pairing.value != null);
    final wait = backend.pairing.value!;
    expect(wait.pending.machine, "Matt's Mac mini");
    // From the ids the tunnel gives (64 "b"s for this device, 64 "a"s for
    // the backend) and the code's token.
    expect(
      wait.number,
      pairingNumber(backendId: 'a' * 64, deviceId: 'b' * 64, token: _token),
    );
    expect(backend.connected.value, isFalse);

    // Well past the welcome timeout, it's still waiting on the same
    // connection.
    await Future<void>.delayed(const Duration(milliseconds: 800));
    expect(backend.pairing.value, isNotNull);

    await say(ServerMessage(welcome: Welcome(deviceId: 'd9')));
    await until(() => backend.connected.value);
    expect(backend.connected.value, isTrue);
    expect(backend.pairing.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test('turned away while waiting, it says why and stops', () async {
    final (server, say) = await pendingServer();
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.pairing.value != null);
    await say(
      ServerMessage(
        error: Error(
          code: ErrorCode.ERROR_CODE_UNAUTHORIZED,
          message: "Matt's Mac mini didn't allow this device.",
        ),
      ),
    );
    await until(() => backend.refused.value != null);
    expect(backend.refused.value, "Matt's Mac mini didn't allow this device.");
    // The waiting screen stays, to say so.
    expect(backend.pairing.value, isNotNull);
    await backend.close();
    await server.close(force: true);
  });

  Future<Harness> waiting(WidgetTester tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: const Size(393, 852));
    harness.connections.use(parsePairingCode(_code));
    harness.backend.connected.value = false;
    harness.backend.pairing.value = PairingWait(
      PairingPending(
        deviceId: 'd9',
        name: 'iPhone',
        machine: "Matt's Mac mini",
      ),
      '0427',
    );
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('on the phone, waiting shows the number to compare, and '
      'Cancel goes back to pairing', (tester) async {
    final harness = await waiting(tester);
    expect(find.text("Waiting for Matt's Mac mini"), findsOneWidget);
    expect(
      find.text(
        "Allow this iPhone on Matt's Mac mini. Its screen shows the same "
        'number:',
      ),
      findsOneWidget,
    );
    expect(find.text('0427'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
    expect(find.byType(PairScreen), findsOneWidget);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('turned away, the phone says why and offers a new code', (
    tester,
  ) async {
    final harness = await waiting(tester);
    harness.backend.refused.value =
        "Matt's Mac mini didn't allow this device. To try again, add it "
        "again on Matt's Mac mini in Settings → Devices.";
    await pumpFrames(tester);
    expect(find.textContaining("didn't allow this device"), findsOneWidget);
    expect(find.text("Couldn't connect to Matt's Mac mini"), findsOneWidget);
    expect(find.text('0427'), findsNothing);
    await tester.tap(find.text('Use a new code'));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('allowed, the phone shows its Claktory', (tester) async {
    final harness = await waiting(tester);
    harness.backend.pairing.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.text("Waiting for Matt's Mac mini"), findsNothing);
    expect(harness.connections.current, isNotNull);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('a Mac waiting for another Claktory shows the number in its '
      'content area', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    harness.connections.use(parsePairingCode(_code));
    harness.backend.pairing.value = PairingWait(
      PairingPending(deviceId: 'd9', name: 'Mac', machine: 'Studio'),
      '0427',
    );
    await pumpFrames(tester);
    expect(find.text('Waiting for Studio'), findsOneWidget);
    expect(
      find.text('Allow this Mac on Studio. Its screen shows the same number:'),
      findsOneWidget,
    );
    expect(find.text('0427'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
  });
}
