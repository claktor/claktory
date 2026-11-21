import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// The device's network as the system sees it (NWPathMonitor, in the
/// runners' NetworkPath.swift): whether it can reach anything, and through
/// which interfaces, the one it uses first.
class NetworkPath {
  const NetworkPath({
    required this.satisfied,
    this.interfaces = const [],
    this.expensive = false,
    this.constrained = false,
  });

  /// From the channel's event: `satisfied`, `interfaces` ('wifi',
  /// 'cellular', 'wired', 'loopback' or 'other', in the order the system
  /// prefers them), `expensive`, `constrained`.
  factory NetworkPath.fromEvent(Object? event) {
    final map = event is Map ? event : const {};
    return NetworkPath(
      satisfied: map['satisfied'] == true,
      interfaces: [
        for (final each in (map['interfaces'] as List?) ?? const [])
          if (each is String) each,
      ],
      expensive: map['expensive'] == true,
      constrained: map['constrained'] == true,
    );
  }

  final bool satisfied;
  final List<String> interfaces;
  final bool expensive;
  final bool constrained;

  /// The interface the system uses first; empty with none.
  String get primary => interfaces.isEmpty ? '' : interfaces.first;

  /// Whether connections made on [other] may not work on this: it reaches
  /// the network now where it didn't, or through another interface first
  /// (Wi-Fi to cellular, say). An interface added or dropped behind the
  /// one used, or a flag changing, leaves connections as they were.
  bool movedFrom(NetworkPath other) =>
      satisfied && (!other.satisfied || primary != other.primary);

  @override
  String toString() =>
      '${satisfied ? 'up' : 'down'} ${interfaces.join(',')}'
      '${expensive ? ' expensive' : ''}${constrained ? ' constrained' : ''}';
}

/// The moments [paths] moved to a network where open connections may be
/// dead ([NetworkPath.movedFrom]), each told once its burst of updates has
/// settled for [settle]. The first path is where it starts, not a move.
Stream<NetworkPath> networkMoves(
  Stream<NetworkPath> paths, {
  Duration settle = const Duration(milliseconds: 300),
}) {
  late final StreamController<NetworkPath> moves;
  StreamSubscription<NetworkPath>? listening;
  NetworkPath? settled;
  NetworkPath? latest;
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
            settled = now;
            if (before != null && now.movedFrom(before)) moves.add(now);
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
    },
  );
  return moves.stream;
}

/// This device's network moves, for every connection at once; settable for
/// tests. None under `flutter test`, which has no runner to ask.
Stream<NetworkPath> deviceNetworkMoves = Platform.environment.containsKey(
      'FLUTTER_TEST',
    )
    ? const Stream<NetworkPath>.empty()
    : networkMoves(
        const EventChannel(
          'factory/network',
        ).receiveBroadcastStream().map(NetworkPath.fromEvent),
      );
