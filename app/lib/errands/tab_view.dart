import 'dart:async';
import 'dart:ui' as ui;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import '../backend.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'live_input.dart';
import 'live_view.dart';

// The person's live views of an agent's browser (WatchAgentTab): watching
// what it shows, or the one tab a check request hands them, with their
// input. A connection watches one view at a time, so the view opened last
// watches, and when it closes the one under it watches again.

/// One live view: [agentId]'s browser, or with [requestSeq] the tab of its
/// check request at that seq.
class AgentTabView extends ChangeNotifier {
  AgentTabView._(
    this._views, {
    required this.agentId,
    required this.requestSeq,
    required this.maxWidth,
    Future<ui.Image> Function(Uint8List bytes)? decode,
  }) {
    frames = decode == null
        ? LiveFrames(ack: _ack)
        : LiveFrames(ack: _ack, decode: decode);
  }

  final AgentTabViews _views;
  final String agentId;

  /// The check request's seq; 0 for the watch-only view.
  final int requestSeq;
  final int maxWidth;
  late final LiveFrames frames;

  /// Whether the person's input goes to the page: a check's tab only.
  bool get takesInput => requestSeq != 0;

  /// The tab's site, in Claktory's words, from the latest frame.
  String get site => _site;
  String _site = '';

  /// How the view ended on Claktory's side; null while it goes on.
  AgentTabViewEnded? get ended => _ended;
  AgentTabViewEnded? _ended;

  /// Why it couldn't be watched, in Claktory's words; null after a watch
  /// that worked.
  String? get error => _error;
  String? _error;

  /// Claktory's line on why the person's last input did nothing ("Only
  /// the check takes your clicks here."), for [noteFor] after it came;
  /// null otherwise.
  String? get note => _note;
  String? _note;
  Timer? _noteTimer;

  /// How long a note shows.
  static const noteFor = Duration(seconds: 3);

  /// This connection watches it now.
  bool get watching => _watching;
  var _watching = false;

  var _closed = false;

  bool _for(String agent, Int64 seq) =>
      agent == agentId && seq.toInt() == requestSeq;

  void _watch(bool on) {
    if (_closed && on) return;
    if (_watching == on) return;
    _watching = on;
    if (!on) frames.clear();
    notifyListeners();
    unawaited(
      _views
          .send(
            ClientMessage(
              watchAgentTab: WatchAgentTab(
                agentId: agentId,
                requestSeq: Int64(requestSeq),
                on: on,
                maxWidth: on ? maxWidth : 0,
              ),
            ),
          )
          .then(
            (_) {
              if (!on || _error == null) return;
              _error = null;
              notifyListeners();
            },
            onError: (Object failure) {
              if (!on) return;
              _watching = false;
              _error = failure is RequestFailure ? failure.message : '$failure';
              notifyListeners();
            },
          ),
    );
  }

  /// It no longer watches here, Claktory having ended it quietly (another
  /// view watching, the connection gone).
  void _stopped() {
    _watching = false;
    frames.clear();
    notifyListeners();
  }

  /// Watches it here again: after it moved to another device, or failed.
  void again() {
    _ended = null;
    _error = null;
    _watching = false;
    _views._activate(this);
  }

  void _receive(ServerMessage message) {
    switch (message.whichBody()) {
      case ServerMessage_Body.agentTabFrame:
        final frame = message.agentTabFrame;
        if (!_watching || !_for(frame.agentId, frame.requestSeq)) return;
        if (frame.site != _site) {
          _site = frame.site;
          notifyListeners();
        }
        unawaited(
          frames.take(
            frame.frameSeq.toInt(),
            Uint8List.fromList(frame.jpeg),
            origin: frame.origin,
            title: frame.title,
          ),
        );
      case ServerMessage_Body.agentTabNote:
        final note = message.agentTabNote;
        if (!_watching || !_for(note.agentId, note.requestSeq)) return;
        _note = note.message;
        _noteTimer?.cancel();
        _noteTimer = Timer(noteFor, () {
          _note = null;
          notifyListeners();
        });
        notifyListeners();
      case ServerMessage_Body.agentTabViewEnded:
        final end = message.agentTabViewEnded;
        if (!_for(end.agentId, end.requestSeq)) return;
        _ended = end;
        _watching = false;
        // The last frame was still on the site: it stays, dimmed, under
        // the words saying the tab left it.
        if (end.reason != AgentTabViewEnd.AGENT_TAB_VIEW_END_LEFT_SITE) {
          frames.clear();
        }
        notifyListeners();
      default:
        break;
    }
  }

  void _ack(int seq) {
    if (!_watching) return;
    unawaited(
      _views
          .send(
            ClientMessage(
              agentTabFrameAck: AgentTabFrameAck(
                agentId: agentId,
                requestSeq: Int64(requestSeq),
                frameSeq: Int64(seq),
              ),
            ),
          )
          .catchError((Object _) {}),
    );
  }

  /// The person's [events], aimed at the frame shown: a check's tab only,
  /// while it's watched here.
  void input(List<LiveEvent> events) {
    final seq = frames.frame?.seq;
    // The secondary button opens a menu the page draws as it likes:
    // Claktory takes none.
    events = [
      for (final event in events)
        if (event is! LivePointer || event.button != LiveButton.right) event,
    ];
    if (!takesInput || !_watching || seq == null || events.isEmpty) return;
    unawaited(
      _views
          .send(
            ClientMessage(
              agentTabInput: AgentTabInput(
                agentId: agentId,
                requestSeq: Int64(requestSeq),
                events: [
                  for (final event in events) agentBrowserEvent(event, seq),
                ],
              ),
            ),
          )
          .catchError((Object _) {}),
    );
  }

  /// Closes it here: the view under it, if any, watches again.
  void close() {
    if (_closed) return;
    _closed = true;
    _views._remove(this);
  }

  @override
  void dispose() {
    _noteTimer?.cancel();
    close();
    frames.dispose();
    super.dispose();
  }
}

/// [event] as the backend takes it, aimed at frame [seq].
AgentBrowserEvent agentBrowserEvent(LiveEvent event, int seq) =>
    switch (event) {
      LivePointer(:final kind, :final x, :final y, :final button) =>
        AgentBrowserEvent(
          pointer: AgentBrowserPointer(
            kind: switch (kind) {
              LivePointerKind.down =>
                AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_DOWN,
              LivePointerKind.move =>
                AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_MOVE,
              LivePointerKind.up =>
                AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_UP,
            },
            x: x,
            y: y,
            button: kind == LivePointerKind.move && button == LiveButton.left
                ? 0
                : switch (button) {
                    LiveButton.left => 1,
                    LiveButton.middle => 2,
                    LiveButton.right => 3,
                  },
            frameSeq: Int64(seq),
          ),
        ),
      LiveWheel(:final dx, :final dy, :final x, :final y) => AgentBrowserEvent(
        wheel: AgentBrowserWheel(
          x: x,
          y: y,
          dx: dx,
          dy: dy,
          frameSeq: Int64(seq),
        ),
      ),
      LiveKey(
        :final key,
        :final down,
        :final shift,
        :final control,
        :final alt,
        :final meta,
      ) =>
        AgentBrowserEvent(
          key: AgentBrowserKey(
            down: down,
            key: key,
            modifiers:
                (shift ? 1 : 0) |
                (control ? 2 : 0) |
                (alt ? 4 : 0) |
                (meta ? 8 : 0),
          ),
        ),
      LiveTyped(:final text) => AgentBrowserEvent(text: text),
    };

/// This device's live views of agents' browsers, for one Claktory: the
/// one opened last watches.
class AgentTabViews {
  AgentTabViews({
    required this.send,
    required Stream<ServerMessage> messages,
    required this.connected,
    this.decode,
  }) {
    _messages = messages.listen(_receive);
    connected.addListener(_reconnected);
  }

  /// The one for [backend], made the first time it's asked for.
  factory AgentTabViews.of(Backend backend) => _all[backend] ??= AgentTabViews(
    send: backend.sendAgentBrowser,
    messages: backend.agentTabMessages,
    connected: backend.connected,
  );

  static final _all = Expando<AgentTabViews>();

  /// Uses [views] for [backend] (tests).
  @visibleForTesting
  static void attach(Backend backend, AgentTabViews views) =>
      _all[backend] = views;

  final Future<void> Function(ClientMessage message) send;
  final ValueListenable<bool> connected;
  final Future<ui.Image> Function(Uint8List bytes)? decode;
  late final StreamSubscription<ServerMessage> _messages;

  /// Open views, the one watching last.
  final _open = <AgentTabView>[];

  /// Opens a view of [agentId]'s browser, or of the tab of its check
  /// request at [requestSeq], at most [maxWidth] pixels wide; it watches at
  /// once. Close it with [AgentTabView.close].
  AgentTabView open(String agentId, {int requestSeq = 0, int maxWidth = 0}) {
    final view = AgentTabView._(
      this,
      agentId: agentId,
      requestSeq: requestSeq,
      maxWidth: maxWidth,
      decode: decode,
    );
    _activate(view);
    return view;
  }

  void _activate(AgentTabView view) {
    _open.remove(view);
    for (final other in _open) {
      // Watching another ends the one before on Claktory's side, quietly.
      if (other._watching) other._stopped();
    }
    _open.add(view);
    if (connected.value) view._watch(true);
  }

  void _remove(AgentTabView view) {
    final was = _open.isNotEmpty && identical(_open.last, view);
    _open.remove(view);
    if (!was) return;
    final under = _open.lastOrNull;
    if (under != null && under._ended == null && connected.value) {
      under._watch(true);
    } else if (view._watching) {
      view._watch(false);
    }
  }

  void _reconnected() {
    final top = _open.lastOrNull;
    if (top == null) return;
    if (!connected.value) {
      if (top._watching) top._stopped();
      return;
    }
    if (top._ended == null) top._watch(true);
  }

  void _receive(ServerMessage message) {
    for (final view in List.of(_open)) {
      view._receive(message);
    }
  }

  void dispose() {
    unawaited(_messages.cancel());
    connected.removeListener(_reconnected);
  }
}
