import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'device_account.dart' show AccountClaktory;
import 'devices.dart' show thisDeviceKind;
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show Touch;
import 'mobile/platform_policy.dart';
import 'setup.dart'
    show LocalEngine, SetupScope, backgroundItemNote, localEngine;
import 'tunnel/api/tunnel.dart';
import 'network.dart' show deviceNetwork;
import 'tunnel_client.dart'
    show idleTunnel, offlineNow, tunnelAdvice, unreachableAdvice;
import 'usage.dart' show formatAge;

/// How a Claktory's connection is doing, at a glance.
enum Link {
  /// Welcomed by the backend: everything shows as it is.
  live,

  /// Opening, or coming back after it dropped.
  connecting,

  /// Through the private tunnel, its last try failed: it says why.
  failing,

  /// Turned away: this device isn't allowed any more.
  refused,

  /// It moved to another computer: what answers here is the copy it left,
  /// which runs nothing.
  moved,

  /// This Mac's own, turned off by the person (friend bug #27).
  off,

  /// This Mac's own, kept from running by macOS (friend bug #28).
  blocked,
}

/// A Claktory's connection as the app shows it: its state and the words
/// for it, in the sidebar's footer, on the rail's tile and on a phone.
class Connection {
  Connection(this.backend, {String? machine, DateTime? now})
    : link = _linkOf(backend),
      now = now ?? DateTime.now(),
      machine =
          machine ?? (backend.target == null ? null : claktoryName(backend));

  final Backend backend;
  final Link link;

  /// When it's read: ages are from it.
  final DateTime now;

  /// The Claktory's name, when it's on another machine; null for this
  /// Mac's own.
  final String? machine;

  static Link _linkOf(Backend backend) {
    if (backend.refused.value != null) return Link.refused;
    if (backend.movedAway.value != null) return Link.moved;
    if (backend.connected.value) return Link.live;
    if (backend.target == null) {
      switch (localEngine.value) {
        case LocalEngine.off:
          return Link.off;
        case LocalEngine.blocked:
          return Link.blocked;
        case LocalEngine.running:
          break;
      }
      // Its socket turned this app away: it says why while it
      // keeps trying.
      if (backend.localRefused.value != null) return Link.failing;
    }
    final tunnel = backend.tunnel.value;
    // Back from the background or a network change, it reconnects without
    // a word for a few seconds first.
    if (backend.target != null &&
        !backend.reconnecting.value &&
        tunnel != null &&
        tunnel.failure != TunnelFailure.none) {
      return Link.failing;
    }
    // Once it has said it can't connect, it keeps saying so (and what it
    // knows) through the tries after, until one connects (row 475).
    if (backend.target != null && backend.lastFailure.value != null) {
      return Link.failing;
    }
    return Link.connecting;
  }

  /// What the connection's state depends on.
  static Listenable of(Backend backend) => Listenable.merge([
    backend.connected,
    backend.connectedSince,
    backend.reconnecting,
    backend.refused,
    backend.localRefused,
    backend.movedAway,
    backend.tunnel,
    backend.lastConnected,
    backend.checkedIn,
    backend.relayDownSince,
    backend.lastFailure,
    deviceNetwork,
    localEngine,
  ]);

  /// When it was last known to be running, while it can't connect: the
  /// later of when this app was last connected to it and when it last
  /// checked in with the person's account (friend bug #37). Null when
  /// neither is known.
  DateTime? get lastSeen {
    final connected = backend.lastConnected.value;
    final checkedIn = backend.checkedIn.value;
    if (connected == null) return checkedIn;
    if (checkedIn == null) return connected;
    return connected.isAfter(checkedIn) ? connected : checkedIn;
  }

  String _ago(DateTime time) => formatAge(time.millisecondsSinceEpoch, now);

  /// Why this device can't reach Claktory's servers (its relays), in a
  /// few words; empty when it doesn't know.
  static String _relayWhy(TunnelRelayTrouble trouble) => switch (trouble) {
    TunnelRelayTrouble.dns => ": their address couldn't be looked up",
    TunnelRelayTrouble.timeout => ': nothing answered in time',
    TunnelRelayTrouble.tls => ': the secure connection to them failed',
    TunnelRelayTrouble.network => ': the network turned them away',
    TunnelRelayTrouble.unchosen => ': none of them answered',
    TunnelRelayTrouble.other || TunnelRelayTrouble.none => '',
  };

  /// What's known while it can't connect (friend bug #37): why, as far
  /// as this device can tell, and when it was last seen running. Only
  /// what the tunnel and the account say, never a guessed cause.
  String _failingFacts(TunnelStatus tunnel) {
    final device = thisDeviceKind;
    final facts = <String>[];
    final down = backend.relayDownSince.value;
    if (offlineNow(tunnel)) {
      // The advice says this device is offline.
    } else if (!tunnel.relayConnected &&
        (down != null || tunnel.failure == TunnelFailure.offline)) {
      // Its relay out of reach while its own network is up: not offline.
      final why = _relayWhy(tunnel.relayTrouble);
      facts.add(
        down == null
            ? "This $device is online, but can't reach Claktory's servers$why."
            : "This $device is online, but hasn't reached Claktory's servers "
                  'since ${logTime(down, now: now)}$why.',
      );
    } else if (tunnel.relayConnected) {
      facts.add("This $device is online, but $_name doesn't answer.");
    }
    final connected = backend.lastConnected.value;
    final checkedIn = backend.checkedIn.value;
    if (checkedIn != null) {
      // Its own check-in: it was running then. An older one says no
      // more than when (Narancia's wsl-support.md §3: unknown stays
      // unknown).
      final ago = _ago(checkedIn);
      facts.add(
        now.difference(checkedIn) <= AccountClaktory.onlineWithin
            ? '$_name last checked in with your Claktory account $ago, so '
                  'it was running then.'
            : '$_name last checked in with your Claktory account $ago.',
      );
    }
    if (connected != null) {
      facts.add(
        'This $device was last connected to it at '
        '${logTime(connected, now: now)}.',
      );
    }
    return facts.join(' ');
  }

  String get _name => machine ?? 'this Claktory';

  /// Refused: "nixos no longer allows this Mac" once this device saw it
  /// since the app opened (it was removed there), else "doesn't allow",
  /// since it may never have let it in (a code that expired). The footer
  /// and the page say it alike (Abbacchio 49877).
  String notAllowed(String device) => backend.workspace.received
      ? '$_name no longer allows this $device'
      : "$_name doesn't allow this $device";

  /// The tunnel's status it says it can't connect for: the try's own once
  /// it failed; while a new try dials (its status afresh), the last
  /// failure (row 475, 480).
  TunnelStatus get _failed {
    final current = backend.tunnel.value;
    if (current != null && current.failure != TunnelFailure.none) {
      return current;
    }
    return backend.lastFailure.value ?? current ?? idleTunnel;
  }

  /// What went wrong exactly, for whoever looks into it, while it can't
  /// connect; empty otherwise.
  String get error => switch (link) {
    // This Mac's refusal is in the details already.
    Link.failing => _localRefusal != null ? '' : _cause(_failed),
    Link.refused => _cause(backend.tunnel.value ?? idleTunnel),
    _ => '',
  };

  /// [status]'s own words, but where the tunnel, which knows neither this
  /// device's kind nor the Claktory's name, says it reached nothing, in
  /// the sheet's words: "This iPhone can't reach Claktory's servers or
  /// Studio" (Trish, #37).
  String _cause(TunnelStatus status) => status.failure == TunnelFailure.offline
      ? "This $thisDeviceKind can't reach Claktory's servers or $_name: "
            'check its network.'
      : status.error;

  /// The footer's line, and the dialog's title.
  String get line => switch (link) {
    Link.live =>
      machine == null
          ? 'Running on this $_thisComputer'
          : 'Connected to $machine',
    Link.connecting when backend.reconnecting.value => 'Reconnecting…',
    Link.connecting =>
      machine == null ? 'Connecting…' : 'Connecting to $machine…',
    Link.failing => "Can't connect to $_name",
    Link.refused => notAllowed('device'),
    Link.moved => switch (backend.movedAway.value?.to ?? '') {
      '' => 'Moved to another computer',
      final to => 'Moved to $to',
    },
    Link.off => 'Turned off on this $_thisComputer',
    Link.blocked => 'Blocked by macOS',
  };

  /// A word or two, beside a name that's already shown (a phone's rows).
  String get short => switch (link) {
    Link.live => 'Connected',
    Link.connecting =>
      backend.reconnecting.value ? 'Reconnecting…' : 'Connecting…',
    Link.failing => switch (lastSeen) {
      null => "Can't connect",
      final seen => "Can't connect · last seen ${_ago(seen)}",
    },
    Link.refused => 'Not allowed',
    Link.moved => 'Moved',
    Link.off => 'Turned off',
    Link.blocked => 'Blocked by macOS',
  };

  /// Under "Can't connect" where nothing else shows (the main pane): when
  /// it was last seen, and that it keeps trying.
  String get stillTrying => switch ((_localRefusal, lastSeen)) {
    (final refusal?, _) => '$refusal Trying again…',
    (null, null) => 'Trying again…',
    (null, final seen?) => 'Last seen ${_ago(seen)}. Trying again…',
  };

  /// What this Mac's own Claktory said when it turned this app away on
  /// its socket, while it does; null otherwise.
  String? get _localRefusal =>
      backend.target == null ? backend.localRefused.value : null;

  /// How it connects, and since when.
  String get details {
    final since = backend.connectedSince.value;
    final at = since == null ? null : logTime(since);
    switch (link) {
      case Link.live:
        if (machine == null) {
          return 'Claktory is running on this $_thisComputer.'
              '${at == null ? '' : ' Connected since $at.'}';
        }
        final when = at == null ? '' : ' since $at';
        // The route iroh picked, which can change while connected; none
        // for a moment before it picks one.
        return switch (backend.tunnel.value?.path ?? TunnelPath.none) {
          TunnelPath.relayed =>
            'Connected to $machine$when, by an indirect route because a '
                "direct connection wasn't possible, so it may be a little "
                'slower. Private and encrypted.',
          TunnelPath.direct =>
            'Connected directly to $machine$when. Private and encrypted.',
          TunnelPath.none =>
            'Connected to $machine$when. Private and encrypted.',
        };
      case Link.connecting:
        return 'Trying to reach $_name…';
      case Link.off:
        return "Claktory is turned off on this $_thisComputer, so its agents "
            "aren't running. Start it, and they carry on where they left off.";
      case Link.blocked:
        return 'macOS is blocking Claktory from running in the background. '
            'Open System Settings and turn on Claktory under Allow in the '
            'Background. $backgroundItemNote';
      case Link.moved:
        final to = backend.movedAway.value?.to ?? '';
        return 'This Claktory moved to '
            '${to.isEmpty ? 'another computer' : to}. The copy it left here '
            'doesn’t run.';
      case Link.failing:
        if (_localRefusal case final refusal?) return refusal;
        final tunnel = _failed;
        return [
          _failingFacts(tunnel),
          // Its facts said already that it's online.
          tunnel.failure == TunnelFailure.offline && !offlineNow(tunnel)
              ? unreachableAdvice(_name)
              : tunnelAdvice(tunnel, _name, device: thisDeviceKind),
        ].where((part) => part.isNotEmpty).join(' ');
      case Link.refused:
        return tunnelAdvice(
          backend.tunnel.value ?? idleTunnel,
          _name,
          device: thisDeviceKind,
        );
    }
  }

  /// Red when it can't connect.
  bool get problem =>
      link == Link.failing || link == Link.refused || link == Link.blocked;
}

/// The connection's mark: quiet while it's live, pulsing signal while it
/// connects, red when it can't.
class ConnectionDot extends StatelessWidget {
  const ConnectionDot({super.key, required this.link, this.quiet = true});

  final Link link;

  /// Show a faint dot while live; off, nothing (a rail tile).
  final bool quiet;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return switch (link) {
      Link.live =>
        quiet ? Dot(color: palette.textFaint) : const SizedBox.shrink(),
      Link.connecting => const LiveDot(),
      Link.failing ||
      Link.refused ||
      Link.blocked => Dot(color: palette.danger),
      Link.moved || Link.off => Dot(color: palette.textFaint),
    };
  }
}

/// The sidebar footer's connection line: its mark and words, with how it
/// connects on hover (or a long press), and a click for [ConnectionDialog].
/// Not connected, the click reconnects at once too ([reconnectFirst]).
/// Turned away, the content's page says what to do (devices-signin.md §4).
class ConnectionLine extends StatelessWidget {
  const ConnectionLine({super.key, required this.backend, this.machine});

  final Backend backend;

  /// The machine's name, when the Claktory knows it.
  final String? machine;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Connection.of(backend),
      builder: (context, _) {
        final palette = Palette.of(context);
        final connection = Connection(backend, machine: machine);
        // Under a finger on Android (the sidebar's foot of a wide window),
        // a whole touch target, 48dp; a Mac and an iPad keep the line as it
        // is. A phone's drawer shows the connection as a word instead.
        final policy = PhonePlatformPolicy.of(context);
        final least = policy.touch && policy.touchTarget > Touch.target
            ? policy.touchTarget
            : 0.0;
        return HoverCard(
          holdToShow: true,
          card: (context) => Text(
            connection.details,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
          child: Semantics(
            container: true,
            button: true,
            label: connection.line,
            hint: reconnectFirst(connection.link) ? reconnectAndShowHint : null,
            child: Pressable(
              onTap: () {
                if (reconnectFirst(connection.link)) backend.reconnectNow();
                showModal(
                  context,
                  (context, close) => ConnectionDialog(
                    backend: backend,
                    machine: machine,
                    close: close,
                  ),
                );
              },
              builder: (context, hovered) => ConstrainedBox(
                constraints: BoxConstraints(minWidth: least, minHeight: least),
                child: ExcludeSemantics(
                  child: Row(
                    children: [
                      SizedBox(
                        width: 10,
                        child: Center(
                          child: ConnectionDot(link: connection.link),
                        ),
                      ),
                      const SizedBox(width: Space.s - 2),
                      Flexible(
                        child: Text(
                          connection.line,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: connection.problem
                                ? palette.danger
                                : hovered
                                ? palette.textMuted
                                : palette.textFaint,
                            decoration: connection.problem && hovered
                                ? TextDecoration.underline
                                : null,
                            decorationColor: palette.danger,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Whether a tap on a connection's status ([ConnectionLine], a phone's
/// word) reconnects at once before showing the connection: while it's
/// reconnecting or can't connect, as relaunching the app would (#214).
bool reconnectFirst(Link link) =>
    link == Link.connecting || link == Link.failing;

/// VoiceOver's hint on a connection's status that [reconnectFirst].
const reconnectAndShowHint = 'Reconnects and shows the connection';

/// The connection in full: how it connects, or why it can't and what to
/// do, with Reconnect.
class ConnectionDialog extends StatelessWidget {
  const ConnectionDialog({
    super.key,
    required this.backend,
    required this.close,
    this.machine,
  });

  final Backend backend;
  final String? machine;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: Connection.of(backend),
      builder: (context, _) {
        final connection = Connection(backend, machine: machine);
        final error = connection.error;
        // This Mac's own, off or blocked: what starts it, not Reconnect.
        final setup = SetupScope.of(context);
        final (String, VoidCallback)? fix = setup == null
            ? null
            : switch (connection.link) {
                Link.off => (
                  'Start Claktory',
                  () {
                    unawaited(setup.startEngine());
                    close();
                  },
                ),
                Link.blocked => (
                  'Open System Settings',
                  () => unawaited(setup.openLoginItems()),
                ),
                _ => null,
              };
        return FormFrame(
          title: connection.line,
          body: connection.details,
          fields: [
            // What went wrong exactly, for whoever looks into it.
            if (connection.problem && error.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.m),
                child: Text(
                  error,
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ),
          ],
          error: null,
          submitLabel: fix?.$1 ?? 'Reconnect',
          onSubmit:
              fix?.$2 ??
              () {
                backend.reconnectNow();
                close();
              },
          cancelLabel: 'Close',
          onCancel: close,
        );
      },
    );
  }
}

/// The computer this app's own Claktory runs on, as the footer names it:
/// a PC on Windows (Hermes 53466), a Mac everywhere else.
String get _thisComputer =>
    defaultTargetPlatform == TargetPlatform.windows ? 'PC' : 'Mac';
