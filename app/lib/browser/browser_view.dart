import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/lines.dart';
import '../mobile/metrics.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../claktory_identity.dart' show claktoryName;
import '../device_account.dart' show AccountClaktory;
import '../rail.dart' show Claktories;
import '../scope.dart';
import '../settings_view.dart' show modelLabel;
import 'bridge.dart';
import 'session.dart';

// B5a: what the person sees of browser control (browser-control.md §5,
// "What the person sees"): a card for each session an agent asked for,
// where they choose the tab on this Mac or decline; then, while it runs,
// one line, "June is reading example.com on this Mac · Stop", with what
// it did. Every device shows the line and can Stop; only the Mac the tab
// is on can choose. The agent's purpose shows as its own quoted words and
// the page's title only in quotes: neither ever reads as Claktory's. Off
// for everyone until browser control passes its security proof; nothing
// shows it yet.

/// "123 456": the pairing digits, as Chrome's popup shows them too.
String pairingDigits(String code) =>
    code.length == 6 ? '${code.substring(0, 3)} ${code.substring(3)}' : code;

/// Whether [session] asks to act on the page too (B3b's "assist": click,
/// type, open pages on its site, each after the person allows it), not
/// only read it.
bool browserAssists(BrowserSession session) => session.mode == 'assist';

/// What the person agrees to, before choosing the tab: who reads it, with
/// which model, what else it may do ([assist]), and the limits.
String browserDisclosure({
  required String agent,
  required String host,
  required String model,
  required String provider,
  bool assist = false,
}) {
  final reads = model.isEmpty
      ? '$agent has no model available right now.'
      : '$agent runs on $model${provider.isEmpty ? '' : ' from $provider'}, '
            'which sees what $agent reads.';
  final may = assist
      ? '$agent can read and scroll that tab for up to 15 minutes. It can '
            'also click, type and open pages on $host, one action at a time, '
            'each only after you allow it on this Mac.'
      : '$agent can read the text on that tab and scroll it, nothing else, '
            'for up to 15 minutes.';
  return '$may $reads It ends if the tab leaves $host.';
}

/// A session an agent asked for or is running: [session] as the backend
/// lists it; [local] is this Mac's broker, null on a device that can't
/// choose.
class BrowserSessionCard extends StatefulWidget {
  const BrowserSessionCard({super.key, required this.session, this.local});

  final BrowserSession session;
  final BrowserLocal? local;

  @override
  State<BrowserSessionCard> createState() => _BrowserSessionCardState();
}

class _BrowserSessionCardState extends State<BrowserSessionCard> {
  var _busy = false;
  String? _error;

  Future<void> _do(Future<void> Function() action) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Stops it: here first when this Mac runs it, else asks the backend.
  Future<void> _stop(Backend backend) => _do(() async {
    final local = widget.local;
    if (local?.active?.intent.id == widget.session.id) {
      await local!.stopSession();
    } else {
      await backend.sendBrowser(
        ClientMessage(
          browserStop: BrowserStop(
            sessionId: widget.session.id,
            reason: 'stopped',
          ),
        ),
      );
    }
  });

  @override
  Widget build(BuildContext context) {
    final local = widget.local;
    if (local == null) return _card(context);
    return ListenableBuilder(
      listenable: local,
      builder: (context, _) => _card(context),
    );
  }

  /// Set up Chrome on this Mac: the person's own choice, never automatic.
  /// When another app's connection is in the way, asks before replacing it.
  Widget _setUp(BrowserLocal local) => Align(
    alignment: AlignmentDirectional.centerStart,
    child: Button(
      label: 'Set up Chrome on this Mac',
      onPressed: _busy
          ? null
          : () => _do(() async {
              final failed = await local.setUpChrome();
              if (failed == null || !failed.otherProgram || !mounted) return;
              var replace = false;
              await showModal(
                context,
                (context, close) => FormFrame(
                  title: "Replace another app's connection to Chrome?",
                  body:
                      'Browser control will work, but that app may stop '
                      'working with Chrome.',
                  fields: const [],
                  error: null,
                  submitLabel: 'Replace',
                  onSubmit: () {
                    replace = true;
                    close();
                  },
                  onCancel: close,
                ),
              );
              if (replace) await local.setUpChrome(replace: true);
            }),
    ),
  );

  Widget _card(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final session = widget.session;
    final agent = workspace.agents
        .where((agent) => agent.id == session.agentId)
        .firstOrNull;
    final name = agent?.name ?? session.agentId;
    final color = agentColor(agent?.color ?? 'slate');
    final host = browserHost(session.origin);
    final pending = session.state == 'pending';
    final ended = session.state == 'ended';
    final local = widget.local;
    final here = local?.active?.intent.id == session.id;
    final where = here
        ? 'this Mac'
        : workspace.remote.devices
                  .where((device) => device.id == session.deviceId)
                  .firstOrNull
                  ?.name ??
              'a Mac';
    // On the Mac that would serve it, whose Claktory asks, as this app
    // knows it, so another's can't pass as the person's own.
    final whose = local == null || local.claktory.isEmpty
        ? name
        : '$name in ${local.claktory}';
    final assist = browserAssists(session);
    final heading = pending
        ? '$whose wants to ${assist ? 'use' : 'read'} a page in Chrome'
        : ended
        ? '$name ${assist ? 'used' : 'read'} $host'
        : '$name is ${assist ? 'using' : 'reading'} $host on $where';
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(color: color.withValues(alpha: ended ? 0.25 : 0.55)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Expanded(
                child: Text(
                  heading,
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (session.purpose.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            // The agent's words, never Claktory's.
            Text(
              '$name says: “${session.purpose}”',
              style: type.body.copyWith(color: palette.textMuted),
            ),
          ],
          const SizedBox(height: Space.s),
          Text(
            'Only $host',
            style: type.body.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w500,
            ),
          ),
          if (pending) ..._choose(context, palette, type, name, host, agent),
          if (!pending && !ended) ..._running(context, palette, type, here),
          if (ended) ...[
            const SizedBox(height: Space.xs),
            Text(
              'Ended: ${browserEndReason(session.reason, agent: name)}',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
          if (_error case final error?) ...[
            const SizedBox(height: Space.s),
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ],
      ),
    );
  }

  List<Widget> _choose(
    BuildContext context,
    Palette palette,
    TypeScale type,
    String name,
    String host,
    Agent? agent,
  ) {
    final backend = AppScope.of(context);
    final local = widget.local;
    final session = widget.session;
    final tabs = local?.tabsFor(session) ?? const <BrowserTab>[];
    final model = agent == null || !agent.modelAvailable
        ? ''
        : switch (backend.workspace.model(agent.model)) {
            final model? => modelLabel(model),
            null => agent.model,
          };
    final provider = agent == null
        ? ''
        : backend.workspace.workspace.providers
                  .where((p) => p.id == agent.model.split('/').first)
                  .firstOrNull
                  ?.name ??
              '';
    return [
      const SizedBox(height: Space.xs),
      Text(
        browserDisclosure(
          agent: name,
          host: host,
          model: model,
          provider: provider,
          assist: browserAssists(session),
        ),
        style: type.caption.copyWith(color: palette.textMuted),
      ),
      const SizedBox(height: Space.m),
      if (local == null)
        Text(
          'Choose the tab on the Mac where Chrome is open.',
          style: type.caption.copyWith(color: palette.textFaint),
        )
      else if (tabs.isEmpty && local.problem != null) ...[
        // Why no tab can reach this Mac (it couldn't start, or connect).
        Text(
          local.problem!,
          style: type.caption.copyWith(color: palette.danger),
        ),
        if (local.running && local.chromeSetUp != true) _setUp(local),
      ] else if (tabs.isEmpty && local.chromeSetUp == false)
        // Chrome learns of Claktory's helper only by the person's click.
        _setUp(local)
      else if (tabs.isEmpty)
        Text(
          'In Chrome, open $host and click the Claktory button. Its tab '
          'shows up here with the digits Chrome shows.',
          style: type.caption.copyWith(color: palette.textFaint),
        )
      else
        for (final tab in tabs)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.s),
            child: ActionLine(
              lead: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    pairingDigits(tab.code),
                    style: type.body.copyWith(
                      fontFamily: TextStyles.mono,
                      color: palette.text,
                    ),
                  ),
                  // The page's own words: only in quotes.
                  if (tab.title.isNotEmpty)
                    Text(
                      '“${tab.title}”',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                ],
              ),
              action: Button(
                label: browserAssists(session) ? 'Allow' : 'Allow reading',
                primary: true,
                onPressed: _busy
                    ? null
                    : () => _do(() async {
                        if (!await local.allow(session, tab, agentName: name)) {
                          setState(
                            () => _error =
                                "That tab can't be chosen now. Click the "
                                'Claktory button in Chrome again.',
                          );
                        }
                      }),
              ),
            ),
          ),
      const SizedBox(height: Space.s),
      Align(
        alignment: AlignmentDirectional.centerEnd,
        child: Button(
          label: 'Decline',
          onPressed: _busy
              ? null
              : () => _do(
                  () => local != null
                      ? local.decline(session)
                      : backend.sendBrowser(
                          ClientMessage(
                            browserStop: BrowserStop(
                              sessionId: session.id,
                              reason: 'stopped',
                            ),
                          ),
                        ),
                ),
        ),
      ),
    ];
  }

  List<Widget> _running(
    BuildContext context,
    Palette palette,
    TypeScale type,
    bool here,
  ) {
    final session = widget.session;
    final backend = AppScope.of(context);
    final left = Duration(
      milliseconds:
          session.deadlineMs.toInt() - DateTime.now().millisecondsSinceEpoch,
    );
    final activity = here ? widget.local!.activity : const <BrowserActivity>[];
    final review = here
        ? widget.local!.active?.review ?? BrowserReview.none
        : BrowserReview.none;
    return [
      const SizedBox(height: Space.xs),
      ActionLine(
        lead: Text(
          [
            '${session.commandsUsed} of ${session.commandsLimit} actions',
            if (!left.isNegative) '${left.inMinutes + 1} min left',
          ].join(' · '),
          style: type.caption.copyWith(color: palette.textMuted),
        ),
        action: Button(
          label: 'Stop',
          primary: true,
          onPressed: _busy ? null : () => _stop(backend),
        ),
      ),
      // A reviewed action waits in the native panel on this Mac: only its
      // buttons decide it; this only brings it forward.
      if (review == BrowserReview.waiting) ...[
        const SizedBox(height: Space.s),
        ActionLine(
          lead: Text(
            'Review it in the Claktory panel on this Mac',
            style: type.body.copyWith(color: palette.text),
          ),
          action: Button(
            label: 'Show panel',
            onPressed: () => unawaited(
              widget.local!.bridge.showReview(widget.local!.active!.tab.helper),
            ),
          ),
        ),
      ] else if (review == BrowserReview.allowed) ...[
        const SizedBox(height: Space.s),
        Text(
          browserReviewWords['go_back']!,
          style: type.body.copyWith(color: palette.signal),
        ),
      ],
      if (activity.isNotEmpty) ...[
        const SizedBox(height: Space.s),
        for (final line in activity.reversed.take(6))
          Text(
            line.text,
            style: type.caption.copyWith(color: palette.textFaint),
          ),
      ],
    ];
  }
}

/// Keeps this Mac's broker in step with the backend: started while
/// browser control is on for any agent, its capability said again on
/// every new connection, and the backend's commands and endings relayed
/// to it.
class BrowserControl {
  BrowserControl(this.backend, this.local, {Stream<ServerMessage>? messages}) {
    _messages = (messages ?? backend.browserMessages).listen((message) {
      switch (message.whichBody()) {
        case ServerMessage_Body.browserCommand:
          unawaited(local.command(message.browserCommand));
        case ServerMessage_Body.browserEnded:
          unawaited(
            local.ended(
              message.browserEnded.sessionId,
              message.browserEnded.reason,
            ),
          );
        default:
          break;
      }
    });
    backend.connected.addListener(_connected);
    backend.workspace.addListener(_sync);
    _sync();
  }

  final Backend backend;
  final BrowserLocal local;
  late final StreamSubscription<ServerMessage> _messages;

  /// Browser control is on for some agent here.
  bool get wanted => browserWanted(backend.workspace.workspace);

  void _sync() {
    if (wanted && !local.running && local.problem == null) {
      unawaited(local.start());
    } else if (!wanted && local.running) {
      unawaited(local.stop());
    }
  }

  void _connected() {
    if (backend.connected.value && local.running) {
      unawaited(local.sayCapability());
    }
  }

  void dispose() {
    unawaited(_messages.cancel());
    backend.connected.removeListener(_connected);
    backend.workspace.removeListener(_sync);
  }
}

/// Whether browser control is on for some agent in [workspace]. A backend
/// without it (FACTORY_BROWSER_PREVIEW off) has no such extension.
bool browserWanted(Workspace workspace) => workspace.agents.any(
  (agent) =>
      agent.extensions.any((e) => e.id == 'browser_control' && e.enabled),
);

/// A Claktory this Mac's browser control could serve: its backend, the
/// endpoint the app dials it by (null for this Mac's own), and its name as
/// this app knows it.
typedef BrowserCandidate = ({Backend backend, String? endpointId, String name});

/// This Mac's browser control, for one Claktory at a time: Chrome's helpers
/// reach one broker, so it serves the first Claktory in the rail whose
/// agents have browser control on, and stays with it while they do. Only
/// the person's own: this Mac's Claktory, or one their Claktory account
/// lists, by the endpoint the app dials it by; never one merely paired,
/// which may be someone else's (Fugo's B6a gate).
class BrowserHost extends ChangeNotifier {
  BrowserHost({
    required this.candidates,
    required Listenable changes,
    required Listenable account,
    required this.theirs,
    BrowserBridge? bridge,
  }) : _changes = changes,
       _account = account,
       bridge = bridge ?? BrowserBridge() {
    changes.addListener(_sync);
    account.addListener(_listTheirs);
    _listTheirs();
  }

  /// For the app's Claktories, with the person's account's list.
  factory BrowserHost.of(Claktories claktories) {
    final account = claktories.connections.account;
    return BrowserHost(
      candidates: () => [
        for (final claktory in claktories.all)
          (
            backend: claktory.backend,
            endpointId: claktory.target?.endpointId,
            name: claktoryName(claktory.backend),
          ),
      ],
      changes: claktories,
      account: account,
      theirs: () => account.signedIn ? account.claktories() : null,
    );
  }

  final List<BrowserCandidate> Function() candidates;
  final Listenable _changes;
  final Listenable _account;

  /// The Claktories on the person's account; null while signed out.
  final Future<List<AccountClaktory>>? Function() theirs;
  final BrowserBridge bridge;

  /// The account's Claktories' names, by endpoint id (lowercase).
  var _theirs = const <String, String>{};

  /// The remote endpoints open when the account's list was last read.
  var _listedFor = const <String>{};
  var _listing = false;

  /// The open Claktories' backends, whose workspaces it watches.
  final _watched = <Backend>{};
  Backend? _backend;
  BrowserLocal? _local;
  BrowserControl? _control;

  /// The broker, when it serves [backend]'s Claktory.
  BrowserLocal? localFor(Backend backend) =>
      identical(backend, _backend) ? _local : null;

  Future<void> _listTheirs() async {
    final listed = theirs();
    _listedFor = _remotes();
    if (listed == null) {
      _theirs = const {};
      _sync();
      return;
    }
    _listing = true;
    try {
      final claktories = await listed;
      _theirs = {
        for (final claktory in claktories)
          if (claktory.endpointId.isNotEmpty)
            claktory.endpointId.toLowerCase(): claktory.name,
      };
    } catch (_) {
      // Unreadable, no remote Claktory is served; this Mac's own needs no
      // list (Fugo's rule).
      _theirs = const {};
    } finally {
      _listing = false;
    }
    _sync();
  }

  Set<String> _remotes() => {
    for (final candidate in candidates())
      if (candidate.endpointId case final endpoint? when endpoint.isNotEmpty)
        endpoint.toLowerCase(),
  };

  /// [candidate]'s name if it's the person's own, else null.
  String? _own(BrowserCandidate candidate) {
    final endpoint = candidate.endpointId;
    if (endpoint == null) return candidate.name;
    if (endpoint.isEmpty) return null;
    return _theirs[endpoint.toLowerCase()];
  }

  void _sync() {
    final all = candidates();
    // A Claktory paired since the account's list was read: read it again.
    final remotes = _remotes();
    if (!_listing && !remotes.every(_listedFor.contains)) {
      unawaited(_listTheirs());
    }
    final open = [for (final candidate in all) candidate.backend];
    for (final backend in open) {
      if (_watched.add(backend)) backend.workspace.addListener(_sync);
    }
    for (final backend in _watched.toList()) {
      if (!open.contains(backend)) {
        backend.workspace.removeListener(_sync);
        _watched.remove(backend);
      }
    }
    final own = [
      for (final candidate in all)
        if (_own(candidate) case final name?)
          (backend: candidate.backend, name: name),
    ];
    bool serves(Backend backend) =>
        own.any((each) => identical(each.backend, backend)) &&
        browserWanted(backend.workspace.workspace);
    final current = _backend;
    if (current != null && serves(current)) {
      _local?.claktory = own
          .firstWhere((each) => identical(each.backend, current))
          .name;
    }
    final next = current != null && serves(current)
        ? current
        : own
              .where((each) => browserWanted(each.backend.workspace.workspace))
              .firstOrNull
              ?.backend;
    if (identical(next, current)) return;
    // Turned off everywhere, rather than its Claktory going: Chrome
    // forgets the helper too.
    _detach(turnOff: next == null && current != null && open.contains(current));
    if (next != null) {
      final local = BrowserLocal(
        bridge: bridge,
        backend: BackendBrowserPort(next),
        claktory: own.firstWhere((each) => identical(each.backend, next)).name,
      );
      _backend = next;
      _local = local;
      _control = BrowserControl(next, local);
    }
    notifyListeners();
  }

  void _detach({bool turnOff = false}) {
    _control?.dispose();
    final local = _local;
    if (local != null) {
      unawaited(
        (turnOff ? local.turnOff() : local.stop()).whenComplete(local.dispose),
      );
    }
    _backend = null;
    _local = null;
    _control = null;
  }

  @override
  void dispose() {
    _changes.removeListener(_sync);
    _account.removeListener(_listTheirs);
    for (final backend in _watched) {
      backend.workspace.removeListener(_sync);
    }
    _detach();
    super.dispose();
  }
}

/// Gives the cards below this Mac's [BrowserHost]; none on other devices.
class BrowserScope extends InheritedNotifier<BrowserHost> {
  const BrowserScope({
    super.key,
    required BrowserHost? host,
    required super.child,
  }) : super(notifier: host);

  /// This Mac's broker for the shown Claktory, if it serves it.
  static BrowserLocal? localOf(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<BrowserScope>()
      ?.notifier
      ?.localFor(AppScope.of(context));
}

/// How long an ended session's card stays in its agent's conversation.
const browserEndedShows = Duration(minutes: 10);

/// The sessions [agentId] asked for or runs, newest first: those waiting or
/// running, and one that ended in the last [browserEndedShows].
List<BrowserSession> browserSessionsOf(
  Workspace workspace,
  String agentId,
  DateTime now,
) => [
  for (final session in workspace.browserSessions)
    if (session.agentId == agentId &&
        (session.state != 'ended' ||
            now.millisecondsSinceEpoch - session.endedMs.toInt() <
                browserEndedShows.inMilliseconds))
      session,
]..sort((a, b) => b.createdMs.compareTo(a.createdMs));

/// [agent]'s browser sessions in its conversation, above the message box:
/// waiting, running, and for a while after one ends, why. Nothing while
/// there are none, so nothing shows where browser control is off.
class BrowserSessionStrip extends StatefulWidget {
  const BrowserSessionStrip({super.key, required this.agentId});

  final String agentId;

  @override
  State<BrowserSessionStrip> createState() => _BrowserSessionStripState();
}

class _BrowserSessionStripState extends State<BrowserSessionStrip> {
  Timer? _expire;

  @override
  void dispose() {
    _expire?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final local = BrowserScope.localOf(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final now = DateTime.now();
        final sessions = browserSessionsOf(
          backend.workspace.workspace,
          widget.agentId,
          now,
        );
        // Gone from view once the newest ended one has shown long enough.
        _expire?.cancel();
        final ended = [
          for (final session in sessions)
            if (session.state == 'ended') session.endedMs.toInt(),
        ];
        if (ended.isNotEmpty) {
          final first = ended.reduce((a, b) => a < b ? a : b);
          _expire = Timer(
            DateTime.fromMillisecondsSinceEpoch(first)
                .add(browserEndedShows)
                .difference(now),
            () {
              if (mounted) setState(() {});
            },
          );
        }
        if (sessions.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final session in sessions.take(2))
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.s),
                  child: BrowserSessionCard(
                    key: ValueKey(session.id),
                    session: session,
                    local: local,
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// [BrowserBackendPort] over the backend's connection.
class BackendBrowserPort implements BrowserBackendPort {
  const BackendBrowserPort(this.backend);

  final Backend backend;

  @override
  Future<void> capability({
    required bool helperReady,
    required String brokerInstance,
  }) => backend.sendBrowser(
    ClientMessage(
      browserCapability: BrowserCapability(
        version: 1,
        operations: const ['read', 'scroll'],
        helperReady: helperReady,
        brokerInstance: brokerInstance,
      ),
    ),
  );

  @override
  Future<void> grant({
    required String sessionId,
    required String brokerInstance,
    required String origin,
    required String lease,
  }) => backend.sendBrowser(
    ClientMessage(
      browserGrant: BrowserGrant(
        sessionId: sessionId,
        brokerInstance: brokerInstance,
        origin: origin,
        lease: lease,
      ),
    ),
  );

  @override
  Future<void> result({
    required String sessionId,
    required int commandId,
    required String resultJson,
    required String failure,
    required int macMs,
  }) => backend.sendBrowser(
    ClientMessage(
      browserResult: BrowserResult(
        sessionId: sessionId,
        commandId: Int64(commandId),
        resultJson: resultJson,
        failure: failure,
        macMs: macMs,
      ),
    ),
  );

  @override
  Future<void> heartbeat({required String sessionId, required String lease}) =>
      backend.sendBrowser(
        ClientMessage(
          browserHeartbeat: BrowserHeartbeat(
            sessionId: sessionId,
            lease: lease,
          ),
        ),
      );

  @override
  Future<void> stop({required String sessionId, required String reason}) =>
      backend.sendBrowser(
        ClientMessage(
          browserStop: BrowserStop(sessionId: sessionId, reason: reason),
        ),
      );
}
