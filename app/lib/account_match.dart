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
  }) : _changes = changes,
       _account = account {
    changes.addListener(_changed);
    account.addListener(_accountChanged);
    unawaited(relist());
  }

  /// How long after a list that couldn't be read it's tried again.
  static const retryAfter = Duration(minutes: 1);

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

  /// Whose account [_listed] was read from.
  var _listedWhose = '';

  /// The account's Claktories' endpoints (lowercase); null until the list
  /// is first answered.
  Set<String>? _listed;

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
    var unreadable = false;
    if (listed != null) {
      try {
        next = {
          for (final claktory in await listed)
            if (claktory.endpointId.isNotEmpty)
              claktory.endpointId.toLowerCase(),
        };
      } catch (_) {
        // The last list read stands, for the same account; with none,
        // nothing can be checked.
        final last = _listed;
        unreadable =
            last == null || _signedOut || _unreadable || _listedWhose != who;
        if (!unreadable) next = last;
        if (!_disposed) _retry = Timer(retryAfter, () => unawaited(relist()));
      }
    }
    if (_disposed) return;
    final before = _listed;
    final changed =
        before == null ||
        !setEquals(before, next) ||
        _signedOut != (listed == null) ||
        _unreadable != unreadable;
    _listed = next;
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
