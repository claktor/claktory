// The device's diagnostics for one Claktory (Settings' advanced diagnostics,
// off unless turned on there): what the app did and saw, by area, so an
// agent can find out why something went wrong. Recorded only while it's on
// for that Claktory (Workspace.diagnostics.share, remembered here so a
// device that can't connect still records), kept in a file as it happens (a
// force-quit loses nothing), capped, and sent to the Claktory once it
// connects (backend.dart). Connections (#214) are the first area.
//
// What it may say is fixed here, not free text (Fugo): an event is a
// [DiagnosticKind] of its area, and a detail made only of that area's
// [DiagnosticKey]s, each with a number, yes or no, or a [DiagnosticWord]
// from a fixed list. No address, relay, endpoint id, code, name or message
// can get in, since nothing here takes a string from outside.
//
// A new area is one place: its name in [DiagnosticArea], an enum of its
// kinds (implementing [DiagnosticKind], naming the area), one of its keys
// (implementing [DiagnosticKey]), and enums for the words its values may
// take (implementing [DiagnosticWord]); its events go through the same
// [DiagnosticsRecorder.add].
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart' show visibleForTesting;

import 'network.dart';

/// The areas diagnostics cover, as the Claktory files them.
enum DiagnosticArea { connection, notifications }

/// What happened, in an area.
abstract interface class DiagnosticKind implements Enum {
  DiagnosticArea get area;
}

/// A key a detail may have, in an area.
abstract interface class DiagnosticKey implements Enum {
  /// As the detail writes it.
  String get wire;
}

/// A word a detail's value may be: only the lists here implement it.
abstract interface class DiagnosticWord implements Enum {}

/// What happened to the connection.
enum ConnectionKind implements DiagnosticKind {
  /// The app went to the background, came back, or started.
  lifecycle,

  /// The device's network changed.
  network,

  /// A try to connect began.
  connect,

  /// A try connected (the Claktory welcomed it).
  connected,

  /// A try failed, or a connection closed.
  disconnected,

  /// Whether the relay can be reached changed.
  relay,

  /// The tunnel's path changed: direct, or through the relay.
  path,

  /// The connection was made afresh: why.
  reconnect;

  @override
  DiagnosticArea get area => DiagnosticArea.connection;
}

/// The keys a connection event's detail may have.
enum ConnectionKey implements DiagnosticKey {
  /// The app's state: [AppLife].
  app,

  /// How long the app was away, in seconds.
  awayS,

  /// The network's kind: [NetKind].
  net,

  /// How a connection goes: [ConnectionRoute].
  route,

  /// Why it didn't connect, or closed: [ConnectionFailure].
  why,

  /// Why it reconnected: [ConnectionReason].
  reason,

  /// The WebSocket close code.
  code,

  /// Whether the relay can be reached (1) or not (0).
  up,

  /// Which try this is, since the app started.
  attempt,

  /// Whether the try was welcomed before it ended.
  welcomed,

  /// Why the relay can't be reached, roughly: [RelayTrouble] (#388).
  err;

  @override
  String get wire => switch (this) {
    ConnectionKey.awayS => 'away_s',
    _ => name,
  };
}

enum AppLife implements DiagnosticWord { started, background, back }

/// Why this device's relay can't be reached, in a word: the class of the
/// tunnel's last error for it, never the error itself (it can name hosts).
enum RelayTrouble implements DiagnosticWord {
  /// None chosen yet: none answered the device's last measurement of the
  /// network.
  unchosen,

  /// Its name couldn't be looked up.
  dns,

  /// Nothing answered in time.
  timeout,

  /// The secure connection to it failed.
  tls,

  /// The network turned it away: refused, reset or unreachable.
  network,

  other,
}

enum NetKind implements DiagnosticWord { wifi, cellular, wired, other, none }

/// The kind of network [path] goes through first.
NetKind netKind(NetworkPath path) => !path.satisfied
    ? NetKind.none
    : switch (path.primary) {
        'wifi' => NetKind.wifi,
        'cellular' => NetKind.cellular,
        'wired' => NetKind.wired,
        _ => NetKind.other,
      };

/// How a try goes: this Mac's own Claktory on its open port ([local]),
/// through the private tunnel, its path not known yet ([tunnel]), then
/// [direct] or [relayed]; or a code from before the tunnel ([plain]).
enum ConnectionRoute implements DiagnosticWord {
  local,
  tunnel,
  direct,
  relayed,
  plain,
}

enum ConnectionFailure implements DiagnosticWord {
  /// Not connected, or not welcomed, in time.
  timeout,

  /// This device was turned away.
  refused,

  /// The connection couldn't be made (its socket failed).
  unreachable,

  /// The tunnel didn't open.
  tunnel,

  /// It closed (see the code).
  closed,

  /// The certificate wasn't the pinned one.
  handshake,

  /// This device has to sign in first.
  signin,

  /// Given up on, to start from scratch.
  abandoned,

  /// Anything else.
  other,
}

enum ConnectionReason implements DiagnosticWord {
  /// Back from the background.
  resume,

  /// The network moved.
  network,

  /// The person asked (a tap on Reconnecting…).
  asked,

  /// Not back in time after a reconnect.
  watch,

  /// The connection didn't answer a check.
  check,

  /// A live connection dropped.
  dropped,

  /// The tunnels' runtime didn't answer, and was started afresh.
  runtime,

  /// The tunnel didn't bring the connection back in time, and was opened
  /// afresh.
  tunnel,

  /// What the Claktory sent couldn't be applied.
  patch,
}

/// What happened when this device set up its notifications with a
/// Claktory (mobile/push.dart).
enum NotificationsKind implements DiagnosticKind {
  /// It registered: with the push relay afresh ([NotificationsKey.fresh]),
  /// and with the Claktory.
  registered,

  /// It couldn't: at which step, and the keychain's status.
  failed;

  @override
  DiagnosticArea get area => DiagnosticArea.notifications;
}

/// The keys a notifications event's detail may have.
enum NotificationsKey implements DiagnosticKey {
  /// Where it failed: [PushStep].
  step,

  /// The keychain's status (an OSStatus, its minus sign dropped:
  /// 34018 is errSecMissingEntitlement).
  status,

  /// Whether it asked the push relay for a new handle.
  fresh;

  @override
  String get wire => name;
}

/// Where setting up notifications failed.
enum PushStep implements DiagnosticWord {
  /// Asking the push relay for a handle.
  relay,

  /// Keeping the key in the keychain.
  key,

  /// Keeping the registration in the keychain.
  registration,

  /// Telling the Claktory.
  claktory,

  /// Anything else.
  other,
}

/// One event, as the device keeps it and sends it.
class RecordedEvent {
  const RecordedEvent({
    required this.atMs,
    required this.seq,
    required this.area,
    required this.kind,
    required this.detail,
    required this.durationMs,
    required this.session,
  });

  final int atMs;
  final int seq;
  final String area;
  final String kind;
  final String detail;
  final int durationMs;
  final int session;

  Map<String, Object> toJson() => {
    'at': atMs,
    'seq': seq,
    'area': area,
    'kind': kind,
    'detail': detail,
    'ms': durationMs,
    'session': session,
  };

  static RecordedEvent? fromJson(Object? json) {
    if (json is! Map) return null;
    final at = json['at'], seq = json['seq'], kind = json['kind'];
    final area = json['area'];
    final detail = json['detail'], ms = json['ms'], session = json['session'];
    if (at is! int ||
        seq is! int ||
        area is! String ||
        kind is! String ||
        detail is! String ||
        ms is! int ||
        session is! int) {
      return null;
    }
    return RecordedEvent(
      atMs: at,
      seq: seq,
      area: area,
      kind: kind,
      detail: detail,
      durationMs: ms,
      session: session,
    );
  }
}

/// [values] as a detail: `key=value` pairs on one line. Only an area's
/// keys, and only numbers or the lists' words: anything else is refused,
/// so nothing from outside the app's own words gets in.
String recordDetail(Map<DiagnosticKey, Object> values) => [
  for (final MapEntry(:key, :value) in values.entries)
    '${key.wire}=${switch (value) {
      final int number => '$number',
      final bool yes => yes ? '1' : '0',
      final DiagnosticWord word => word.name,
      _ => throw ArgumentError.value(value, key.wire, 'not a number or word'),
    }}',
].join(' ');

/// The record for one Claktory, every area, in [file]: one event per
/// line, appended as it happens. At most [maxEvents] and [maxBytes]: the
/// oldest go first.
class DiagnosticsRecord {
  DiagnosticsRecord(
    this.file, {
    required this.session,
    this.maxEvents = 5000,
    this.maxBytes = 1024 * 1024,
    DateTime Function()? now,
  }) : _now = now ?? DateTime.now;

  final File file;

  /// This run of the app ([nextSession]).
  final int session;
  final int maxEvents;
  final int maxBytes;
  final DateTime Function() _now;

  /// What's kept, oldest first; read from [file] the first time.
  List<RecordedEvent>? _events;
  var _bytes = 0;

  /// Events recorded in this [session]: an event's seq is the session's
  /// number times 2^32 plus this, so it goes up across launches too, and
  /// one sent again is known by it.
  var _count = 0;

  List<RecordedEvent> get _kept => _events ??= _read();

  List<RecordedEvent> _read() {
    final events = <RecordedEvent>[];
    try {
      if (file.existsSync()) {
        for (final line in file.readAsLinesSync()) {
          if (line.trim().isEmpty) continue;
          try {
            final event = RecordedEvent.fromJson(jsonDecode(line));
            if (event != null) events.add(event);
          } on FormatException {
            // A line cut short by a force-quit mid-write: dropped.
          }
        }
      }
    } on FileSystemException {
      // Unreadable: starts over.
    }
    _bytes = events.fold(0, (sum, e) => sum + _size(e));
    return events;
  }

  static int _size(RecordedEvent event) =>
      utf8.encode(jsonEncode(event.toJson())).length + 1;

  /// What's waiting to be sent, oldest first.
  List<RecordedEvent> get waiting => List.unmodifiable(_kept);

  /// How many wait.
  int get waitingCount => _kept.length;

  /// Records [kind] with [detail] (and how long it took), in its area.
  void add(
    DiagnosticKind kind, [
    Map<DiagnosticKey, Object> detail = const {},
    Duration took = Duration.zero,
  ]) {
    final events = _kept;
    final event = RecordedEvent(
      atMs: _now().millisecondsSinceEpoch,
      seq: (session << 32) + ++_count,
      area: kind.area.name,
      kind: kind.name,
      detail: recordDetail(detail),
      durationMs: took.inMilliseconds,
      session: session,
    );
    events.add(event);
    _bytes += _size(event);
    if (events.length > maxEvents || _bytes > maxBytes) {
      while (events.isNotEmpty &&
          (events.length > maxEvents || _bytes > maxBytes)) {
        _bytes -= _size(events.removeAt(0));
      }
      _rewrite();
    } else {
      final line = '${jsonEncode(event.toJson())}\n';
      _write((sink) => sink.writeStringSync(line), FileMode.append);
    }
  }

  /// The Claktory kept everything up to [seq]: it goes from the device.
  void sent(int seq) {
    final events = _kept;
    final before = events.length;
    events.removeWhere((event) => event.seq <= seq);
    if (events.length == before) return;
    _bytes = events.fold(0, (sum, e) => sum + _size(e));
    _rewrite();
  }

  /// Forgets everything: sharing turned off.
  void clear() {
    _events = [];
    _bytes = 0;
    try {
      if (file.existsSync()) file.deleteSync();
    } on FileSystemException {
      // Gone already.
    }
  }

  void _rewrite() {
    final text = _kept.map((e) => '${jsonEncode(e.toJson())}\n').join();
    _write((sink) => sink.writeStringSync(text), FileMode.write);
  }

  /// Written at once, and flushed: what's recorded is on disk before
  /// anything else happens, so a force-quit loses none of it.
  void _write(void Function(RandomAccessFile) write, FileMode mode) {
    try {
      file.parent.createSync(recursive: true);
      final open = file.openSync(mode: mode);
      try {
        write(open);
        open.flushSync();
      } finally {
        open.closeSync();
      }
    } on FileSystemException {
      // A full disk: what's in memory is still sent.
    }
  }

  /// Done once every write so far is on disk: at once (tests).
  Future<void> get settled => Future.value();
}

/// This app run's number, one more than the last run's, kept in [file]: a
/// session boundary in the record is a relaunch (or force-quit).
int nextSession(File file) {
  var last = 0;
  try {
    if (file.existsSync()) last = int.tryParse(file.readAsStringSync()) ?? 0;
  } on FileSystemException {
    // Starts again from 1.
  }
  final next = last + 1;
  try {
    file.parent.createSync(recursive: true);
    file.writeAsStringSync('$next', flush: true);
  } on FileSystemException {
    // Not kept: this run still has its number.
  }
  return next;
}

/// The folder this device keeps its diagnostics in: Android's private
/// no_backup folder, or Application Support (the iPhone's and the iPad's
/// container, the Mac's Claktory or Claktory Dev folder).
/// Null on unsupported platforms and in unit tests.
Directory? diagnosticsFolder() {
  // Never the person's own from a unit test (the app itself, driven by an
  // integration test, records as it would).
  if (Platform.resolvedExecutable.contains('flutter_tester')) return null;
  return diagnosticsFolderFor(
    operatingSystem: Platform.operatingSystem,
    temporary: Directory.systemTemp,
    home: Platform.environment['HOME'],
    dev: const bool.fromEnvironment('FACTORY_DEV'),
  );
}

/// The device's folder selection, separate from the unit-test write guard.
@visibleForTesting
Directory? diagnosticsFolderFor({
  required String operatingSystem,
  required Directory temporary,
  String? home,
  bool dev = false,
}) {
  if (operatingSystem == 'android') {
    // As for kept voice recordings (#401): Flutter gives Dart the app's
    // code_cache as systemTemp. Its sibling no_backup is Android's
    // Context.getNoBackupFilesDir(): private, persistent across updates,
    // and excluded from backup and device transfer. Never clear its other
    // contents (credentials, drafts and recordings); own only diagnostics.
    return Directory('${temporary.parent.path}/no_backup/diagnostics');
  }
  if (operatingSystem == 'ios') {
    // The app's environment is empty here (no HOME); its temporary folder
    // is the container's tmp, beside its Library.
    final temp = temporary;
    if (temp.uri.pathSegments.where((s) => s.isNotEmpty).lastOrNull != 'tmp') {
      return null;
    }
    return Directory(
      '${temp.parent.path}/Library/Application Support/diagnostics',
    );
  }
  if (operatingSystem == 'macos') {
    if (home == null || home.isEmpty) return null;
    final app = dev ? 'Claktory Dev' : 'Claktory';
    return Directory('$home/Library/Application Support/$app/diagnostics');
  }
  return null;
}

/// This run of the app's session, the same for every Claktory it shows.
final _sessions = <String, int>{};

/// The device's diagnostics, for the Claktory a connection shows: recorded
/// only while that Claktory says to share them, which is remembered here
/// (a file beside its record) so a device that can't reach it still
/// records why.
class DiagnosticsRecorder {
  DiagnosticsRecorder(
    this.folder, {
    this.maxEvents = 5000,
    this.maxBytes = 1024 * 1024,
    this.now,
  });

  /// On this device, where it can keep them; null where it can't.
  static DiagnosticsRecorder? onDevice() {
    final folder = diagnosticsFolder();
    return folder == null ? null : DiagnosticsRecorder(folder);
  }

  final Directory folder;

  /// This run of the app, counted only once something is recorded: with
  /// diagnostics off, nothing is written.
  late final int session = _sessions.putIfAbsent(
    folder.path,
    () => nextSession(File('${folder.path}/session')),
  );
  final int maxEvents;
  final int maxBytes;

  /// The clock (tests).
  final DateTime Function()? now;

  /// The Claktory recorded for, as a file name; null before one is chosen.
  String? _claktory;

  /// Its record, while sharing is on for it.
  DiagnosticsRecord? _record;

  /// Records for the Claktory [id] names from now on (its endpoint id, say;
  /// kept only in this device's file names, never sent).
  void select(String id) {
    final name = _fileName(id);
    if (name == _claktory) return;
    _claktory = name;
    _record = File('${folder.path}/$name.on').existsSync() ? _open(name) : null;
  }

  DiagnosticsRecord _open(String name) => DiagnosticsRecord(
    File('${folder.path}/$name.jsonl'),
    session: session,
    maxEvents: maxEvents,
    maxBytes: maxBytes,
    now: now,
  );

  /// Whether it records now.
  bool get on => _record != null;

  /// The Claktory said whether to share: remembered, and turned off,
  /// what's kept goes.
  void share(bool share) {
    final name = _claktory;
    if (name == null || share == on) return;
    final marker = File('${folder.path}/$name.on');
    try {
      if (share) {
        folder.createSync(recursive: true);
        marker.writeAsStringSync('', flush: true);
      } else if (marker.existsSync()) {
        marker.deleteSync();
      }
    } on FileSystemException {
      // Not remembered: it follows the Claktory once connected again.
    }
    if (share) {
      _record = _open(name);
    } else {
      (_record ?? _open(name)).clear();
      _record = null;
    }
  }

  /// Records [kind], when sharing is on.
  void add(
    DiagnosticKind kind, [
    Map<DiagnosticKey, Object> detail = const {},
    Duration took = Duration.zero,
  ]) => _record?.add(kind, detail, took);

  /// What's waiting to be sent, oldest first.
  List<RecordedEvent> get waiting => _record?.waiting ?? const [];

  /// How many wait.
  int get waitingCount => _record?.waitingCount ?? 0;

  /// The Claktory kept everything up to [seq].
  void sent(int seq) => _record?.sent(seq);

  /// Done once every write so far is on disk (tests).
  Future<void> get settled => _record?.settled ?? Future.value();

  static String _fileName(String id) {
    final safe = id.toLowerCase().replaceAll(RegExp('[^a-z0-9_-]'), '_');
    return safe.isEmpty
        ? 'local'
        : safe.length > 80
        ? safe.substring(0, 80)
        : safe;
  }
}
