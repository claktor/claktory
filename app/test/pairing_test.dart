import 'dart:async';
import 'dart:io';

import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart' show AccountClaktory;
import 'package:factory_app/pairing.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/visible_codes.dart' show nameInText;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;

final _token = 'c' * 64;
final _code = 'factory://?t=$_token&name=mac-mini&e=${'a' * 64}';

/// A backend that answers a Hello with PairingPending and keeps the
/// connection open; [say] sends it what it says next. What else it's sent
/// goes in [heard].
Future<(HttpServer, Future<void> Function(ServerMessage) say)> pendingServer({
  List<ClientMessage>? heard,
}) async {
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
      } else {
        heard?.add(message);
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
      // Long enough that the PairingPending answering Hello comes in
      // time on a busy machine, which a 300 ms one sometimes missed.
      ..connectWithin = const Duration(seconds: 1);
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
    await Future<void>.delayed(const Duration(milliseconds: 2500));
    expect(backend.pairing.value, isNotNull);

    await say(ServerMessage(welcome: Welcome(deviceId: 'd9')));
    await until(() => backend.connected.value);
    expect(backend.connected.value, isTrue);
    expect(backend.pairing.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test('Cancel takes the request back on the connection that made it '
      '(#603)', () async {
    final heard = <ClientMessage>[];
    final (server, _) = await pendingServer(heard: heard);
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..connectWithin = const Duration(seconds: 1);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.pairing.value != null);
    backend.withdrawPairing();
    await until(() => heard.any((message) => message.hasWithdrawPairing()));
    expect(
      heard.where((message) => message.hasWithdrawPairing()),
      hasLength(1),
    );
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

  // The code's name is what the waiting page says, as the strip and the
  // confirm sheet do, never the backend's hostname (Zucchero's row 720).
  final named = 'factory://?t=$_token&name=Matt%27s%20Mac%20mini&e=${'a' * 64}';

  Future<Harness> waiting(WidgetTester tester, {String? code}) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: const Size(393, 852));
    harness.connections.use(parsePairingCode(code ?? named));
    harness.backend.connected.value = false;
    harness.backend.pairing.value = PairingWait(
      PairingPending(deviceId: 'd9', name: 'iPhone', machine: 'mac-mini-2'),
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
      // Since #546 the person types this number there (Trish).
      find.text(
        "Allow this iPhone on Matt's Mac mini, or on another device that "
        "uses Matt's Mac mini, by typing the number this iPhone shows "
        'there:',
      ),
      findsOneWidget,
    );
    expect(find.text('0427'), findsOneWidget);
    expect(harness.backend.sent, isEmpty);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    // Taken back there too, so it leaves that Claktory's list at once
    // (#603).
    expect(
      harness.backend.sent.where((message) => message.hasWithdrawPairing()),
      hasLength(1),
    );
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
    // Turned away, there's nothing there to take back.
    expect(
      harness.backend.sent.where((message) => message.hasWithdrawPairing()),
      isEmpty,
    );
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('a code without a name: the backend\'s hostname, not an '
      'address', (tester) async {
    await waiting(
      tester,
      code: 'factory://10.0.0.5:7420?t=$_token&fp=${'b' * 64}',
    );
    expect(find.text('Waiting for mac-mini-2'), findsOneWidget);
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

  testWidgets("a Claktory's name from the account reads as a name on the "
      "waiting page: what reorders it shows by its code (row 1784)", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    await harness.show(tester);
    harness.connections.use(
      RemoteFactory.fromAccount(
        AccountClaktory(
          id: 'c3',
          name: 'Evil\u202EMac',
          endpointId: 'a' * 64,
          relays: const ['https://relay.example.com'],
        ),
      ),
    );
    harness.backend.pairing.value = PairingWait(
      PairingPending(deviceId: 'd9', name: 'Mac', machine: 'studio-host'),
      '0427',
    );
    await pumpFrames(tester);
    final shown = nameInText('Evil\u202EMac');
    expect(find.text('Waiting for $shown'), findsOneWidget);
    expect(find.textContaining('\u202E'), findsNothing);
  });

  testWidgets('a Mac waiting for another Claktory shows the number in its '
      'content area, and where to type it', (tester) async {
    // A Mac's words ("this Mac"); tests run as Android otherwise.
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    await harness.show(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=$_token&name=Studio&e=${'a' * 64}'),
    );
    harness.backend.pairing.value = PairingWait(
      PairingPending(deviceId: 'd9', name: 'Mac', machine: 'studio-host'),
      '0427',
    );
    await pumpFrames(tester);
    expect(find.text('Waiting for Studio'), findsOneWidget);
    expect(
      find.text(
        'Allow this Mac on Studio, or on another device that uses Studio, '
        'by typing the number this Mac shows there:',
      ),
      findsOneWidget,
    );
    expect(find.text('0427'), findsOneWidget);
    // Never a command to run: a terminal can't allow it.
    expect(find.textContaining('factoryd allow'), findsNothing);
    expect(
      find.text(
        'No other device connected yet? Use one that’s signed in to your '
        'Claktory account.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
  });
}
