// A Claktory that can't be reached and isn't on screen backs off instead of
// being redialed every few seconds (#454): each try at one that isn't there
// costs a burst of packets. Opening it, or the network moving, tries again
// at once. And an app in the background can close its connection until it's
// back (suspend), keeping it for work the person waits on (holding).

import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/network.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

import 'reconnect_from_scratch_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

/// The Claktory at the other end of the tunnel: it turns every try away
/// while [away], noting when, and welcomes each Hello otherwise.
class _Far {
  late final HttpServer server;
  var away = true;
  final tries = <DateTime>[];
  final sockets = <WebSocket>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      tries.add(DateTime.now());
      if (away) {
        request.response.statusCode = HttpStatus.serviceUnavailable;
        await request.response.close();
        return;
      }
      final socket = await WebSocketTransformer.upgrade(request);
      sockets.add(socket);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
      });
    });
  }

  /// The gaps between its last [count] tries.
  List<Duration> lastGaps(int count) {
    final last = tries.sublist(tries.length - count - 1);
    return [
      for (var i = 1; i < last.length; i++) last[i].difference(last[i - 1]),
    ];
  }
}

const _quick = Duration(milliseconds: 40);
const _most = Duration(milliseconds: 600);

Backend _backend(FakeTunnel tunnel) {
  final backend =
      Backend(
          workspace: WorkspaceStore(),
          transcript: TranscriptStore(),
          explorer: ExplorerStore(),
          extensionView: ExtensionViewStore(),
          tunnels: FakeTunnels(tunnel),
          localPairing: () async => null,
        )
        ..deviceKey = ((_) => 'k' * 64)
        ..retryMost = _quick
        ..awayRetryMost = _most;
  backend.connectTo(parsePairingCode(_code));
  return backend;
}

Future<void> _wait(int ms) => Future<void>.delayed(Duration(milliseconds: ms));

void main() {
  late _Far far;
  late FakeTunnel tunnel;
  setUp(() async {
    far = _Far();
    await far.start();
    tunnel = FakeTunnel(far.server.port);
  });
  tearDown(() => far.server.close(force: true));

  test('on screen, it keeps trying every few seconds', () async {
    final backend = _backend(tunnel);
    unawaited(backend.run());
    await _wait(1500);
    expect(far.tries.length, greaterThan(10));
    for (final gap in far.lastGaps(5)) {
      expect(gap, lessThan(const Duration(milliseconds: 300)));
    }
    await backend.close();
  });

  test('off screen, its quick tries done, it waits twice as long each '
      'time, up to its most', () async {
    final backend = _backend(tunnel)..onScreen = false;
    unawaited(backend.run());
    await _wait(3500);
    final gaps = far.lastGaps(3);
    for (final gap in gaps) {
      expect(gap, greaterThanOrEqualTo(_most * 0.9));
      expect(gap, lessThan(_most * 1.6));
    }
    expect(far.tries.length, lessThan(14));
    await backend.close();
  });

  test('opened, it tries again at once, and quickly from then on', () async {
    final backend = _backend(tunnel)..onScreen = false;
    unawaited(backend.run());
    await _wait(2500);
    final before = far.tries.length;
    final opened = DateTime.now();
    backend.onScreen = true;
    await until(() => far.tries.length > before, seconds: 1);
    expect(
      far.tries.last.difference(opened),
      lessThan(const Duration(milliseconds: 200)),
    );
    await _wait(500);
    expect(far.tries.length, greaterThan(before + 3));
    await backend.close();
  });

  test('the network moving tries at once; then it keeps to its '
      'backoff', () async {
    final backend = _backend(tunnel)..onScreen = false;
    unawaited(backend.run());
    await _wait(2500);
    final before = far.tries.length;
    final moved = DateTime.now();
    backend.networkMoved();
    await until(() => far.tries.length > before, seconds: 1);
    expect(
      far.tries.last.difference(moved),
      lessThan(const Duration(milliseconds: 200)),
    );
    await _wait(250);
    expect(far.tries.length, before + 1, reason: 'one try, then the backoff');
    await backend.close();
  });

  test('back in front before its next try is due, it waits for it; a check '
      'after sleep does too', () async {
    final backend = _backend(tunnel)..onScreen = false;
    unawaited(backend.run());
    await _wait(2500);
    await until(() => far.tries.length > 3, seconds: 1);
    final before = far.tries.length;
    final last = far.tries.last;
    backend.hidden();
    backend.resumed();
    backend.check();
    await _wait(100);
    if (DateTime.now().difference(last) < _most * 0.8) {
      expect(far.tries.length, before, reason: 'not due yet');
    }
    await backend.close();
  });

  test('a reachable one off screen connects as before, and once it drops it '
      'tries quickly before backing off', () async {
    far.away = false;
    final backend = _backend(tunnel)..onScreen = false;
    unawaited(backend.run());
    await until(() => backend.connected.value);
    final before = far.tries.length;
    far.away = true;
    for (final socket in far.sockets) {
      await socket.close();
    }
    await until(() => !backend.connected.value);
    await _wait(800);
    expect(far.tries.length, greaterThan(before + 2), reason: 'quick tries');
    far.away = false;
    await until(() => backend.connected.value, seconds: 3);
    await backend.close();
  });

  test('suspended in the background, it closes the connection and the '
      'tunnel, tries nothing, and connects afresh once back', () async {
    far.away = false;
    final backend = _backend(tunnel);
    unawaited(backend.run());
    await until(() => backend.connected.value);
    backend.suspend();
    expect(tunnel.closed, isFalse, reason: 'only while hidden');
    backend.hidden();
    backend.suspend();
    await until(() => !backend.connected.value);
    await until(() => tunnel.closed);
    final before = far.tries.length;
    backend.check();
    backend.networkMoved();
    await _wait(400);
    expect(far.tries.length, before, reason: 'nothing while suspended');

    tunnel.closed = false;
    backend.resumed();
    await until(() => backend.connected.value, seconds: 3);
    await backend.close();
  });

  group('kept trying in the background (W6)', () {
    tearDown(() => deviceNetwork.value = null);

    test('told to, one keeps trying while the app is in the background; '
        'the others wait until it is back', () async {
      final other = _Far();
      await other.start();
      addTearDown(() => other.server.close(force: true));
      final kept = _backend(tunnel);
      final waiting = _backend(FakeTunnel(other.server.port));
      unawaited(kept.run());
      unawaited(waiting.run());
      await _wait(300);
      kept
        ..hidden()
        ..keepTryingWhileHidden = true;
      waiting.hidden();
      await _wait(300);
      final keptBefore = far.tries.length;
      final waitingBefore = other.tries.length;
      await _wait(600);
      expect(far.tries.length, greaterThan(keptBefore + 3));
      expect(other.tries.length, waitingBefore, reason: 'parked');

      // Told to stop: it waits for the app too.
      kept.keepTryingWhileHidden = false;
      await _wait(300);
      final stopped = far.tries.length;
      await _wait(400);
      expect(far.tries.length, stopped);
      await kept.close();
      await waiting.close();
    });

    test('suspended, told to keep trying, its connection opens again; '
        'nothing says the app came back', () async {
      far.away = false;
      final backend = _backend(tunnel);
      unawaited(backend.run());
      await until(() => backend.connected.value);
      backend
        ..hidden()
        ..suspend();
      await until(() => !backend.connected.value);
      backend.keepTryingWhileHidden = true;
      await until(() => backend.connected.value, seconds: 3);
      // Still in the background: a check (as after sleep) is its own.
      expect(backend.lastFailure.value, isNull);
      await backend.close();
    });

    test('its network let through again, only the one told to keep '
        'trying tries at once', () async {
      final other = _Far();
      await other.start();
      addTearDown(() => other.server.close(force: true));
      deviceNetwork.value = const NetworkPath(satisfied: true);
      final kept = _backend(tunnel)
        ..onScreen = false
        ..awayRetryMost = const Duration(seconds: 30);
      final waiting = _backend(FakeTunnel(other.server.port))
        ..onScreen = false
        ..awayRetryMost = const Duration(seconds: 30);
      unawaited(kept.run());
      unawaited(waiting.run());
      await _wait(2500);
      kept
        ..hidden()
        ..keepTryingWhileHidden = true;
      waiting.hidden();
      deviceNetwork.value = const NetworkPath(satisfied: true, blocked: true);
      await _wait(200);
      final keptBefore = far.tries.length;
      final waitingBefore = other.tries.length;
      deviceNetwork.value = const NetworkPath(satisfied: true);
      await until(() => far.tries.length > keptBefore, seconds: 1);
      await _wait(200);
      expect(other.tries.length, waitingBefore);
      await kept.close();
      await waiting.close();
    });
  });

  group('back after a browser tab kept it connected (#505, Sale row 175)', () {
    /// Connected, then hidden for 40 seconds by the clock resumed reads,
    /// told to keep trying meanwhile when [kept] (a shared website's tab
    /// over the app), and whatever [away] does then.
    Future<(Backend, int)> trip({
      required bool kept,
      Future<void> Function(Backend backend)? away,
    }) async {
      far.away = false;
      final backend = _backend(tunnel);
      unawaited(backend.run());
      await until(() => backend.connected.value);
      backend.hidden();
      if (kept) {
        backend.keepTryingWhileHidden = true;
        await _wait(100);
        // The tab says it closed before the app says it's back.
        backend.keepTryingWhileHidden = false;
      }
      await away?.call(backend);
      final before = far.tries.length;
      backend.resumed(now: DateTime.now().add(const Duration(seconds: 40)));
      return (backend, before);
    }

    test('kept connected, it is only checked: no new connection', () async {
      final (backend, before) = await trip(kept: true);
      await _wait(500);
      expect(far.tries.length, before, reason: 'nothing dialled afresh');
      expect(backend.connected.value, isTrue);
      expect(far.sockets.where((s) => s.closeCode != null), isEmpty);
      await backend.close();
    });

    test('dropped while away, it connects afresh', () async {
      final (backend, before) = await trip(
        kept: true,
        away: (backend) async {
          for (final socket in far.sockets) {
            await socket.close();
          }
          await until(() => !backend.connected.value);
        },
      );
      await until(() => backend.connected.value, seconds: 3);
      expect(far.tries.length, greaterThan(before));
      await backend.close();
    });

    test('suspended after the tab closed, it connects afresh', () async {
      final (backend, before) = await trip(
        kept: true,
        away: (backend) async {
          backend.suspend();
          await until(() => !backend.connected.value);
        },
      );
      await until(() => backend.connected.value, seconds: 3);
      expect(far.tries.length, greaterThan(before));
      await backend.close();
    });

    test('hidden as long without a tab, it connects afresh, as '
        'before', () async {
      final (backend, before) = await trip(kept: false);
      await until(() => far.tries.length > before, seconds: 3);
      await until(() => backend.connected.value, seconds: 3);
      await backend.close();
    });
  });

  test('work held counts until it ends, however it ends', () async {
    final backend = _backend(tunnel);
    expect(backend.holds.value, isFalse);
    final done = Completer<int>();
    final held = backend.holding(() => done.future);
    expect(backend.holds.value, isTrue);
    final failing = backend.holding<void>(() async => throw StateError('no'));
    await expectLater(failing, throwsStateError);
    expect(backend.holds.value, isTrue, reason: 'the first still runs');
    done.complete(4);
    expect(await held, 4);
    expect(backend.holds.value, isFalse);
    await backend.close();
  });
}
