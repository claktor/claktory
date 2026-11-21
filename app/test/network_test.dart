import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connection.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/network.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, home, signedIn;

import 'package:factory_app/tunnel/api/tunnel.dart';

import 'tunnel_test.dart'
    show FakeTunnel, FakeTunnels, backendWith, tunnelStatus;

const _wifi = NetworkPath(satisfied: true, interfaces: ['wifi', 'cellular']);
const _cellular = NetworkPath(satisfied: true, interfaces: ['cellular']);
const _none = NetworkPath(satisfied: false);

final _code = 'factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini';

/// Waits up to [seconds] for [done].
Future<void> until(bool Function() done, {int seconds = 5}) async {
  final end = DateTime.now().add(Duration(seconds: seconds));
  while (!done() && DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  expect(done(), isTrue);
}

/// A backend at the other end of the tunnel: it welcomes each Hello
/// unless [welcoming] says not, and counts them.
class _Far {
  late final HttpServer server;
  var hellos = 0;
  final assertions = <String>[];
  var welcoming = true;

  /// Sent after each welcome, when set.
  Workspace? workspace;

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos++;
        assertions.add(message.hello.assertion);
        if (!welcoming) return;
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
        if (workspace case final workspace?) {
          socket.add(ServerMessage(workspace: workspace).writeToBuffer());
        }
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

/// The account service, its assertion held until [answer] completes.
class _SlowAccountApi extends FakeAccountApi {
  var answer = Completer<void>();

  @override
  Future<({String assertion, Duration expiresIn})> assertion(
    String deviceKey,
  ) async {
    asked.add(deviceKey);
    await answer.future;
    if (failure case final Object failure) throw failure;
    return (
      assertion: 'assertion-for-$deviceKey',
      expiresIn: const Duration(hours: 1),
    );
  }
}

void main() {
  late StreamController<NetworkPath> moves;
  final kept = deviceNetworkMoves;
  setUp(() {
    moves = StreamController<NetworkPath>.broadcast();
    deviceNetworkMoves = moves.stream;
  });
  tearDown(() async {
    deviceNetworkMoves = kept;
    await moves.close();
  });

  group('network moves', () {
    test('read from the runner\'s events', () {
      final path = NetworkPath.fromEvent({
        'satisfied': true,
        'interfaces': ['wifi', 'cellular'],
        'expensive': false,
        'constrained': true,
      });
      expect(path.satisfied, isTrue);
      expect(path.primary, 'wifi');
      expect(path.constrained, isTrue);
      expect(NetworkPath.fromEvent(null).satisfied, isFalse);
    });

    test('are another first interface, or the network back; the first '
        'path, losing it, or one behind the first are not', () {
      expect(_cellular.movedFrom(_wifi), isTrue);
      expect(_wifi.movedFrom(_none), isTrue);
      expect(_none.movedFrom(_wifi), isFalse);
      expect(
        const NetworkPath(
          satisfied: true,
          interfaces: ['wifi'],
        ).movedFrom(_wifi),
        isFalse,
      );
      expect(
        const NetworkPath(
          satisfied: true,
          interfaces: ['wifi', 'cellular'],
          expensive: true,
        ).movedFrom(_wifi),
        isFalse,
      );
    });

    test('are told once a burst of updates settles', () async {
      final paths = StreamController<NetworkPath>();
      final told = <NetworkPath>[];
      final listening = networkMoves(
        paths.stream,
        settle: const Duration(milliseconds: 30),
      ).listen(told.add);
      paths.add(_wifi);
      await Future<void>.delayed(const Duration(milliseconds: 60));
      expect(told, isEmpty, reason: 'where it starts');
      // Wi-Fi drops, cellular comes up, Wi-Fi is back: all within the
      // settle, so nothing moved.
      paths
        ..add(_none)
        ..add(_cellular)
        ..add(_wifi);
      await Future<void>.delayed(const Duration(milliseconds: 60));
      expect(told, isEmpty);
      paths
        ..add(_none)
        ..add(_cellular);
      await Future<void>.delayed(const Duration(milliseconds: 60));
      expect(told, [_cellular]);
      await listening.cancel();
      await paths.close();
    });

    test('a runner without the channel is no moves', () async {
      final paths = StreamController<NetworkPath>();
      final told = <NetworkPath>[];
      final listening = networkMoves(paths.stream).listen(told.add);
      paths.addError(Exception('MissingPluginException'));
      await pumpEventQueue();
      expect(told, isEmpty);
      await listening.cancel();
      await paths.close();
    });
  });

  test('the network moving makes the connection to another machine afresh '
      'at once, quietly', () async {
    final far = _Far();
    await far.start();
    final tunnels = FakeTunnels(FakeTunnel(far.server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(far.hellos, 1);

    moves.add(_cellular);
    await until(() => tunnels.tunnel.reconnects == 1);
    expect(backend.reconnecting.value, isTrue);
    await until(() => far.hellos == 2 && backend.connected.value);
    expect(backend.reconnecting.value, isFalse);
    await backend.close();
    await far.close();
  });

  test(
    "this Mac's own Claktory isn't on the network: a move leaves it",
    () async {
      final far = _Far();
      await far.start();
      final backend = Backend(
        workspace: WorkspaceStore(),
        transcript: TranscriptStore(),
        explorer: ExplorerStore(),
        extensionView: ExtensionViewStore(),
        tunnels: FakeTunnels(FakeTunnel(0)),
        localPairing: () async => null,
        localEndpoint: 'ws://127.0.0.1:${far.server.port}',
      );
      unawaited(backend.run());
      await until(() => backend.connected.value);
      moves.add(_cellular);
      await Future<void>.delayed(const Duration(milliseconds: 200));
      expect(far.hellos, 1);
      expect(backend.reconnecting.value, isFalse);
      await backend.close();
      await far.close();
    },
  );

  test('reconnecting stays quiet until the first try since has had its '
      'chance, however long it takes', () async {
    final far = _Far();
    await far.start();
    final tunnels = FakeTunnels(FakeTunnel(far.server.port));
    final backend = backendWith(tunnels)
      ..deviceKey = ((_) => 'k' * 64)
      ..quietReconnect = const Duration(milliseconds: 200)
      ..connectWithin = const Duration(milliseconds: 1500);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.connected.value);

    // The other end takes the next Hello but never welcomes it.
    far.welcoming = false;
    backend.networkMoved();
    await Future<void>.delayed(const Duration(milliseconds: 700));
    expect(far.hellos, 2);
    expect(
      backend.reconnecting.value,
      isTrue,
      reason: 'past its time, but the try is still under way',
    );
    await until(() => !backend.reconnecting.value);
    expect(backend.connected.value, isFalse);
    await backend.close();
    await far.close();
  });

  test("while a move copies or after, a dropped connection is the move's: "
      "the reconnect stays quiet, and the move as last seen stays", () async {
    final far = _Far()
      ..workspace = Workspace(
        move: Move(
          phase: MovePhase.MOVE_PHASE_COPYING,
          targetMachine: 'cloud-box',
        ),
      );
    await far.start();
    final tunnels = FakeTunnels(FakeTunnel(far.server.port));
    final backend = backendWith(tunnels)
      ..deviceKey = ((_) => 'k' * 64)
      ..quietReconnect = const Duration(milliseconds: 200)
      ..connectWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.move.value != null);
    expect(backend.move.value!.targetMachine, 'cloud-box');

    // Frozen, the backend serves this device no more.
    far.welcoming = false;
    backend.networkMoved();
    await until(() => far.hellos >= 3);
    expect(backend.connected.value, isFalse);
    expect(backend.reconnecting.value, isTrue);
    expect(backend.move.value!.phase, MovePhase.MOVE_PHASE_COPYING);

    // Back, with the move gone (failed and dismissed, or another
    // Claktory's): no move.
    far
      ..workspace = Workspace()
      ..welcoming = true;
    await until(() => backend.connected.value && backend.move.value == null);
    expect(backend.reconnecting.value, isFalse);

    // Another Claktory picked: nothing of this one's move shows meanwhile.
    far.workspace = Workspace(move: Move(phase: MovePhase.MOVE_PHASE_DONE));
    backend.networkMoved();
    await until(() => backend.move.value != null);
    backend.connectTo(parsePairingCode(_code));
    expect(backend.move.value, isNull);
    await backend.close();
    await far.close();
  });

  test('a Claktory connected to is unknown, not empty, until its '
      'Workspace arrives', () async {
    final far = _Far();
    await far.start();
    final backend = backendWith(FakeTunnels(FakeTunnel(far.server.port)))
      ..deviceKey = ((_) => 'k' * 64);
    backend.workspace.apply(Workspace(agents: [Agent(id: 'a1')]));
    backend.connectTo(parsePairingCode(_code));
    expect(backend.workspace.received, isFalse);
    expect(backend.workspace.workspace.agents, isEmpty);
    unawaited(backend.run());
    await until(() => backend.connected.value);
    // Welcomed, and still nothing known: the far end sends no Workspace.
    expect(backend.workspace.received, isFalse);
    await backend.close();
    await far.close();
  });

  test("after a wake or a resume, a failure from before isn't shown: it's "
      'reconnecting until a try made since fails', () async {
    // Nothing listens: every try fails at once.
    final closed = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = closed.port;
    await closed.close();
    final tunnel = FakeTunnel(port)
      ..status = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'no route',
      );
    final backend = backendWith(FakeTunnels(tunnel))
      ..deviceKey = ((_) => 'k' * 64)
      ..quietReconnect = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => Connection(backend).link == Link.failing);

    // The Mac wakes (or the app comes back): it doesn't know yet.
    final tries = tunnel.reconnects;
    backend.check();
    expect(Connection(backend).link, Link.connecting);
    expect(backend.reconnecting.value, isTrue);
    expect(tunnel.reconnects, tries + 1, reason: 'the tunnel redials');
    // Short of its quiet time it stays so, failures or not.
    await Future<void>.delayed(const Duration(milliseconds: 150));
    expect(Connection(backend).link, Link.connecting);
    // Then a try made since has failed: now it can say so.
    await until(() => Connection(backend).link == Link.failing);

    // The same for a phone back from a moment away.
    backend.hidden();
    backend.resumed();
    expect(Connection(backend).link, Link.connecting);
    await backend.close();
  });

  group('the device assertion', () {
    test('is asked for while the tunnel dials; slow, a Claktory found '
        'through the account is tried again, not given up on', () async {
      final far = _Far();
      await far.start();
      final api = _SlowAccountApi();
      final backend = backendWith(FakeTunnels(FakeTunnel(far.server.port)))
        ..deviceKey = ((_) => 'k' * 64)
        ..assertionWithin = const Duration(milliseconds: 200)
        ..account = signedIn(api);
      backend.connectTo(RemoteFactory.fromAccount(home));
      unawaited(backend.run());
      await until(() => api.asked.isNotEmpty);
      await Future<void>.delayed(const Duration(milliseconds: 600));
      expect(far.hellos, 0, reason: 'no Hello without it');
      expect(backend.signInRequired.value, isNull);
      expect(api.asked, hasLength(1), reason: 'one request, kept on its way');

      api.answer.complete();
      await until(() => backend.connected.value);
      expect(far.assertions.last, 'assertion-for-${'b' * 64}');
      expect(api.asked, hasLength(1));
      await backend.close();
      await far.close();
    });

    test("claktory.com out of reach isn't a reason to sign in: it tries "
        'again', () async {
      final far = _Far();
      await far.start();
      final api = FakeAccountApi()
        ..failure = const AccountServiceUnreachable('offline');
      final backend = backendWith(FakeTunnels(FakeTunnel(far.server.port)))
        ..deviceKey = ((_) => 'k' * 64)
        ..account = signedIn(api);
      backend.connectTo(RemoteFactory.fromAccount(home));
      unawaited(backend.run());
      await until(() => api.asked.length >= 2);
      expect(backend.signInRequired.value, isNull);
      expect(far.hellos, 0);

      api.failure = null;
      await until(() => backend.connected.value);
      expect(backend.signInRequired.value, isNull);
      await backend.close();
      await far.close();
    });

    test('refused by the service, it says to sign in', () async {
      final far = _Far();
      await far.start();
      final api = FakeAccountApi()
        ..failure = const AccountServiceError(403, 'forbidden', 'No.');
      final backend = backendWith(FakeTunnels(FakeTunnel(far.server.port)))
        ..deviceKey = ((_) => 'k' * 64)
        ..account = signedIn(api);
      backend.connectTo(RemoteFactory.fromAccount(home));
      unawaited(backend.run());
      await until(() => backend.signInRequired.value != null);
      expect(far.hellos, 0);
      await backend.close();
      await far.close();
    });

    test('a paired device goes without one that comes too late', () async {
      final far = _Far();
      await far.start();
      final api = _SlowAccountApi();
      final backend = backendWith(FakeTunnels(FakeTunnel(far.server.port)))
        ..deviceKey = ((_) => 'k' * 64)
        ..assertionWithin = const Duration(milliseconds: 100)
        ..account = signedIn(api);
      backend.connectTo(parsePairingCode(_code));
      unawaited(backend.run());
      await until(() => backend.connected.value);
      expect(far.assertions.single, isEmpty);
      api.answer.complete();
      await backend.close();
      await far.close();
    });
  });
}
