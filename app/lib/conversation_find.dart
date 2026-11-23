// Find in a conversation (⌘F in an agent's or a chat's conversation, or a
// pipe's timeline): Claktory searches the whole conversation, not only
// what the app holds, and answers the entries a query is in, newest first,
// a page at a time (SearchConversation). In a pipe's timeline the liaison's
// conversation and the pipe's own messages are searched both, and their
// matches merged by time. The messages still queued at its end, shown but
// not yet in it, are matched here, the newest of all.
import 'dart:async';

import 'package:flutter/foundation.dart';

import 'backend.dart';
import 'find.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// A match gone to: an entry of the conversation by its seq, or, with
/// [pipe], one of the pipe's messages; or, with [queued], a message
/// still queued at the conversation's end (that key of [QueuedFind]'s).
typedef FoundAt = ({bool pipe, int seq, Object? queued});

/// The messages queued at a conversation's end, top to bottom: each one's
/// key and the words it shows.
typedef QueuedFind = List<(Object key, String text)> Function();

/// What the conversation marks: [query]'s occurrences, and the match gone
/// to ([at]), if any.
typedef FindMarks = ({String query, FoundAt? at});

/// The search a conversation's find makes: a page of matches older than
/// `beforeSeq` (the newest without), in the conversation or, with [pipe],
/// the pipe's messages.
typedef MatchSearch = Future<ConversationMatches> Function(
  String query, {
  required bool pipe,
  int? beforeSeq,
});

/// One log searched: the conversation, or a pipe's messages.
class _Log {
  _Log(this.pipe);

  final bool pipe;

  /// Loaded so far, newest first.
  final matches = <ConversationMatch>[];

  /// Older ones remain.
  var more = false;

  /// Occurrences in the whole log, from its first page.
  var total = 0;
  var capped = false;

  /// The time of the oldest loaded: a match of another log older than it
  /// can't be placed until this one has loaded past it.
  int get horizon => matches.isEmpty ? 0 : matches.last.timestampMs.toInt();
}

/// An occurrence of the query: where it is, the [nth] of the [hits] there
/// counted from its end (the next one goes up the screen).
typedef _Occurrence = ({FoundAt at, int nth, int hits});

/// How many times [query] is in [text], as Claktory counts: matched
/// case-insensitively, one after another.
int _hits(String text, String query) {
  final lower = text.toLowerCase();
  final needle = query.toLowerCase();
  if (needle.isEmpty) return 0;
  var hits = 0;
  for (var at = lower.indexOf(needle); at >= 0;) {
    hits++;
    at = lower.indexOf(needle, at + needle.length);
  }
  return hits;
}

class ConversationFind extends FindTarget {
  ConversationFind({
    required MatchSearch search,
    this.pipe = false,
    this.conversation = true,
    this.queued,
    required this.go,
  }) : _searchLog = search;

  /// The messages queued at the conversation's end, matched here as they
  /// show; none when null.
  final QueuedFind? queued;

  /// Whether a pipe's messages are searched too.
  final bool pipe;

  /// Whether a conversation is searched: not for a pipe with no liaison,
  /// whose messages are all there is.
  final bool conversation;

  final MatchSearch _searchLog;

  /// Brings the match to view: loads back to it if it's older than what's
  /// held, and puts it in sight: its [nth] hit, counted from the entry's
  /// end, of the [hits] Claktory found in it, where the entry shows as
  /// many.
  final void Function(FoundAt at, int nth, int hits) go;

  /// What the conversation marks.
  ValueListenable<FindMarks> get marks => _marks;
  final _marks = ValueNotifier<FindMarks>((query: '', at: null));

  /// A search's answers that come after another search started are
  /// dropped.
  var _generation = 0;
  var _query = '';
  var _logs = <_Log>[];

  /// The queued messages' occurrences as the search found them, newest
  /// first: above every log's.
  var _queued = <_Occurrence>[];
  var _occurrences = <_Occurrence>[];
  var _current = -1;
  var _searching = false;
  var _searched = false;
  String? _failed;

  @override
  String get placeholder => 'Search this conversation';

  @override
  bool get nextIsUp => true;

  @override
  int get count {
    final total = _logs.fold(_queued.length, (sum, log) => sum + log.total);
    // Past a capped count, as many as have been found.
    return total < _occurrences.length ? _occurrences.length : total;
  }

  @override
  bool get capped => _logs.any((log) => log.capped);

  @override
  int get current => _current;

  @override
  bool get searching => _searching;

  @override
  bool get searched => _searched;

  @override
  String? get failed => _failed;

  @override
  void search(String query) {
    final generation = ++_generation;
    _query = query;
    _logs = [if (conversation) _Log(false), if (pipe) _Log(true)];
    _queued = [
      for (final (key, text) in (queued?.call() ?? const []).reversed)
        if (_hits(text, query) case final hits when hits > 0)
          for (var nth = 0; nth < hits; nth++)
            (at: (pipe: false, seq: -1, queued: key), nth: nth, hits: hits),
    ];
    _occurrences = [];
    _current = -1;
    _failed = null;
    if (query.isEmpty) {
      _searching = false;
      _searched = false;
      _marks.value = (query: '', at: null);
      notifyListeners();
      return;
    }
    _searching = true;
    _searched = false;
    notifyListeners();
    unawaited(() async {
      final loaded = await _load(_logs, generation);
      if (!loaded) return;
      _searched = true;
      _marks.value = (query: query, at: null);
      if (_occurrences.isNotEmpty) {
        _current = 0;
        _go();
      }
      notifyListeners();
    }());
  }

  @override
  void step(int by) {
    if (_searching || _occurrences.isEmpty && !_more) return;
    final generation = _generation;
    if (by > 0) {
      if (_current + 1 < _occurrences.length) {
        _current++;
        _go();
        notifyListeners();
        return;
      }
      if (!_more) {
        // Past the oldest: back to the newest.
        _current = 0;
        _go();
        notifyListeners();
        return;
      }
      _searching = true;
      notifyListeners();
      unawaited(() async {
        if (!await _load(_bottleneck(), generation)) return;
        if (_current + 1 < _occurrences.length) _current++;
        _go();
        notifyListeners();
      }());
      return;
    }
    if (_current > 0) {
      _current--;
      _go();
      notifyListeners();
      return;
    }
    if (!_more) {
      // Before the newest: round to the oldest.
      _current = _occurrences.length - 1;
      _go();
      notifyListeners();
      return;
    }
    _searching = true;
    notifyListeners();
    unawaited(() async {
      while (_more) {
        if (!await _load(_bottleneck(), generation)) return;
      }
      _current = _occurrences.length - 1;
      _go();
      notifyListeners();
    }());
  }

  @override
  void clear() {
    _generation++;
    _query = '';
    _logs = [];
    _queued = [];
    _occurrences = [];
    _current = -1;
    _searching = false;
    _searched = false;
    _failed = null;
    _marks.value = (query: '', at: null);
    notifyListeners();
  }

  @override
  void dispose() {
    _generation++;
    _marks.dispose();
    super.dispose();
  }

  bool get _more => _logs.any((log) => log.more);

  /// The logs to page on to place the next older match: the one whose
  /// oldest loaded is newest, holding the others back.
  List<_Log> _bottleneck() {
    _Log? next;
    for (final log in _logs) {
      if (!log.more) continue;
      if (next == null || log.horizon > next.horizon) next = log;
    }
    return [?next];
  }

  /// Loads the next page of each of [logs] (the first, when none is
  /// loaded) and places what's found. False when another search started
  /// meanwhile, or it failed (saying why).
  Future<bool> _load(List<_Log> logs, int generation) async {
    try {
      final pages = await Future.wait([
        for (final log in logs)
          _searchLog(
            _query,
            pipe: log.pipe,
            beforeSeq: log.matches.isEmpty
                ? null
                : log.matches.last.seq.toInt(),
          ),
      ]);
      if (generation != _generation) return false;
      for (final (i, page) in pages.indexed) {
        final log = logs[i];
        if (log.matches.isEmpty) {
          log.total = page.total;
          log.capped = page.totalCapped;
        }
        log.matches.addAll(page.matches);
        log.more = page.more && page.matches.isNotEmpty;
      }
      _place();
      _searching = false;
      return true;
    } on RequestFailure catch (failure) {
      if (generation != _generation) return false;
      _searching = false;
      // A Claktory from before search knows no such request: what to do,
      // not its word for it.
      _failed = failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE
          ? 'Update Claktory to search'
          : failure.message;
      notifyListeners();
      return false;
    }
  }

  /// The occurrences, newest first, as far as every log's loaded matches
  /// can be put in order: down to the newest of the logs' oldest loaded,
  /// among those with more to load.
  void _place() {
    var floor = 0;
    for (final log in _logs) {
      if (log.more && log.horizon > floor) floor = log.horizon;
    }
    final found = [
      for (final log in _logs)
        for (final match in log.matches)
          if (match.timestampMs.toInt() >= floor) (log: log, match: match),
    ];
    // In one log its own order (newest first); across two, by time.
    found.sort((a, b) {
      final byTime = b.match.timestampMs.compareTo(a.match.timestampMs);
      if (byTime != 0) return byTime;
      if (a.log == b.log) return b.match.seq.compareTo(a.match.seq);
      return a.log.pipe ? -1 : 1;
    });
    _occurrences = [
      ..._queued,
      for (final (:log, :match) in found)
        for (var nth = 0; nth < (match.hits < 1 ? 1 : match.hits); nth++)
          (
            at: (pipe: log.pipe, seq: match.seq.toInt(), queued: null),
            nth: nth,
            hits: match.hits,
          ),
    ];
  }

  /// Goes to the current occurrence, and marks its match.
  void _go() {
    if (_current < 0 || _current >= _occurrences.length) return;
    final occurrence = _occurrences[_current];
    final at = occurrence.at;
    // Another occurrence in the same entry is marked already, but may be
    // out of sight in a long one.
    if (_marks.value.at != at || _marks.value.query != _query) {
      _marks.value = (query: _query, at: at);
    }
    go(at, occurrence.nth, occurrence.hits);
  }
}
