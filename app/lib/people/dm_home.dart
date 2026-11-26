import 'dart:async';
import 'dart:collection';
import 'dart:convert';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import '../protocol/factory/v1/factory.pb.dart';

// #451: the person's direct messages go through their own always-on
// Claktory, their home (dms-no-server-storage.md), not claktory.com, which
// keeps only MLS's control plane. This device hands its MLS messages to
// the home (DmSend) and follows its groups there (DmFollow); the home
// sends them on to friends' homes and serves the person's devices, in one
// order per group, numbered from [dmHomeSeqBase] up so they always come
// after anything the worker numbered before.

/// Every seq the home gives is past this one: a group's home numbering
/// starts at 2^32 + 1, past any seq the worker gave (Mista, 32426). A
/// position at or under it is the worker's, so the home starts that group
/// from this device's join.
const dmHomeSeqBase = 1 << 32;

/// The person's home Claktory, as this device reaches it: the Claktory the
/// app shows (Mista's (c) for v1: another one's NOT_HOME means "Waiting for
/// your Claktory"). Tests give their own.
abstract class DmHomeLink {
  /// Whether the app is connected to it now.
  ValueListenable<bool> get connected;

  /// DmFeed as it comes: DmFollow's pages, then each new entry and
  /// delivery; DmSend's answers too.
  Stream<DmFeed> get feeds;

  /// DmHome as it changes.
  Stream<DmHome> get homes;

  Future<DmReceipt> send(DmSend send);
  Future<void> follow(DmFollow follow);
  Future<void> ack(DmAck ack);
  Future<DmHome> routes(DmRoutes routes);
}

/// An envelope as the store takes it (`{"format": 2, "bytes": …}`), from
/// the MLS message's bytes the home carries.
String dmEnvelopeOf(List<int> bytes) =>
    jsonEncode({'format': 2, 'bytes': _b64(bytes)});

/// The MLS message's bytes in [envelope], for the home.
Uint8List dmBytesOf(String envelope) {
  final json = jsonDecode(envelope);
  if (json is! Map || json['bytes'] is! String) {
    throw const FormatException('an envelope is {"format": 2, "bytes": …}');
  }
  return _unb64(json['bytes'] as String);
}

String _b64(List<int> bytes) => base64Url.encode(bytes).replaceAll('=', '');

Uint8List _unb64(String text) =>
    base64Url.decode(text.padRight((text.length + 3) ~/ 4 * 4, '='));

/// One entry of a group's log at the home, as the store takes it.
@immutable
class DmHomeEntry {
  const DmHomeEntry({
    required this.seq,
    required this.envelope,
    required this.commit,
    this.epoch = 0,
    this.acceptedMs = 0,
  });

  final int seq;

  /// As the store takes it ([dmEnvelopeOf]).
  final String envelope;
  final bool commit;
  final int epoch;
  final int acceptedMs;
}

/// What this device has from its home while it follows: each group's
/// entries not taken in yet, in order, what the home says of each group,
/// and how the person's own messages are getting on. Kept in memory only:
/// a new DmFollow (after the connection came back, say) starts it again
/// from where the store got to.
class DmHomeFeed {
  /// Entries not taken in yet, by group (base64url), by seq.
  final _entries = <String, SplayTreeMap<int, DmHomeEntry>>{};

  /// Each group as the home last said: its last seq, epoch, and whether it
  /// let go of entries past this device's cursor.
  final groups = <String, DmGroupState>{};

  /// The person's own messages' deliveries, by client message id: the
  /// latest the home said.
  final deliveries = <String, DmDelivery>{};

  /// Waiting for an entry of a group to come.
  final _waiters = <String, List<(int, Completer<void>)>>{};

  /// Starts again: a new DmFollow is on its way.
  void reset() {
    _entries.clear();
    groups.clear();
    for (final waiting in _waiters.values) {
      for (final (_, done) in waiting) {
        if (!done.isCompleted) done.complete();
      }
    }
    _waiters.clear();
  }

  /// Takes [feed] in: the groups it moved (base64url), and whether it said
  /// anything about deliveries.
  ({Set<String> moved, bool delivered}) take(DmFeed feed) {
    final moved = <String>{};
    for (final entry in feed.entries) {
      final group = _b64(entry.groupId);
      final seq = entry.seq.toInt();
      (_entries[group] ??= SplayTreeMap())[seq] = DmHomeEntry(
        seq: seq,
        envelope: dmEnvelopeOf(entry.envelope),
        commit: entry.kind == DmEntryKind.DM_ENTRY_KIND_COMMIT,
        epoch: entry.epoch.toInt(),
        acceptedMs: entry.acceptedMs.toInt(),
      );
      moved.add(group);
      _woken(group, seq);
    }
    for (final state in feed.groups) {
      final group = _b64(state.groupId);
      final before = groups[group];
      groups[group] = state;
      if (before == null ||
          before.lastSeq != state.lastSeq ||
          before.behind != state.behind) {
        moved.add(group);
      }
    }
    for (final delivery in feed.deliveries) {
      deliveries[delivery.clientMessageId] = delivery;
      moved.add(_b64(delivery.groupId));
    }
    return (moved: moved, delivered: feed.deliveries.isNotEmpty);
  }

  /// [group]'s entries after [after], oldest first, up to [limit].
  List<DmHomeEntry> page(String group, int after, {int limit = 100}) {
    final entries = _entries[group];
    if (entries == null) return const [];
    return [
      for (final entry in entries.values)
        if (entry.seq > after) entry,
    ].take(limit).toList();
  }

  /// Lets go of [group]'s entries through [seq]: taken in.
  void taken(String group, int seq) =>
      _entries[group]?.removeWhere((each, _) => each <= seq);

  /// The last seq the home has for [group]: what it said, or the latest
  /// entry come since; 0 when it said nothing of it.
  int lastSeq(String group) {
    final said = groups[group]?.lastSeq.toInt() ?? 0;
    final entries = _entries[group];
    final come = entries == null || entries.isEmpty ? 0 : entries.lastKey()!;
    return said > come ? said : come;
  }

  /// Whether the home let go of [group]'s entries past this device's
  /// cursor: it joins again.
  bool behind(String group) => groups[group]?.behind ?? false;

  /// Completes once [group]'s entry [seq] has come, or after [timeout].
  Future<void> arrived(String group, int seq, Duration timeout) {
    if (_entries[group]?.containsKey(seq) ?? false) return Future.value();
    if (lastSeq(group) >= seq) return Future.value();
    final done = Completer<void>();
    (_waiters[group] ??= []).add((seq, done));
    return done.future.timeout(timeout, onTimeout: () {});
  }

  void _woken(String group, int seq) {
    final waiting = _waiters[group];
    if (waiting == null) return;
    waiting.removeWhere((each) {
      final (at, done) = each;
      if (at > seq) return false;
      if (!done.isCompleted) done.complete();
      return true;
    });
  }
}

/// [seq] as the home's cursors carry it.
Int64 dmSeq(int seq) => Int64(seq);
