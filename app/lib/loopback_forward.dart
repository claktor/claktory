import 'dart:async';
import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show PlatformException;
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'scope.dart';
import 'web_auth.dart';

/// Browser flows that come back to `http://localhost:<port>/…`, forwarded
/// to the Claktory's machine.
///
/// A sign-in page (a provider's in Settings → Providers, or one a CLI an
/// agent runs prints) sends the browser back to a port on the computer
/// that started it, where something listens: factoryd, or the CLI. When
/// the app shows a Claktory on another machine, the browser is on this
/// device instead, so the app listens on the same port here and forwards
/// each connection through the tunnel to that port on the Claktory's
/// machine (ALPN claktory-forward/1). It's generic: no provider is known
/// here, only ports found in the address opened.

/// The loopback ports [url] sends the browser to: its own host when it's
/// localhost or 127.0.0.1, and each `redirect_uri`, `redirect`,
/// `callback` or `callback_url` parameter that is one.
List<int> loopbackPorts(Uri url) {
  final ports = <int>{};
  void consider(Uri? address) {
    if (address == null || !address.hasPort) return;
    if (address.scheme != 'http' && address.scheme != 'https') return;
    if (!_loopbackHosts.contains(address.host.toLowerCase())) return;
    if (address.port <= 0 || address.port > 65535) return;
    ports.add(address.port);
  }

  consider(url);
  for (final name in _callbackParameters) {
    for (final value in url.queryParametersAll[name] ?? const <String>[]) {
      consider(Uri.tryParse(value));
    }
  }
  return ports.toList();
}

const _loopbackHosts = {'localhost', '127.0.0.1', '::1', '[::1]'};
const _callbackParameters = [
  'redirect_uri',
  'redirect',
  'callback',
  'callback_url',
];

/// One port forwarded: this device's 127.0.0.1 and ::1 at [port], to the
/// same port on the Claktory's machine.
abstract interface class PortForward {
  int get port;

  /// Connections it has passed on so far.
  int get connections;

  /// What went wrong with the last one (the backend's sentence, or why the
  /// tunnel couldn't carry it); empty while all's well.
  String get lastError;

  /// Stops listening; connections under way finish.
  Future<void> stop();
}

/// Why a port couldn't be forwarded.
enum ForwardProblem {
  /// Something on this device listens on it already.
  taken,

  /// The Claktory can't forward (a version from before it).
  unsupported,

  /// The tunnel couldn't open it.
  failed,
}

class ForwardFailure implements Exception {
  const ForwardFailure(this.problem, [this.message = '']);

  final ForwardProblem problem;
  final String message;

  @override
  String toString() => message.isEmpty ? problem.name : message;
}

/// A flow's forwards: the ports one address opened needs, until the flow
/// ends ([LoopbackForwards.stop]) or [LoopbackForwards.lifetime] passes.
class Forwarding {
  Forwarding._(this.url, this.ports);

  /// The address opened.
  final Uri url;
  final List<int> ports;

  /// Each port's problem, if it couldn't be forwarded.
  final problems = <int, ForwardProblem>{};

  Timer? _expiry;

  /// Whether all its ports listen.
  bool get working => problems.isEmpty;
}

/// The ports a Claktory on another machine forwards from this device, for
/// the browser flows the app opened. One listener per port, shared by the
/// flows that need it; it stops when the last of them does.
class LoopbackForwards extends ChangeNotifier {
  LoopbackForwards({
    required this.remote,
    required this.forward,
    this.forwardTo,
    this.lifetime = const Duration(minutes: 10),
  });

  /// Whether the Claktory is on another machine: on its own, its
  /// listeners are already where the browser goes.
  final bool Function() remote;

  /// Forwards one port (throws [ForwardFailure]).
  final Future<PortForward> Function(int port) forward;

  /// Forwards a port of this device to another, `target`, on the Claktory's
  /// machine (throws [ForwardFailure]): for websites shared over pipes.
  final Future<PortForward> Function(int port, int target)? forwardTo;

  /// How long a flow's ports are forwarded at most.
  final Duration lifetime;

  final _flows = <Forwarding>[];
  final _ports = <int, PortForward>{};
  Timer? _poll;
  var _seen = <int, (int, String)>{};

  /// The flows forwarding now, oldest first.
  List<Forwarding> get active => List.unmodifiable(_flows);

  /// Connections passed on so far for [flow]'s ports.
  int served(Forwarding flow) =>
      [for (final port in flow.ports) _ports[port]?.connections ?? 0]
          .fold(0, (sum, count) => sum + count);

  /// Why the last connection to one of [flow]'s ports couldn't be carried
  /// (the Claktory's sentence: "Nothing is listening on port 1455 on …");
  /// empty while all's well.
  String lastError(Forwarding flow) {
    for (final port in flow.ports) {
      final error = _ports[port]?.lastError ?? '';
      if (error.isNotEmpty) return error;
    }
    return '';
  }

  /// Forwards the loopback ports [url] needs, for [lifetime] or until
  /// [stop]; null when it needs none, or the Claktory is on this machine.
  /// Opening the same address again keeps its forwards [lifetime] longer.
  Future<Forwarding?> start(Uri url) async {
    if (!remote()) return null;
    final ports = loopbackPorts(url);
    if (ports.isEmpty) return null;
    for (final flow in _flows) {
      if (flow.url == url) {
        _expire(flow);
        return flow;
      }
    }
    final flow = Forwarding._(url, ports);
    _flows.add(flow);
    _expire(flow);
    for (final port in ports) {
      if (_ports.containsKey(port)) continue;
      try {
        _ports[port] = await forward(port);
      } on ForwardFailure catch (failure) {
        _log('could not forward $port: $failure');
        flow.problems[port] = failure.problem;
      }
    }
    if (!_flows.contains(flow)) {
      // Stopped while its ports were opening.
      await _release(flow);
      return flow;
    }
    _watch();
    notifyListeners();
    return flow;
  }

  /// The flow ended: its ports stop unless another flow needs them.
  Future<void> stop(Forwarding flow) async {
    if (!_flows.remove(flow)) return;
    flow._expiry?.cancel();
    await _release(flow);
    notifyListeners();
  }

  /// Stops every flow's forwards (the sidebar's Stop); shared websites'
  /// stay.
  Future<void> stopFlows() async {
    for (final flow in [..._flows]) {
      await stop(flow);
    }
  }

  /// Stops everything forwarded: the flows' ports and the shared websites'
  /// (the tunnel closed, or the Claktory was forgotten).
  Future<void> stopAll() async {
    await stopFlows();
    for (final id in [..._shares.keys]) {
      await _stopShare(id);
    }
    _shareProblems.clear();
  }

  // Websites a pipe's peer shares, opened on this device (#503). Each is
  // forwarded from a port of this device's own to the share's port on the
  // Claktory's machine, for as long as the share lasts there (not for
  // [lifetime]): a page left open keeps working, and so does a bookmark,
  // since the port here is the same each time ([sharePortFor]).

  final _shares = <String, ({int target, PortForward forward})>{};
  final _shareStarts = <String, Future<int>>{};
  final _shareProblems = <String, ForwardFailure>{};

  /// The port [shareId] is forwarded from on this device; null while it
  /// isn't.
  int? sharePort(String shareId) => _shares[shareId]?.forward.port;

  /// What went wrong with [shareId]'s last connection here (the sentence
  /// of the Claktory it's from, or why the tunnel couldn't carry it); empty
  /// while all's well or while it isn't forwarded. Said on an iPhone or
  /// iPad once its page didn't load (W6, #505).
  String shareLastError(String shareId) =>
      _shares[shareId]?.forward.lastError ?? '';

  /// Why [shareId] couldn't be forwarded the last time it was tried; null
  /// when it could, or wasn't tried.
  ForwardFailure? shareProblem(String shareId) => _shareProblems[shareId];

  /// Forwards website [shareId] to [target] on the Claktory's machine,
  /// unless it is already; the port here it's at. Throws [ForwardFailure]:
  /// [ForwardProblem.taken] when no port near its own is free here.
  Future<int> forwardShare(String shareId, int target) {
    final current = _shares[shareId];
    if (current != null && current.target == target) {
      return Future.value(current.forward.port);
    }
    return _shareStarts[shareId] ??= _startShare(shareId, target).whenComplete(
      () {
        // Not `=> remove(…)`: whenComplete would wait for the future that
        // returns, this one.
        _shareStarts.remove(shareId);
      },
    );
  }

  Future<int> _startShare(String shareId, int target) async {
    final forwardTo = this.forwardTo;
    if (forwardTo == null) {
      throw const ForwardFailure(ForwardProblem.unsupported);
    }
    await _stopShare(shareId);
    final first = sharePortFor(shareId);
    try {
      for (var i = 0; i < _shareTries; i++) {
        final port =
            _sharePortsFrom + (first - _sharePortsFrom + i) % _sharePorts;
        try {
          final forward = await forwardTo(port, target);
          _shares[shareId] = (target: target, forward: forward);
          _shareProblems.remove(shareId);
          notifyListeners();
          return port;
        } on ForwardFailure catch (failure) {
          if (failure.problem != ForwardProblem.taken) rethrow;
        }
      }
      throw const ForwardFailure(ForwardProblem.taken);
    } on ForwardFailure catch (failure) {
      _log('could not forward share $shareId: $failure');
      _shareProblems[shareId] = failure;
      notifyListeners();
      rethrow;
    }
  }

  /// Forwards exactly the shares in [wanted] (each id with its port on the
  /// Claktory's machine): the others stop, and those not forwarded yet
  /// start, quietly ([shareProblem] says why one couldn't).
  Future<void> keepShares(Map<String, int> wanted) async {
    if (!remote()) wanted = const {};
    for (final MapEntry(key: id, value: forwarded) in [..._shares.entries]) {
      if (wanted[id] != forwarded.target) await _stopShare(id);
    }
    _shareProblems.removeWhere((id, _) => !wanted.containsKey(id));
    for (final MapEntry(key: id, value: target) in wanted.entries) {
      if (_shares.containsKey(id) || _shareStarts.containsKey(id)) continue;
      // Only a passing failure is tried again with each Workspace; Open
      // tries the others again.
      final problem = _shareProblems[id]?.problem;
      if (problem != null && problem != ForwardProblem.failed) continue;
      try {
        await forwardShare(id, target);
      } on ForwardFailure {
        // Said by shareProblem; tried again with the next Workspace.
      }
    }
  }

  Future<void> _stopShare(String shareId) async {
    final forwarded = _shares.remove(shareId);
    if (forwarded == null) return;
    await forwarded.forward.stop();
    notifyListeners();
  }

  void _expire(Forwarding flow) {
    flow._expiry?.cancel();
    flow._expiry = Timer(lifetime, () => unawaited(stop(flow)));
  }

  Future<void> _release(Forwarding flow) async {
    for (final port in flow.ports) {
      if (_flows.any((other) => other.ports.contains(port))) continue;
      final forward = _ports.remove(port);
      if (forward != null) await forward.stop();
    }
    if (_flows.isEmpty) {
      _poll?.cancel();
      _poll = null;
    }
  }

  /// While anything is forwarded, notices connections passed on and
  /// their errors.
  void _watch() {
    _poll ??= Timer.periodic(const Duration(seconds: 1), (_) {
      final seen = {
        for (final MapEntry(:key, :value) in _ports.entries)
          key: (value.connections, value.lastError),
      };
      if (!mapEquals(seen, _seen)) {
        _seen = seen;
        notifyListeners();
      }
    });
  }

  @override
  void dispose() {
    for (final flow in _flows) {
      flow._expiry?.cancel();
    }
    _flows.clear();
    _poll?.cancel();
    for (final forward in _ports.values) {
      unawaited(forward.stop());
    }
    _ports.clear();
    for (final forwarded in _shares.values) {
      unawaited(forwarded.forward.stop());
    }
    _shares.clear();
    super.dispose();
  }
}

/// Opens [url] in the browser, forwarding the loopback ports it comes back
/// to first ([LoopbackForwards.start]); the forwarding, if any.
///
/// On iPhone a page that comes back to this device opens in the system's
/// sign-in sheet rather than Safari: the app stays in front, so its
/// listeners keep working while the page loads. [closeSheet] closes it.
Future<Forwarding?> openInBrowser(
  Uri url, {
  required LoopbackForwards forwards,
  Future<bool> Function(Uri url)? launch,
}) async {
  Forwarding? forwarding;
  try {
    forwarding = await forwards.start(url);
  } catch (error) {
    _log('could not forward for $url: $error');
  }
  final sheet =
      forwarding != null &&
      forwarding.problems.isEmpty &&
      defaultTargetPlatform == TargetPlatform.iOS &&
      launch == null;
  try {
    if (sheet) {
      unawaited(
        webAuth(url, _sheetScheme).then(
          (_) {},
          onError: (Object error) => _log('the sign-in sheet: $error'),
        ),
      );
    } else if (!await (launch ?? launchUrl)(url)) {
      _log('the browser did not open $url');
    }
  } on PlatformException catch (error) {
    _log('could not open $url: ${error.message}');
  }
  return forwarding;
}

/// Closes the sign-in sheet [openInBrowser] opened on iPhone, once its
/// flow is done.
Future<void> closeSheet() async {
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  await cancelWebAuth();
}

/// Nothing comes back to it: the sheet stays until closed.
const _sheetScheme = 'claktory-forward';

/// Opens a link the person clicked (in a conversation, say), forwarding
/// the ports it comes back to, for [LoopbackForwards.lifetime].
Future<void> openLink(BuildContext context, Uri url) async {
  final scope = context.getInheritedWidgetOfExactType<AppScope>();
  if (scope == null) {
    // Outside the app (a widget on its own): nothing to forward through.
    await (_linkLauncher ?? launchUrl)(url);
    return;
  }
  await openInBrowser(
    url,
    forwards: scope.backend.forwards,
    launch: _linkLauncher,
  );
}

/// Opens links' pages; tests replace it.
@visibleForTesting
Future<bool> Function(Uri url)? linkLauncher;

Future<bool> Function(Uri url)? get _linkLauncher => linkLauncher;

/// How a flow's forwarding is doing, under its sign-in page's buttons:
/// waiting for the browser, or why it can't come back through this device
/// (pasting still works where the flow takes it).
class ForwardingLine extends StatelessWidget {
  const ForwardingLine({
    super.key,
    required this.forwards,
    required this.forwarding,
    required this.machine,
  });

  final LoopbackForwards forwards;
  final Forwarding forwarding;

  /// The Claktory's name.
  final String machine;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final problem = forwardingProblem(forwards, forwarding, machine);
    return Text(
      problem ?? forwardingState(forwards, forwarding, machine),
      style: TextStyles.caption.copyWith(
        color: problem == null ? palette.textMuted : palette.danger,
      ),
    );
  }
}

/// The sidebar footer's line while links opened from conversations wait for
/// the browser to come back through this device: "Waiting for the
/// browser…", with Stop. Nothing otherwise, nor for the sign-in at
/// [except] (Settings shows its own).
class ForwardingsLine extends StatelessWidget {
  const ForwardingsLine({
    super.key,
    required this.forwards,
    this.except = '',
    this.padding = EdgeInsets.zero,
  });

  final LoopbackForwards forwards;
  final String except;

  /// Around it, while it shows.
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: forwards,
    builder: (context, _) {
      final palette = Palette.of(context);
      final shown = [
        for (final flow in forwards.active)
          if ('${flow.url}' != except) flow,
      ];
      if (shown.isEmpty) return const SizedBox.shrink();
      final machine = claktoryName(AppScope.of(context));
      final problem = forwardingProblem(forwards, shown.last, machine);
      final line = HoverCard(
        holdToShow: true,
        card: (context) => Text(
          problem ??
              'A page you opened needs to reach $machine, so this '
                  '$thisDeviceKind passes it on for up to 10 minutes.',
        ),
        child: Row(
          children: [
            if (problem == null) ...[
              const LiveDot(),
              const SizedBox(width: Space.s),
            ],
            Expanded(
              child: Text(
                problem == null
                    ? forwardingState(
                        forwards,
                        shown.last,
                        machine,
                        short: true,
                      )
                    : "The browser can't reach $machine",
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.caption.copyWith(
                  color: problem == null ? palette.textMuted : palette.danger,
                ),
              ),
            ),
            Pressable(
              onTap: () => unawaited(forwards.stopFlows()),
              builder: (context, hovered) => Text(
                'Stop',
                style: TextStyles.caption.copyWith(
                  color: hovered ? palette.text : palette.textFaint,
                ),
              ),
            ),
          ],
        ),
      );
      return Padding(padding: padding, child: line);
    },
  );
}

/// How [forwarding] is doing while all's well: waiting for the browser,
/// or the browser came back through it.
/// [short] for the sidebar's footer.
String forwardingState(
  LoopbackForwards forwards,
  Forwarding forwarding,
  String machine, {
  bool short = false,
}) => forwards.served(forwarding) == 0
    ? (short
          ? 'Waiting for the browser…'
          : 'Waiting for you to finish in the browser…')
    : (short
          ? 'Passed on to $machine'
          : 'Done in the browser. Passed on to $machine.');

/// Why [forwarding] can't bring the browser back to [machine], in words
/// for the person: a port it couldn't forward, or the last connection
/// [machine] couldn't take; null while all's well.
String? forwardingProblem(
  LoopbackForwards forwards,
  Forwarding forwarding,
  String machine,
) {
  for (final MapEntry(key: port, value: problem)
      in forwarding.problems.entries) {
    return switch (problem) {
      ForwardProblem.taken =>
        'Another app on this $thisDeviceKind is using port $port, so the '
            "browser can't reach $machine. Quit that app and try again, or "
            'paste the address the browser ends up on.',
      ForwardProblem.unsupported =>
        '$machine needs an update before this $thisDeviceKind can pass the '
            'browser on to it. Paste the address the browser ends up on.',
      ForwardProblem.failed =>
        "The browser can't reach $machine right now. Paste the address it "
            'ends up on.',
    };
  }
  final error = forwards.lastError(forwarding);
  return error.isEmpty ? null : error;
}

/// Whether this device is a phone or tablet: it forwards only the shares
/// opened on it, never all of them in the background, and has no use for a
/// copied address, which works only while Claktory is up.
bool get sharesOnTouchDevice => switch (defaultTargetPlatform) {
  TargetPlatform.iOS || TargetPlatform.android => true,
  _ => false,
};

/// Where a shared website's port on this device is looked for: the same
/// number for the same share each time, so its address here, and a
/// bookmark of it, keep working; away from the ports programs usually take
/// (3000, 5173, 8080…), so it never takes one from the person's own.
const _sharePortsFrom = 20000;
const _sharePorts = 10000;

/// How many ports on from its own a share tries before giving up.
const _shareTries = 20;

/// The port a shared website is first tried at on this device: between
/// 20000 and 29999, from its id (FNV-1a).
int sharePortFor(String shareId) {
  var hash = 0x811c9dc5;
  for (final unit in shareId.codeUnits) {
    hash = ((hash ^ unit) * 0x01000193) & 0xffffffff;
  }
  return _sharePortsFrom + hash % _sharePorts;
}

/// [address] with its port [port]: a share's address as this device opens
/// it.
String withPort(String address, int port) {
  final uri = Uri.tryParse(address);
  if (uri == null || !uri.hasAuthority) return address;
  return uri.replace(port: port).toString();
}

void _log(String message) => developer.log(message, name: 'factory.forward');
