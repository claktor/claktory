import 'dart:async';

import 'package:flutter/foundation.dart';

import 'device_account.dart' show AccountClaktory;

/// What the person's Claktory account says of one of the app's Claktories.
enum AccountCheck {
  /// On the account, or this device's own.
  theirs,

  /// Not on it (or reached by a direct code, which no account lists).
  notTheirs,

  /// This device is signed out of the account: nothing can be checked.
  signedOut,

  /// Signed in, but the account's list couldn't be read yet.
  unreadable;

  /// Whether it's marked: anything but [theirs].
  bool get marked => this != theirs;

  /// Whether it's marked in `danger`: known not to be theirs, or signed in
  /// and still unchecked. Signed out is marked neutrally (Fugo): on a
  /// device never signed in, red on every Claktory would tell nothing.
  bool get alarming => this == notTheirs || this == unreadable;
}

/// Which of the app's Claktories are on the person's Claktory account, by
/// the endpoint the app dials each by (never by what one says of itself),
/// as browser control and the pairing confirmation match them: so one that
/// isn't stays marked as such, and can't later pass as one of theirs
/// (Fugo's pairing condition 4).
class AccountMatch extends ChangeNotifier {
  AccountMatch({
    required this.remotes,
    required Listenable changes,
    required Listenable account,
    required this.theirs,
    this.whose = _nobody,
    DateTime Function()? now,
  }) : _changes = changes,
       _account = account,
       _now = now ?? DateTime.now {
    changes.addListener(_changed);
    account.addListener(_accountChanged);
    unawaited(relist());
  }

  /// How long after a list that couldn't be read it's tried again.
  static const retryAfter = Duration(minutes: 1);

  /// How long reads of a list never read may fail before the Claktories
  /// are marked as unchecked: just launched, the network may still be
  /// coming up, which isn't an account that can't be read (#388). Until
  /// then nothing is marked, as while the first read is under way.
  static const unsureFor = Duration(seconds: 30);

  /// How soon a read that failed is tried again meanwhile, doubling each
  /// time: 2, 4, 8 and 16 seconds, the last at [unsureFor].
  static const firstRetry = Duration(seconds: 2);

  /// The endpoints the app dials its other Claktories by.
  final Iterable<String> Function() remotes;
  final Listenable _changes;
  final Listenable _account;

  /// The Claktories on the person's account; null while signed out.
  final Future<List<AccountClaktory>>? Function() theirs;

  /// Which account [theirs] reads (its service and email): a list read
  /// for one never stands in for another's.
  final String Function() whose;

  static String _nobody() => '';

  final DateTime Function() _now;

  /// When reads of a list never read began to fail; null while they don't.
  DateTime? _failingSince;
  var _nextRetry = firstRetry;

  /// Whose account [_listed] was read from.
  var _listedWhose = '';

  /// The account's Claktories' endpoints (lowercase); null until the list
  /// is first answered.
  Set<String>? _listed;

  /// When each of the account's Claktories last checked in with it, by
  /// endpoint (lowercase), as the list last read said.
  var _checkedIn = const <String, DateTime>{};

  /// Signed out when the list was last asked for.
  var _signedOut = false;

  /// Signed in, but no list was ever read.
  var _unreadable = false;

  /// The remote endpoints open when the list was last read.
  var _listedFor = const <String>{};
  var _listing = false;
  var _again = false;
  var _disposed = false;

  /// Tries a list that couldn't be read again.
  Timer? _retry;

  /// What the account says of the Claktory dialled by [endpointId]: this
  /// device's own (null) is always theirs; one without an endpoint (a
  /// direct code) never is. A list read once stands while it can't be read
  /// again. Null until the list is first answered.
  AccountCheck? check(String? endpointId) {
    if (endpointId == null) return AccountCheck.theirs;
    final listed = _listed;
    if (listed == null) return null;
    if (_signedOut) return AccountCheck.signedOut;
    if (_unreadable) return AccountCheck.unreadable;
    return endpointId.isNotEmpty && listed.contains(endpointId.toLowerCase())
        ? AccountCheck.theirs
        : AccountCheck.notTheirs;
  }

  /// When the Claktory dialled by [endpointId] last checked in with the
  /// person's account, as its list last said; null when it isn't on it,
  /// never did, or nothing is known (friend bug #37).
  DateTime? checkedIn(String? endpointId) {
    if (endpointId == null || endpointId.isEmpty) return null;
    if (check(endpointId) != AccountCheck.theirs) return null;
    return _checkedIn[endpointId.toLowerCase()];
  }

  /// Reads the account's list again: when the account changes, a Claktory
  /// is added, the app comes back to the front, and a minute after it
  /// couldn't be read.
  Future<void> relist() async {
    if (_disposed) return;
    if (_listing) {
      _again = true;
      return;
    }
    _listing = true;
    try {
      do {
        _again = false;
        await _listOnce();
      } while (_again && !_disposed);
    } finally {
      _listing = false;
    }
  }

  Future<void> _listOnce() async {
    _retry?.cancel();
    _listedFor = _remotes();
    final who = whose();
    final listed = theirs();
    var next = const <String>{};
    var checkedIn = const <String, DateTime>{};
    var unreadable = false;
    // Failing, but not for long enough to say so: nothing known yet.
    var unsure = false;
    if (listed != null) {
      try {
        final claktories = await listed;
        next = {
          for (final claktory in claktories)
            if (claktory.endpointId.isNotEmpty)
              claktory.endpointId.toLowerCase(),
        };
        checkedIn = {
          for (final claktory in claktories)
            if (claktory.endpointId.isNotEmpty && claktory.lastContact != null)
              claktory.endpointId.toLowerCase(): claktory.lastContact!,
        };
        _failingSince = null;
        _nextRetry = firstRetry;
      } catch (_) {
        // The last list read stands, for the same account; with none,
        // nothing can be checked, once that's lasted [unsureFor].
        final last = _listed;
        final none =
            last == null || _signedOut || _unreadable || _listedWhose != who;
        var wait = retryAfter;
        if (!none) {
          next = last;
          checkedIn = _checkedIn;
        } else {
          final since = _failingSince ??= _now();
          final left = unsureFor - _now().difference(since);
          if (left > Duration.zero) {
            unsure = true;
            wait = _nextRetry < left ? _nextRetry : left;
            _nextRetry *= 2;
          } else {
            unreadable = true;
          }
        }
        if (!_disposed) _retry = Timer(wait, () => unawaited(relist()));
      }
    } else {
      _failingSince = null;
      _nextRetry = firstRetry;
    }
    if (_disposed) return;
    if (unsure) {
      // As before the first answer: nothing marked.
      final changed = _listed != null;
      _listed = null;
      _checkedIn = const {};
      _signedOut = false;
      _unreadable = false;
      if (changed) notifyListeners();
      return;
    }
    final before = _listed;
    final changed =
        before == null ||
        !setEquals(before, next) ||
        _signedOut != (listed == null) ||
        _unreadable != unreadable ||
        !mapEquals(_checkedIn, checkedIn);
    _listed = next;
    _checkedIn = checkedIn;
    if (!unreadable) _listedWhose = who;
    _signedOut = listed == null;
    _unreadable = unreadable;
    if (changed) notifyListeners();
  }

  Set<String> _remotes() => {
    for (final endpoint in remotes())
      if (endpoint.isNotEmpty) endpoint.toLowerCase(),
  };

  void _accountChanged() => unawaited(relist());

  void _changed() {
    // A Claktory paired since the list was read: read it again.
    if (!_remotes().every(_listedFor.contains)) unawaited(relist());
  }

  @override
  void dispose() {
    _disposed = true;
    _retry?.cancel();
    _changes.removeListener(_changed);
    _account.removeListener(_accountChanged);
    super.dispose();
  }
}
