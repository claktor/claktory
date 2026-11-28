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
import 'package:factory_app/network.dart';
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
  'reach',
  'bytes',
  'rebuild_ms',
  'dial_ms',
  'relay_ms',
};
final _token = RegExp(r'^([a-z_]+)=([0-9]+|[a-z]+)$');

/// A Claktory at the other end of the tunnel: it welcomes each Hello, says
/// whether diagnostics are on ([share]), and keeps what's sent while they
/// are, refusing it otherwise.
class _Far {
  late final HttpServer server;
  var share = true;

  /// An agent shown: its conversation follows the Workspace, after this.
  Duration? conversationAfter;
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
          if (conversationAfter case final after?) {
            Timer(after, () {
              socket.add(
                ServerMessage(transcript: Transcript(agentId: 'a1'))
                    .writeToBuffer(),
              );
            });
          }
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
    workspace: Workspace(
      diagnostics: Diagnostics(share: share),
      selectedAgentId: conversationAfter == null ? null : 'a1',
      agents: [if (conversationAfter != null) Agent(id: 'a1', name: 'Ana')],
    ),
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

  /// Called as each opens.
  void Function()? opening;

  /// How many were opened.
  var opened = 0;

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    opening?.call();
    opened++;
    return last = FakeTunnel(port)..status = first ?? tunnelStatus();
  }

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

  for (final system in ['ios', 'android', 'macos']) {
    test('$system: turned on, the device records its connection, sends it once '
        'connected, and keeps nothing the Claktory has', () async {
      final deviceFolder = diagnosticsFolderFor(
        operatingSystem: system,
        temporary: Directory(
          '${folder.path}/${system == 'ios' ? 'tmp' : 'code_cache'}',
        ),
        home: folder.path,
      );
      expect(deviceFolder, isNotNull);
      final recorder = DiagnosticsRecorder(deviceFolder!);
      final app = backend(recorder);
      app.connectTo(parsePairingCode(_code));
      unawaited(app.run());
      await until(() => app.connected.value);
      // Told it's on with the first snapshot: from now on.
      await until(() => recorder.on);
      expect(File('${deviceFolder.path}/$_endpoint.on').existsSync(), isTrue);

      // Away a while and back: the reconnect is recorded, then sent, with
      // where its connect went: the tunnel's rebuild, its dial, its relay.
      tunnels.last!.status = tunnelStatus(dialMs: 640, relayUpMs: 120);
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
        matches(
          RegExp(
            r'^attempt=\d+ route=direct( rebuild_ms=\d+)? dial_ms=640 '
            r'relay_ms=120$',
          ),
        ),
      );
      expect(
        kinds,
        containsAll(['lifecycle', 'connect', 'connected', 'disconnected']),
      );
      expectAllowed(far.received);
      // Each sent once, in order.
      final seqs = far.received.map((event) => event.seq.toInt()).toList();
      expect(seqs, [...seqs]..sort());
      expect(seqs.toSet().length, seqs.length);
      await app.close();
    });
  }

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

  test('a connection says which network it began on, what its direct path '
      'crosses, and when it was ready, with what it took (Polpo §4)', () async {
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    deviceNetwork.value = const NetworkPath(
      satisfied: true,
      interfaces: ['cellular'],
    );
    addTearDown(() => deviceNetwork.value = null);
    far.conversationAfter = const Duration(milliseconds: 400);
    tunnels.first = tunnelStatus(
      reach: TunnelReach.tailnet,
      linkReceived: 4321,
    );
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => far.received.any((event) => event.kind == 'ready'));
    final connect = far.received.firstWhere((event) => event.kind == 'connect');
    expect(
      connect.detail,
      matches(RegExp(r'^attempt=\d+ route=tunnel net=cellular$')),
    );
    final path = far.received.firstWhere((event) => event.kind == 'path');
    expect(path.detail, 'route=direct reach=tailnet');
    // Once the conversation shown came, not with the Workspace before it.
    final ready = far.received.firstWhere((event) => event.kind == 'ready');
    expect(ready.detail, matches(RegExp(r'^attempt=\d+ bytes=4321$')));
    expect(ready.durationMs.toInt(), greaterThanOrEqualTo(400));
    expect(far.received.where((event) => event.kind == 'ready'), hasLength(1));
    // A direct path that moves to the internet says so.
    tunnels.last!.status = tunnelStatus(reach: TunnelReach.public);
    await until(
      () => far.received.any(
        (event) => event.detail == 'route=direct reach=public',
      ),
    );
    expectAllowed(far.received);
    await app.close();
  });

  test('the first try after a launch, before the system has told the '
      'network, says it once connected (Sale)', () async {
    DiagnosticsRecorder(folder)
      ..select(_endpoint)
      ..share(true);
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder);
    deviceNetwork.value = null;
    addTearDown(() => deviceNetwork.value = null);
    tunnels.opening = () => deviceNetwork.value = const NetworkPath(
      satisfied: true,
      interfaces: ['wifi'],
    );
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => far.received.any((event) => event.kind == 'connected'));
    expect(
      far.received.firstWhere((event) => event.kind == 'connect').detail,
      matches(RegExp(r'^attempt=\d+ route=tunnel$')),
    );
    expect(
      far.received.firstWhere((event) => event.kind == 'connected').detail,
      matches(RegExp(r'^attempt=\d+ route=direct net=wifi$')),
    );
    await app.close();
  });

  test(
    'when the system tells the network only after the first try '
    'connected (Android, Sale), it is said once, as the network at launch',
    () async {
      DiagnosticsRecorder(folder)
        ..select(_endpoint)
        ..share(true);
      final recorder = DiagnosticsRecorder(folder);
      final app = backend(recorder);
      deviceNetwork.value = null;
      addTearDown(() => deviceNetwork.value = null);
      app.connectTo(parsePairingCode(_code));
      unawaited(app.run());
      await until(() => far.received.any((event) => event.kind == 'ready'));
      deviceNetwork.value = const NetworkPath(
        satisfied: true,
        interfaces: ['wifi'],
      );
      await until(() => far.received.any((event) => event.kind == 'network'));
      expect(
        far.received.where((event) => event.kind == 'network').single.detail,
        'net=wifi app=started',
      );
      for (final kind in ['connect', 'connected']) {
        expect(
          far.received.firstWhere((event) => event.kind == kind).detail,
          isNot(contains('net=')),
        );
      }
      // The same network told again (a flag changing): nothing more.
      deviceNetwork.value = const NetworkPath(
        satisfied: true,
        interfaces: ['wifi'],
        expensive: true,
      );
      await Future<void>.delayed(const Duration(milliseconds: 300));
      expect(
        far.received.where((event) => event.kind == 'network'),
        hasLength(1),
      );
      expectAllowed(far.received);
      await app.close();
    },
  );

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

  for (final system in ['ios', 'android', 'macos']) {
    test(
      '$system: remembered on the device: what happens before it connects is '
      'recorded too, and sent once it does',
      () async {
        final deviceFolder = diagnosticsFolderFor(
          operatingSystem: system,
          temporary: Directory(
            '${folder.path}/${system == 'ios' ? 'tmp' : 'code_cache'}',
          ),
          home: folder.path,
        );
        expect(deviceFolder, isNotNull);
        // On from an earlier run.
        DiagnosticsRecorder(deviceFolder!)
          ..select(_endpoint)
          ..share(true);
        final recorder = DiagnosticsRecorder(deviceFolder);
        final app = backend(recorder);
        app.connectTo(parsePairingCode(_code));
        expect(recorder.on, isTrue);
        unawaited(app.run());
        await until(
          () => far.received.any((event) => event.kind == 'connected'),
        );
        // The tunnel's relay and path as it opens, between them.
        expect(
          far.received.map((event) => event.kind),
          containsAllInOrder(['lifecycle', 'connect', 'connected']),
        );
        expect(far.received.first.detail, 'app=started');
        expect(far.received[1].detail, 'attempt=1 route=tunnel');
        expectAllowed(far.received);
        await app.close();
      },
    );
  }

  // Friend bug #53: a Mac connected for hours had nothing to send when
  // Advanced diagnostics were turned on, so it seemed to start only after
  // a relaunch. Turned on while connected, it says so at once; off and on
  // again, the same.
  test('turned on while connected, the device sends how its connection '
      'goes at once, without reconnecting', () async {
    far.share = false;
    final recorder = DiagnosticsRecorder(folder);
    final app = backend(recorder)
      ..diagnosticsEvery = const Duration(minutes: 5);
    app.connectTo(parsePairingCode(_code));
    unawaited(app.run());
    await until(() => app.connected.value);
    expect(recorder.on, isFalse);
    final connections = tunnels.opened;

    far.turn(true);
    await until(() => far.received.any((e) => e.kind == 'recording'));
    final said = far.received.lastWhere((e) => e.kind == 'recording');
    expect(said.area, 'connection');
    expect(said.detail, contains('route='));
    expectAllowed(far.received);
    expect(tunnels.opened, connections, reason: 'never reconnected');

    far.turn(false);
    await until(() => !recorder.on);
    far.received.clear();
    far.turn(true);
    await until(() => far.received.any((e) => e.kind == 'recording'));
    expect(tunnels.opened, connections);
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
