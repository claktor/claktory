import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// The device's network as the system sees it (NWPathMonitor, in the
/// runners' NetworkPath.swift): whether it can reach anything, and through
/// which interfaces, the one it uses first, and on Android which network.
class NetworkPath {
  const NetworkPath({
    required this.satisfied,
    this.interfaces = const [],
    this.expensive = false,
    this.constrained = false,
    this.network = '',
    this.blocked = false,
  });

  /// From the channel's event: `satisfied`, `interfaces` ('wifi',
  /// 'cellular', 'wired', 'loopback' or 'other', in the order the system
  /// prefers them), `expensive`, `constrained`, and `network`: Android's
  /// id for the default network (its handle), left out when there's none
  /// and on the iPhone and the Mac, whose NWPath has no such id.
  factory NetworkPath.fromEvent(Object? event) {
    final map = event is Map ? event : const {};
    final network = map['network'];
    return NetworkPath(
      satisfied: map['satisfied'] == true,
      interfaces: [
        for (final each in (map['interfaces'] as List?) ?? const [])
          if (each is String) each,
      ],
      expensive: map['expensive'] == true,
      constrained: map['constrained'] == true,
      network: network is String || network is int ? '$network' : '',
      blocked: map['blocked'] == true,
    );
  }

  final bool satisfied;
  final List<String> interfaces;
  final bool expensive;
  final bool constrained;

  /// Which network it is, where the system says (Android); empty where it
  /// doesn't. The same network keeps its connections whatever its
  /// interfaces say meanwhile.
  final String network;

  /// Whether the system blocks this app's network for now, where it says
  /// (Android 10 and later: an app in the background while the screen is
  /// off, say); false elsewhere. Unblocked, a Claktory told to keep trying
  /// in the background tries again at once (W6).
  final bool blocked;

  /// The interface the system uses first; empty with none.
  String get primary => interfaces.isEmpty ? '' : interfaces.first;

  /// Whether connections made on [other] may not work on this: it reaches
  /// the network now where it didn't, or through another interface first
  /// (Wi-Fi to cellular, say). An interface added or dropped behind the
  /// one used, or a flag changing, leaves connections as they were. Where
  /// both say which network they are, only another network is a move: on
  /// one network the interfaces listed can change (an emulator's Wi-Fi
  /// going off and on) while its connections hold.
  bool movedFrom(NetworkPath other) {
    if (!satisfied) return false;
    if (!other.satisfied) return true;
    if (network.isNotEmpty && other.network.isNotEmpty) {
      return network != other.network;
    }
    return primary != other.primary;
  }

  @override
  String toString() =>
      '${satisfied ? 'up' : 'down'} ${interfaces.join(',')}'
      '${expensive ? ' expensive' : ''}${constrained ? ' constrained' : ''}'
      '${network.isEmpty ? '' : ' #$network'}'
      '${blocked ? ' blocked' : ''}';
}

/// The moments [paths] moved to a network where open connections may be
/// dead ([NetworkPath.movedFrom]), each told once its burst of updates has
/// settled for [settle]. The first path is where it starts, not a move.
/// Where paths say which network they are (Android), a path back up is
/// weighed against the last one up: the same network back after a while
/// reported down (an app Doze kept off the network, say) is no move, since
/// a network truly lost comes back as another.
Stream<NetworkPath> networkMoves(
  Stream<NetworkPath> paths, {
  Duration settle = const Duration(milliseconds: 300),
}) {
  late final StreamController<NetworkPath> moves;
  StreamSubscription<NetworkPath>? listening;
  NetworkPath? settled;
  NetworkPath? latest;

  /// The last settled path that was up.
  NetworkPath? up;
  Timer? settling;
  moves = StreamController<NetworkPath>.broadcast(
    onListen: () {
      listening = paths.listen(
        (path) {
          latest = path;
          settling?.cancel();
          settling = Timer(settle, () {
            final now = latest!;
            final before = settled;
            final wasUp = up;
            settled = now;
            if (now.satisfied) up = now;
            if (before == null) return;
            // Up again on the network it was up on: not a move.
            final against =
                !before.satisfied &&
                    now.network.isNotEmpty &&
                    (wasUp?.network.isNotEmpty ?? false)
                ? wasUp!
                : before;
            if (now.movedFrom(against)) moves.add(now);
          });
        },
        // A runner without the channel (or a test): no moves to tell.
        onError: (Object _) {},
      );
    },
    onCancel: () {
      settling?.cancel();
      unawaited(listening?.cancel());
      listening = null;
      settled = null;
      up = null;
    },
  );
  return moves.stream;
}

/// The device's network as last told, while [deviceNetworkMoves] is
/// listened to; null before (and under `flutter test`). Settable for tests.
final deviceNetwork = ValueNotifier<NetworkPath?>(null);

/// This device's network moves, for every connection at once; settable for
/// tests. None under `flutter test`, which has no runner to ask.
Stream<NetworkPath> deviceNetworkMoves =
    Platform.environment.containsKey('FLUTTER_TEST')
    ? const Stream<NetworkPath>.empty()
    : networkMoves(
        const EventChannel('factory/network')
            .receiveBroadcastStream()
            .map(NetworkPath.fromEvent)
            .map((path) => deviceNetwork.value = path),
      );
