// #214's diagnostics, the device's side: while the Claktory has Advanced
// diagnostics on, the connection's events are recorded on the device
// (remembered, so a device that can't connect still records), sent once
// connected, and gone from the device once the Claktory kept them; turned
// off, the device stops and deletes what it kept. What's sent holds only
// the allowlisted keys, with numbers or fixed words (Fugo).
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/diagnostics_record.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'reconnect_from_scratch_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, tunnelStatus;

final _endpoint = 'a' * 64;
final _code = 'factory://?t=${'c' * 64}&e=$_endpoint&name=mac-mini';

/// The keys a connection event may carry, and the shape of a detail:
/// `key=value` tokens, each value a number or a lowercase word.
const _allowedKeys = {
  'app',
  'away_s',
  'net',
  'route',
  'why',
  'reason',
  'code',
  'up',
  'attempt',
  'welcomed',
  'err',
};
final _token = RegExp(r'^([a-z_]+)=([0-9]+|[a-z]+)$');

/// A Claktory at the other end of the tunnel: it welcomes each Hello, says
/// whether diagnostics are on ([share]), and keeps what's sent while they
/// are, refusing it otherwise.
class _Far {
  late final HttpServer server;
  var share = true;
  final received = <DiagnosticEvent>[];
  var requests = 0;
  final _sockets = <WebSocket>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      _sockets.add(socket);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket
            ..add(
              ServerMessage(
                replyTo: message.id,
                welcome: Welcome(serverVersion: 'test'),
              ).writeToBuffer(),
            )
            ..add(_workspace().writeToBuffer());
        } else if (message.hasSendDiagnostics()) {
          requests++;
          if (!share) {
            socket.add(
              ServerMessage(
                replyTo: message.id,
                error: Error(message: 'Diagnostics are off on this Claktory.'),
              ).writeToBuffer(),
            );
            return;
          }
          received.addAll(message.sendDiagnostics.events);
          socket.add(
            ServerMessage(replyTo: message.id, ack: Ack()).writeToBuffer(),
          );
        }
      });
    });
  }

  ServerMessage _workspace() => ServerMessage(
    workspace: Workspace(diagnostics: Diagnostics(share: share)),
  );

  /// Turns them on or off, as Settings would: every device is told.
  void turn(bool on) {
    share = on;
    for (final socket in _sockets) {
      if (socket.readyState == WebSocket.open) {
        socket.add(_workspace().writeToBuffer());
      }
    }
  }

  Future<void> close() => server.close(force: true);
}

class _Tunnels implements Tunnels {
  _Tunnels(this.port);

  final int port;

  /// The last one opened.
  FakeTunnel? last;

  /// What the next one opened says first.
  TunnelStatus? first;

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async => last = FakeTunnel(port)..status = first ?? tunnelStatus();

  @override
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  }) async => (device: 'b' * 64, backend: _endpoint);
}

void main() {
  late _Far far;
  late Directory folder;
  setUp(() async {
    far = _Far();
    await far.start();
    folder = Directory.systemTemp.createTempSync('diagnostics_sending');
  });
  tearDown(() async {
    await far.close();
    folder.deleteSync(recursive: true);
  });

  late _Tunnels tunnels;
  Backend backend(DiagnosticsRecorder recorder) =>
      Backend(
          workspace: WorkspaceStore(),
          transcript: TranscriptStore(),
          explorer: ExplorerStore(),
          extensionView: ExtensionViewStore(),
          tunnels: tunnels = _Tunnels(far.server.port),
          diagnostics: recorder,
          localPairing: () async => null,
        )
        ..deviceKey = ((_) => 'k' * 64)
        ..diagnosticsEvery = const Duration(milliseconds: 100);

  void expectAllowed(Iterable<DiagnosticEvent> events) {
    for (final event in events) {
      expect(event.area, 'connection');
      expect(
        ConnectionKind.values.map((kind) => kind.name),
        contains(event.kind),
      );
      if (event.detail.isEmpty) continue;
      for (final token in event.detail.split(' ')) {
        final match = _token.firstMatch(token);
        expect(match, isNotNull, reason: '"$token" in "${event.detail}"');
        expect(_allowedKeys, contains(match!.group(1)));
      }
    }
  }

  test('turned on, the device records its connection, sends it once '
      'connected, and keeps nothing the Claktory has', () async {
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => app.connected.value);
    // Told it's on with the first snapshot: from now on.
    await until(() => recorder.on);
    expect(File('${folder.path}/$_endpoint.on').existsSync(), isTrue);

    // Away a while and back: the reconnect is recorded, then sent.
    app.hidden();
    app.resumed(now: DateTime.now().add(const Duration(minutes: 2)));
    await until(
      () =>
          far.received.any((event) => event.kind == 'connected') &&
          recorder.waitingCount == 0,
    );
    final kinds = far.received.map((event) => event.kind).toList();
    expect(kinds, containsAllInOrder(['lifecycle', 'reconnect', 'connect']));
    expect(
      far.received.where((e) => e.kind == 'lifecycle').map((e) => e.detail),
      containsAllInOrder([
        'app=background',
        matches(RegExp(r'^app=back away_s=\d+$')),
      ]),
    );
    expect(
      far.received.firstWhere((e) => e.kind == 'reconnect').detail,
      'reason=resume',
    );
    expect(
      far.received.lastWhere((e) => e.kind == 'connected').detail,
      matches(RegExp(r'^attempt=\d+ route=(direct|relayed|tunnel)$')),
    );
    expectAllowed(far.received);
    // Each sent once, in order.
    final seqs = far.received.map((event) => event.seq.toInt()).toList();
    expect(seqs, [...seqs]..sort());
    expect(seqs.toSet().length, seqs.length);
    await app.close();
  });

  test("the relay's trouble while it's down, and how long it took to come "
      'up, are recorded in words and numbers only (#388)', () async {
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => far.received.any((event) => event.kind == 'connected'));
    final tunnel = tunnels.last!;
    tunnel.status = tunnelStatus(relay: false, trouble: TunnelRelayTrouble.dns);
    await until(
      () => far.received.any((event) => event.detail == 'up=0 err=dns'),
    );
    // Still down, now timing out: said again.
    tunnel.status = tunnelStatus(
      relay: false,
      trouble: TunnelRelayTrouble.timeout,
    );
    await until(
      () => far.received.any((event) => event.detail == 'up=0 err=timeout'),
    );
    tunnel.status = tunnelStatus(relayUpMs: 2300);
    // Up again, with how long it took.
    await until(
      () => far.received.any(
        (event) =>
            event.kind == 'relay' &&
            event.detail == 'up=1' &&
            event.durationMs.toInt() == 2300,
      ),
    );
    expectAllowed(far.received);
    await app.close();
  });

  test('a launch whose relay comes up a little after says only how long '
      'it took: no bare up=0 before it (Zucchero, #388)', () async {
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    tunnels.first = tunnelStatus(relay: false);
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => far.received.any((event) => event.kind == 'connected'));
    tunnels.last!.status = tunnelStatus(relayUpMs: 640);
    await until(() => far.received.any((event) => event.kind == 'relay'));
    // A tick more, for anything noted after.
    await Future<void>.delayed(const Duration(milliseconds: 1500));
    final relay = [
      for (final event in far.received)
        if (event.kind == 'relay') event,
    ];
    expect(relay.map((event) => event.detail), ['up=1']);
    expect(relay.single.durationMs.toInt(), 640);
    await app.close();
  });

  test('remembered on the device: what happens before it connects is '
      'recorded too, and sent once it does', () async {
    // On from an earlier run.
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    app.connectTo(parsePairingCode(_code));
    expect(recorder.on, isTrue);
    unawaited(app.run());
    await until(() => far.received.any((event) => event.kind == 'connected'));
    // The tunnel's relay and path as it opens, between them.
    expect(
      far.received.map((event) => event.kind),
      containsAllInOrder(['lifecycle', 'connect', 'connected']),
    );
    expect(far.received.first.detail, 'app=started');
    expect(far.received[1].detail, 'attempt=1 route=tunnel');
    expectAllowed(far.received);
    await app.close();
  });

  test('turned off on the Claktory, the device stops recording and deletes '
      'what it kept', () async {
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder)
      ..diagnosticsEvery = const Duration(minutes: 5);
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => app.connected.value);
    await until(() => recorder.waitingCount == 0);

    far.turn(false);
    await until(() => !recorder.on);
    await recorder.settled;
    expect(File('${folder.path}/$_endpoint.on').existsSync(), isFalse);
    expect(File('${folder.path}/$_endpoint.jsonl').existsSync(), isFalse);
    final requests = far.requests;
    app.hidden();
    app.resumed(now: DateTime.now().add(const Duration(minutes: 2)));
    await until(() => app.connected.value);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(recorder.waitingCount, 0);
    expect(far.requests, requests);
    await app.close();
  });

  test('each Claktory has its own: on for one, not for another', () {
    // Never turned on, nothing is written.
    DiagnosticsRecorder(folder)
      ..select('zero')
      ..add(ConnectionKind.connect, {ConnectionKey.attempt: 1});
    expect(folder.listSync(), isEmpty);
    final recorder = DiagnosticsRecorder(folder)
      ..select('one')
      ..share(true);
    recorder.add(ConnectionKind.connect, {ConnectionKey.attempt: 1});
    recorder.select('two');
    expect(recorder.on, isFalse);
    recorder.add(ConnectionKind.connect, {ConnectionKey.attempt: 2});
    expect(recorder.waitingCount, 0);
    recorder.select('one');
    expect(recorder.on, isTrue);
    expect(recorder.waitingCount, 1);
  });
}
