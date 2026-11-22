import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';

import '../protocol/factory/v1/factory.pb.dart';
import 'bridge.dart';

// B5a: what this Mac holds of browser control, in memory only (nothing is
// kept on the device): the tabs whose helpers connected, the one the
// person chose for a pending session, and that session while it runs. It
// relays the backend's commands to that tab and its results back, beats
// the heartbeat, and stops first here, then tells the backend
// (browser-control.md §5). Nothing here grants anything by itself: only
// the person's choice on this Mac ([BrowserLocal.allow]) binds a tab.

/// What the Mac sends the backend, as the contract has it (b3a: Browser
/// Capability, Grant, Result, Heartbeat, Stop).
abstract class BrowserBackendPort {
  Future<void> capability({
    required bool helperReady,
    required String brokerInstance,
  });

  Future<void> grant({
    required String sessionId,
    required String brokerInstance,
    required String origin,
    required String lease,
  });

  Future<void> result({
    required String sessionId,
    required int commandId,
    required String resultJson,
    required String failure,
    required int macMs,
  });

  Future<void> heartbeat({required String sessionId, required String lease});

  Future<void> stop({required String sessionId, required String reason});
}

/// A tab whose helper connected: the pairing digits Chrome's popup shows
/// too, and once it says so, the tab's origin and title (the page's own
/// words: never trusted, shown only in quotes).
class BrowserTab {
  BrowserTab({required this.helper, required this.code});

  final int helper;
  final String code;
  String origin = '';
  String title = '';
  int tab = -1;

  /// It said which tab it is.
  bool get introduced => origin.isNotEmpty;
}

/// The session running on this Mac: whose, which tab, its lease.
class BrowserActive {
  BrowserActive({
    required this.intent,
    required this.tab,
    required this.lease,
    required this.agentName,
  });

  final BrowserSession intent;
  final BrowserTab tab;
  final String lease;
  final String agentName;
  var paused = false;
  var lastSequence = -1;

  /// The outstanding command's op ("read", "click"…).
  String op = '';

  /// Its review in the native panel on this Mac: waiting for the person,
  /// or allowed and waiting for them to go back to the tab.
  BrowserReview review = BrowserReview.none;

  /// The command sent to the tab and not yet answered, and when it went.
  int? outstanding;
  DateTime? sentAt;
}

/// Where a reviewed action stands on this Mac.
enum BrowserReview { none, waiting, allowed }

/// One line of what happened, in the app's own words: "Read page",
/// "Scrolled", "Paused: …", "Ended: …".
class BrowserActivity {
  const BrowserActivity(this.at, this.text);

  final DateTime at;
  final String text;
}

/// This Mac's browser control.
class BrowserLocal extends ChangeNotifier {
  BrowserLocal({
    required this.bridge,
    required this.backend,
    DateTime Function()? now,
    String? brokerInstance,
    this.claktory = '',
  }) : _now = now ?? DateTime.now,
       brokerInstance = brokerInstance ?? _random();

  final BrowserBridge bridge;
  final BrowserBackendPort backend;
  final DateTime Function() _now;

  /// The Claktory it serves, by the name this app knows it by, for the
  /// card on this Mac to say whose agent asks.
  String claktory;

  /// Whether Chrome knows where Claktory's helper is; null until asked.
  /// Only the person's own click sets it up ([setUpChrome]).
  bool? chromeSetUp;

  /// This launch's broker, as the backend knows it (BrowserCapability).
  final String brokerInstance;

  /// Tabs whose helpers are connected, by helper id.
  final tabs = <int, BrowserTab>{};

  /// What happened, newest last; only while the app runs.
  final activity = <BrowserActivity>[];

  BrowserActive? active;

  /// Why it couldn't start, or why it last turned something away, in the
  /// person's words.
  String? problem;
  var running = false;
  Timer? _heartbeat;
  Timer? _limit;

  /// Starts listening for Chrome's helpers.
  Future<void> start() async {
    if (running) return;
    bridge.listen(_on);
    try {
      await bridge.start();
      running = true;
      problem = null;
      await sayCapability();
    } on BrowserStartFailure catch (failure) {
      problem = failure.message;
    }
    // Chrome starts the helper only once it knows where it is, which only
    // the person's click on this Mac sets up.
    if (running) chromeSetUp = await bridge.registered();
    notifyListeners();
  }

  /// The person chose to set up Chrome on this Mac: Chrome learns where
  /// Claktory's helper is. [replace] only once they chose to replace
  /// another app's connection. The failure, if any.
  Future<BrowserRegisterFailure?> setUpChrome({bool replace = false}) async {
    BrowserRegisterFailure? failed;
    try {
      await bridge.register(replace: replace);
      chromeSetUp = true;
      problem = null;
    } on BrowserRegisterFailure catch (failure) {
      failed = failure;
      problem = failure.message;
    }
    notifyListeners();
    return failed;
  }

  /// Browser control was turned off here: stops, and Chrome forgets the
  /// helper.
  Future<void> turnOff() async {
    await stop();
    try {
      await bridge.unregister();
    } on BrowserRegisterFailure {
      // Nothing more to do: without the app, the helper answers nobody.
    }
  }

  /// Stops listening: a session running ends here first.
  Future<void> stop() async {
    await _end(
      'stopped',
      tellHelper: true,
      tellBackend: true,
      said: 'browser control stopped on this Mac.',
    );
    tabs.clear();
    if (running) await bridge.stop();
    running = false;
    notifyListeners();
  }

  /// The tabs [intent] can be given: those on its exact origin.
  List<BrowserTab> tabsFor(BrowserSession intent) => [
    for (final tab in tabs.values)
      if (tab.introduced && tab.origin == intent.origin) tab,
  ];

  /// The person's choice on this Mac: [tab] for [intent]. It's the only
  /// thing that binds a tab to a session. Refused (false) when another
  /// session runs here, the tab is on another origin, or it's gone.
  Future<bool> allow(
    BrowserSession intent,
    BrowserTab tab, {
    required String agentName,
  }) async {
    if (active != null || intent.state != 'pending') return false;
    if (!tab.introduced || tab.origin != intent.origin) return false;
    if (!await bridge.choose(tab.helper)) return false;
    final lease = _random();
    final sent = await bridge.send(tab.helper, {
      'type': 'bound',
      'agent': _cut(agentName, 100),
      'until_ms': intent.deadlineMs.toInt(),
      if (intent.purpose.isNotEmpty) 'purpose': _cut(intent.purpose, 500),
    });
    if (!sent) return false;
    active = BrowserActive(
      intent: intent,
      tab: tab,
      lease: lease,
      agentName: agentName,
    );
    await backend.grant(
      sessionId: intent.id,
      brokerInstance: brokerInstance,
      origin: intent.origin,
      lease: lease,
    );
    _heartbeat = Timer.periodic(const Duration(seconds: 5), (_) {
      final now = active;
      if (now != null) {
        unawaited(
          backend.heartbeat(sessionId: now.intent.id, lease: now.lease),
        );
      }
    });
    // The hard limit, by this Mac's clock too.
    final left = intent.deadlineMs.toInt() - _now().millisecondsSinceEpoch;
    if (intent.deadlineMs > 0) {
      _limit = Timer(
        Duration(milliseconds: max(0, left)),
        () =>
            unawaited(_end('time_limit', tellHelper: true, tellBackend: true)),
      );
    }
    _log('Allowed reading ${_host(intent.origin)}');
    notifyListeners();
    return true;
  }

  /// The person declined [intent] here.
  Future<void> decline(BrowserSession intent) =>
      backend.stop(sessionId: intent.id, reason: 'declined');

  /// The person's Stop: it ends here first, then the backend is told.
  Future<void> stopSession() =>
      _end('stopped', tellHelper: true, tellBackend: true);

  /// The backend ended [sessionId] (BrowserEnded).
  Future<void> ended(String sessionId, String reason) async {
    if (active?.intent.id != sessionId) return;
    await _end(reason, tellHelper: true, tellBackend: false);
  }

  /// A command from the backend for the bound tab: relayed only when it's
  /// this session's, with its lease, in order, one at a time.
  Future<void> command(BrowserCommand command) async {
    final now = active;
    Future<void> refuse(String failure) => backend.result(
      sessionId: command.sessionId,
      commandId: command.commandId.toInt(),
      resultJson: '',
      failure: failure,
      macMs: 0,
    );
    if (now == null || now.intent.id != command.sessionId) {
      return refuse('interrupted');
    }
    if (command.lease != now.lease) return refuse('wrong_device');
    if (command.sequence.toInt() <= now.lastSequence ||
        now.outstanding != null) {
      return refuse('interrupted');
    }
    if (now.paused) return refuse('paused');
    final Object? op;
    try {
      op = jsonDecode(command.opJson);
    } on FormatException {
      return refuse('unsupported_page');
    }
    if (op is! Map<String, Object?>) return refuse('unsupported_page');
    now
      ..lastSequence = command.sequence.toInt()
      ..outstanding = command.commandId.toInt()
      ..op = _text(op['op'])
      ..review = BrowserReview.none
      ..sentAt = _now();
    final sent = await bridge.send(now.tab.helper, {
      'id': command.commandId.toInt(),
      // The helper and extension hold each command to this session and a
      // rising sequence too, and bind a review's hash to both.
      'session': command.sessionId,
      'sequence': command.sequence.toInt(),
      'cmd': op,
    });
    if (!sent) {
      now.outstanding = null;
      await refuse('no_helper');
    }
  }

  /// Tells the backend what this Mac can do now (on every new connection,
  /// and whenever a tab's helper comes or the last goes).
  Future<void> sayCapability() => backend.capability(
    helperReady: tabs.isNotEmpty,
    brokerInstance: brokerInstance,
  );

  void _on(BrowserBrokerEvent event) {
    switch (event) {
      case HelperConnected(:final id, :final code):
        final first = tabs.isEmpty;
        tabs[id] = BrowserTab(helper: id, code: code);
        if (first) unawaited(sayCapability());
      case HelperMessage(:final id, :final frame):
        unawaited(_frame(id, frame));
      case HelperGone(:final id, :final code):
        final tab = tabs.remove(id);
        if (tab != null && active?.tab == tab) {
          unawaited(
            _end(
              code == 'closed' ? 'chrome_closed' : 'helper_gone',
              tellHelper: false,
              tellBackend: true,
            ),
          );
        }
        if (tabs.isEmpty) unawaited(sayCapability());
      case HelperRefused():
        problem = browserRefusedLine;
      case HelperDecided(:final id, :final command, :final allow):
        final now = active;
        if (now != null &&
            now.tab.helper == id &&
            now.outstanding == command &&
            now.review == BrowserReview.waiting) {
          now.review = allow ? BrowserReview.allowed : BrowserReview.none;
          _log(allow ? 'You allowed it once' : "You didn't allow it");
        }
    }
    notifyListeners();
  }

  Future<void> _frame(int helper, Map<String, Object?> frame) async {
    final tab = tabs[helper];
    if (tab == null) return;
    final now = active;
    final ours = now != null && now.tab == tab;
    switch (frame['type']) {
      case 'hello':
        // Once: a tab says which it is before it's chosen, never after.
        if (tab.introduced) return;
        tab
          ..origin = _text(frame['origin'])
          ..title = _text(frame['title'])
          ..tab = switch (frame['tab']) {
            final int tab => tab,
            _ => -1,
          };
      case 'review' when ours:
        // The panel shows it (Pesci's, native): the card only says so.
        if (frame['id'] != now.outstanding) return;
        now.review = BrowserReview.waiting;
      case 'result' when ours:
        final id = frame['id'];
        if (id != now.outstanding) return;
        final op = now.op;
        now.review = BrowserReview.none;
        final result = frame['result'];
        final failure =
            result is Map<String, Object?> && result['error'] is String
            ? result['error']! as String
            : '';
        final sentAt = now.sentAt ?? _now();
        now
          ..outstanding = null
          ..sentAt = null;
        await backend.result(
          sessionId: now.intent.id,
          commandId: id! as int,
          resultJson: jsonEncode(result),
          failure: failure,
          macMs: _now().difference(sentAt).inMilliseconds,
        );
        if (failure.isEmpty) {
          _log(switch (op) {
            'scroll' => 'Scrolled',
            'click' => 'Clicked',
            'fill' => 'Typed',
            'navigate' => 'Opened a page',
            _ => 'Read page',
          });
        }
      case 'paused' when ours:
        now.paused = true;
        _log('Paused: ${browserPauseReason(_text(frame['reason']))}');
      case 'resumed' when ours:
        now.paused = false;
        _log('Resumed');
      case 'ended' when ours:
        final reason = _text(frame['reason']);
        await _end(
          _extensionEnds.contains(reason) ? reason : 'interrupted',
          tellHelper: false,
          tellBackend: true,
        );
    }
    notifyListeners();
  }

  /// Ends the running session here first: the tab, the timers, what it
  /// was waiting for; then tells the backend.
  Future<void> _end(
    String reason, {
    required bool tellHelper,
    required bool tellBackend,
    String? said,
  }) async {
    final now = active;
    if (now == null) return;
    active = null;
    _heartbeat?.cancel();
    _limit?.cancel();
    _heartbeat = null;
    _limit = null;
    if (tellHelper) await bridge.end(now.tab.helper, 'stop');
    tabs.remove(now.tab.helper);
    _log('Ended: ${said ?? browserEndReason(reason, agent: now.agentName)}');
    notifyListeners();
    if (tellBackend) {
      await backend.stop(sessionId: now.intent.id, reason: reason);
    }
  }

  void _log(String text) {
    activity.add(BrowserActivity(_now(), text));
    if (activity.length > 50) activity.removeAt(0);
  }

  @override
  void dispose() {
    _heartbeat?.cancel();
    _limit?.cancel();
    super.dispose();
  }
}

/// Why a session paused, in the app's words.
String browserPauseReason(String reason) => switch (reason) {
  'tab_inactive' ||
  'window_blurred' => 'the tab isn’t in front. Go back to it.',
  'popup_opened' => 'the page opened another tab or window.',
  'dialog_open' => 'the page stopped answering. It may be showing a dialog.',
  _ => 'Chrome paused it.',
};

/// What the extension may say ended a session, passed on as it is;
/// anything else is passed on as interrupted.
const _extensionEnds = {
  'stopped',
  'origin_changed',
  'tab_closed',
  'chrome_closed',
};

/// Why a session ended, after "Ended: ", in the app's words: the
/// backend's reason words (BrowserSession.reason), and the broker's codes.
String browserEndReason(String reason, {String agent = 'The agent'}) =>
    switch (reason) {
      'stopped' || 'stop' => 'you stopped it.',
      'declined' => 'you declined it.',
      'expired' => "you didn't choose a tab in time.",
      'time_limit' => 'it reached its 15 minutes.',
      'idle' => 'nothing happened for 5 minutes.',
      'command_limit' => 'it reached its 100 actions.',
      'data_limit' => "it read as much as it's allowed to.",
      'interrupted' => 'the connection was interrupted.',
      'origin_changed' => 'the tab left the site.',
      'tab_closed' => 'the tab closed.',
      'chrome_closed' => 'Chrome closed.',
      'helper_gone' => "Chrome's connection to Claktory ended.",
      'app_quit' => 'Claktory quit.',
      'agent_changed' => "$agent's settings changed.",
      'ended' => 'the agent finished.',
      _ => browserGoneReason(reason),
    };

String _text(Object? value) => value is String ? value : '';

String _cut(String text, int most) =>
    text.length <= most ? text : text.substring(0, most);

/// "example.com" from "https://example.com".
String browserHost(String origin) => _host(origin);

String _host(String origin) => Uri.tryParse(origin)?.host.isNotEmpty == true
    ? Uri.parse(origin).host
    : origin;

final _secure = Random.secure();

String _random() => base64Url
    .encode([for (var i = 0; i < 16; i++) _secure.nextInt(256)])
    .replaceAll('=', '');
