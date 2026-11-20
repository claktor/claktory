import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'devices.dart' show FactoryChooser, thisDeviceKind;
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'tunnel/api/tunnel.dart';
import 'tunnel_client.dart' show idleTunnel, tunnelAdvice;

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
}

/// A Claktory's connection as the app shows it: its state and the words
/// for it, in the sidebar's footer, on the rail's tile and on a phone.
class Connection {
  Connection(this.backend, {String? machine})
    : link = _linkOf(backend),
      machine =
          machine ?? (backend.target == null ? null : claktoryName(backend));

  final Backend backend;
  final Link link;

  /// The Claktory's name, when it's on another machine; null for this
  /// Mac's own.
  final String? machine;

  static Link _linkOf(Backend backend) {
    if (backend.refused.value != null) return Link.refused;
    if (backend.connected.value) return Link.live;
    final tunnel = backend.tunnel.value;
    // Back from the background or a network change, it reconnects without
    // a word for a few seconds first.
    if (backend.target != null &&
        !backend.reconnecting.value &&
        tunnel != null &&
        tunnel.failure != TunnelFailure.none) {
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
    backend.tunnel,
  ]);

  String get _name => machine ?? 'this Claktory';

  /// The footer's line, and the dialog's title.
  String get line => switch (link) {
    Link.live =>
      machine == null ? 'Running on this Mac' : 'Connected to $machine',
    Link.connecting when backend.reconnecting.value => 'Reconnecting…',
    Link.connecting =>
      machine == null ? 'Connecting…' : 'Connecting to $machine…',
    Link.failing => "Can't connect to $_name",
    Link.refused => "$_name doesn't allow this device",
  };

  /// A word or two, beside a name that's already shown (a phone's rows).
  String get short => switch (link) {
    Link.live => 'Connected',
    Link.connecting =>
      backend.reconnecting.value ? 'Reconnecting…' : 'Connecting…',
    Link.failing => "Can't connect",
    Link.refused => 'Not allowed',
  };

  /// How it connects, and since when.
  String get details {
    final since = backend.connectedSince.value;
    final at = since == null ? null : logTime(since);
    switch (link) {
      case Link.live:
        if (machine == null) {
          return 'Claktory is running on this Mac.'
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
      case Link.failing || Link.refused:
        return tunnelAdvice(
          backend.tunnel.value ?? idleTunnel,
          _name,
          device: thisDeviceKind,
        );
    }
  }

  /// Red when it can't connect.
  bool get problem => link == Link.failing || link == Link.refused;
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
      Link.failing || Link.refused => Dot(color: palette.danger),
    };
  }
}

/// The sidebar footer's connection line: its mark and words, with how it
/// connects on hover (or a long press), and a click for [ConnectionDialog].
/// Turned away, a click lists the Claktories instead: trying again won't
/// help.
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
        return HoverCard(
          holdToShow: true,
          card: (context) => Text(
            connection.details,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
          child: Semantics(
            button: true,
            label: connection.line,
            child: Pressable(
              onTap: () => connection.link == Link.refused
                  ? showModal(
                      context,
                      (context, close) => FactoryChooser(close: close),
                    )
                  : showModal(
                      context,
                      (context, close) => ConnectionDialog(
                        backend: backend,
                        machine: machine,
                        close: close,
                      ),
                    ),
              builder: (context, hovered) => ExcludeSemantics(
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
        );
      },
    );
  }
}

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
        final error = backend.tunnel.value?.error ?? '';
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
          submitLabel: 'Reconnect',
          onSubmit: () {
            backend.reconnectNow();
            close();
          },
          cancelLabel: 'Close',
          onCancel: close,
          secondary: Button(
            label: 'Switch Claktory…',
            onPressed: () {
              close();
              showModal(
                context,
                (context, close) => FactoryChooser(close: close),
              );
            },
          ),
        );
      },
    );
  }
}
