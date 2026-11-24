// Big files over a slow link (#375). A file's bytes go over the same
// WebSocket as everything else, one frame per request, and the app's
// pings wait behind whatever frame is on its way: an 8 MB frame on two
// bars of 4G takes a minute, the pong comes too late, and the connection
// is closed with every request on it. So reads and uploads go in pieces
// sized to the link, about [Pace.target] each: the pong is never long
// behind one, progress moves with each, and a read that loses the
// connection carries on from where it got once it's back.
import 'dart:async';
import 'dart:math' as math;
import 'dart:typed_data' show BytesBuilder;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import 'backend.dart' show RequestFailure;
import 'protocol/factory/v1/factory.pb.dart';

/// How far a read has got: the file's bytes up to [through] (its offset
/// and what has come since) of its whole [size].
typedef ReadProgress = void Function(int through, int size);

/// The screen a preview is made for, in pixels, portrait (#379).
typedef PreviewBox = ({int width, int height});

/// What a read says of whether it's a preview (#379).
extension FilePreviews on FileContent {
  /// A smaller copy made for this screen, in place of the file itself.
  bool get isPreview => preview == FilePreview.FILE_PREVIEW_MADE;
}

/// What a read that lost its connection says when the connection didn't
/// come back in time.
const lostWhileReading =
    'The connection dropped while this was loading and hasn\'t come back. '
    'Try again.';

/// How big a piece to send or ask for next, so each takes about [target]
/// on this link: learned from how long the last ones took.
class Pace {
  Pace({
    this.target = const Duration(milliseconds: 1500),
    this.least = 32 * 1024,
    this.most = 4 * 1024 * 1024,
    int first = 64 * 1024,
  }) : _next = first,
       _first = first;

  final Duration target;
  final int least;
  final int most;
  final int _first;
  int _next;

  /// Bytes a second, as last measured; null before any.
  double? _rate;

  /// How long a request takes there and back with next to nothing in it
  /// (a read of one byte, say); null before one.
  Duration? _roundTrip;

  /// Bytes a second once the round trip is taken out: what a big file
  /// moves at (#379). Null before a piece bigger than a tiny one.
  double? _bandwidth;

  /// The last piece bigger than a tiny one: with the next, bigger one,
  /// what the round trip is when no tiny read has said.
  ({int bytes, Duration elapsed})? _last;

  /// The size of the next piece.
  int get next => _next;

  /// About how long [bytes] would take to come over this link, in pieces:
  /// null until both the round trip and the link's speed are known (a
  /// tiny read and a piece, or two pieces of different sizes).
  Duration? estimate(int bytes) {
    final bandwidth = _bandwidth;
    if (bandwidth == null || _roundTrip == null) return null;
    return Duration(microseconds: (bytes / bandwidth * 1e6).round()) +
        (_roundTrip ?? Duration.zero);
  }

  /// [bytes] took [elapsed], there and back.
  void took(int bytes, Duration elapsed) {
    if (bytes <= 0) return;
    // A tiny piece is all round trip: it says how far away the backend
    // is, not how fast the link is, and mustn't shrink the next piece.
    if (bytes < least ~/ 8) {
      final trip = _roundTrip;
      _roundTrip = trip == null ? elapsed : (trip + elapsed) ~/ 2;
      return;
    }
    // Two pieces of different sizes: what the bigger one took over the
    // smaller is the link's speed, and the rest of it the round trip.
    // What was measured before the round trip was known counted it as
    // the link being slow: forgotten once it's known.
    final tripWasKnown = _roundTrip != null;
    final last = _last;
    if (last != null && bytes >= last.bytes * 2 && elapsed > last.elapsed) {
      final speed =
          (bytes - last.bytes) /
          ((elapsed - last.elapsed).inMicroseconds / 1e6);
      final trip = Duration(
        microseconds: math.max(
          0,
          last.elapsed.inMicroseconds - (last.bytes / speed * 1e6).round(),
        ),
      );
      final known = _roundTrip;
      _roundTrip = known == null ? trip : (known + trip) ~/ 2;
    }
    _last = (bytes: bytes, elapsed: elapsed);
    final transfer = math.max(
      (elapsed - (_roundTrip ?? Duration.zero)).inMicroseconds,
      math.max(elapsed.inMicroseconds ~/ 4, 1000),
    );
    final moved = bytes / (transfer / 1e6);
    final bandwidth = tripWasKnown ? _bandwidth : null;
    _bandwidth = bandwidth == null ? moved : (bandwidth + moved) / 2;
    final seconds = math.max(elapsed.inMicroseconds, 1000) / 1e6;
    final measured = bytes / seconds;
    final rate = _rate;
    // Smoothed, so one slow or quick piece doesn't swing it; but a piece
    // that took far longer than it should is the link now (a fast one gone
    // slow, two bars of 4G), not a blip: half the old speed would still
    // be pieces that hold the connection's pings up (Sale's row 68).
    _rate = rate == null || elapsed > target * 2
        ? measured
        : (rate + measured) / 2;
    final ideal = _rate! * target.inMicroseconds / 1e6;
    // A small piece is mostly the round trip, so it grows at most four
    // times a step and settles from there.
    final grown = math.min(ideal, _next * 4.0);
    _next = grown.round().clamp(least, most);
  }

  /// A piece of [bytes] didn't make it (the connection dropped under it,
  /// most likely because it held the pings up): the next is a quarter of
  /// it, so a retry isn't the same piece cut off again (Sale's row 68).
  void failed(int bytes) {
    if (bytes <= 0) return;
    _next = (math.min(_next, bytes) ~/ 4).clamp(least, most);
    _rate = null;
    _bandwidth = null;
    _last = null;
  }

  /// A new connection, which may be another link altogether (Wi-Fi gone,
  /// cellular now): what was learned on the old one doesn't hold, so the
  /// next piece is the first's size again, and grows from there.
  void relink() {
    _next = _first;
    _rate = null;
    _roundTrip = null;
    _bandwidth = null;
    _last = null;
  }
}

/// How many bytes a read of [mime] from [offset] gets when it asks for no
/// limit: the backend's own limits (factoryd's extensions/viewer.rs), since
/// a read in pieces must stop where a read in one would.
@visibleForTesting
int defaultReadLimit(String mime, int offset) {
  final ranged =
      offset > 0 ||
      mime == 'video/mp4' ||
      mime == 'video/quicktime' ||
      mime == 'application/pdf';
  if (ranged) return 8 * 1024 * 1024;
  const images = {
    'image/png',
    'image/jpeg',
    'image/gif',
    'image/webp',
    'image/bmp',
  };
  if (images.contains(mime)) return 20000000;
  return 2000000;
}

/// One ReadFile request, as the backend answers it.
typedef ReadRange = Future<FileContent> Function(
  int offset,
  int maxBytes,
  String expected,
);

/// Whether a read that has got [through] of [size] goes on: false stops
/// it there, answering what came (a preview would come sooner, #379).
typedef ReadOnward = bool Function(int through, int size);

/// Whether a read goes on past its [first] piece: false answers that
/// piece alone (a page image that didn't come needs none of the PDF,
/// #379).
typedef ReadFirst = bool Function(FileContent first);

/// A read of up to [maxBytes] (0: the backend's limit) from [offset], in
/// pieces of [pace]'s size, answered as one read would be. Pieces after the
/// first name the version the first found, so a file that changes while
/// it comes is read again from the start. Lost connection: once the read
/// has begun, it waits up to [reconnectWithin] for [connected] and carries
/// on from where it got, else fails with [lostWhileReading].
Future<FileContent> pacedRead({
  required ReadRange range,
  required ValueListenable<bool> connected,
  required Pace pace,
  int maxBytes = 0,
  int offset = 0,
  String expectedSha256 = '',
  ReadProgress? onProgress,
  ReadOnward? carryOn,
  ReadFirst? onFirst,
  Duration reconnectWithin = const Duration(seconds: 60),
}) async {
  for (var attempt = 0; ; attempt++) {
    final read = await _readOnce(
      range: range,
      connected: connected,
      pace: pace,
      maxBytes: maxBytes,
      offset: offset,
      expectedSha256: expectedSha256,
      onProgress: onProgress,
      carryOn: carryOn,
      onFirst: onFirst,
      reconnectWithin: reconnectWithin,
    );
    // Changed under it, and the caller named no version: again, as it is
    // now (a file being written can change for a while; a few times).
    if (read != null) return read;
    if (attempt >= 3) {
      return range(offset, maxBytes, expectedSha256);
    }
  }
}

/// [pacedRead] once; null when the file changed while it came.
Future<FileContent?> _readOnce({
  required ReadRange range,
  required ValueListenable<bool> connected,
  required Pace pace,
  required int maxBytes,
  required int offset,
  required String expectedSha256,
  required ReadProgress? onProgress,
  required ReadOnward? carryOn,
  required ReadFirst? onFirst,
  required Duration reconnectWithin,
}) async {
  Future<FileContent> timed(int at, int asked, String expected) async {
    final started = DateTime.now();
    final FileContent piece;
    try {
      piece = await range(at, asked, expected);
    } on RequestFailure {
      pace.failed(asked);
      rethrow;
    }
    pace.took(piece.data.length, DateTime.now().difference(started));
    return piece;
  }

  int capped(int bytes) => maxBytes == 0 ? bytes : math.min(bytes, maxBytes);
  // The first piece: as any read, so a small file is one request.
  final first = await timed(offset, capped(pace.next), expectedSha256);
  if (first.error.isNotEmpty) return first;
  final size = first.size.toInt();
  final limit = maxBytes == 0 ? defaultReadLimit(first.mime, offset) : maxBytes;
  final total = math.max(0, math.min(limit, size - offset));
  onProgress?.call(offset + first.data.length, size);
  if (first.data.length >= total || first.data.isEmpty) return first;
  if (onFirst != null && !onFirst(first)) return first;
  if (carryOn != null && !carryOn(offset + first.data.length, size)) {
    return first;
  }

  final data = BytesBuilder(copy: false)..add(first.data);
  var version = expectedSha256;
  var whole = first.sha256;
  while (data.length < total) {
    final at = offset + data.length;
    final asked = math.min(pace.next, total - data.length);
    final FileContent piece;
    try {
      piece = await timed(at, asked, version);
    } on RequestFailure {
      // Gone with the connection: from here once it's back.
      if (connected.value) rethrow;
      if (!await _connects(connected, reconnectWithin)) {
        throw const RequestFailure(lostWhileReading);
      }
      continue;
    }
    if (piece.error == 'changed' && expectedSha256.isEmpty) return null;
    if (piece.error.isNotEmpty) return piece;
    if (piece.offset.toInt() != at) {
      // A backend from before reads from an offset: as it was, in one.
      return range(offset, maxBytes, expectedSha256);
    }
    // Another version than the first piece's (its time or size moved
    // before the version was known): again.
    if (piece.size != first.size || piece.modifiedMs != first.modifiedMs) {
      if (expectedSha256.isEmpty) return null;
    }
    if (piece.data.isEmpty) break;
    if (version.isEmpty && piece.sha256.isNotEmpty) version = piece.sha256;
    if (piece.sha256.isNotEmpty) whole = piece.sha256;
    data.add(piece.data);
    onProgress?.call(offset + data.length, size);
    if (data.length < total &&
        carryOn != null &&
        !carryOn(offset + data.length, size)) {
      break;
    }
  }
  final bytes = data.takeBytes();
  final truncated = offset > 0 || offset + bytes.length < size;
  final ranged = defaultReadLimit(first.mime, 0) == 8 * 1024 * 1024;
  // As one read answers it, the rest of the first piece's answer (whether
  // it's a preview, say) kept.
  return first.deepCopy()
    ..data = bytes
    ..truncated = truncated
    ..offset = Int64(offset)
    // The whole file's hash, except for text and images read only in part.
    ..sha256 = truncated && !ranged && offset == 0 ? '' : whole;
}

/// Whether [connected] is true, or becomes so within [within].
Future<bool> _connects(ValueListenable<bool> connected, Duration within) {
  if (connected.value) return Future.value(true);
  final done = Completer<bool>();
  late final Timer timer;
  void changed() {
    if (!connected.value || done.isCompleted) return;
    timer.cancel();
    connected.removeListener(changed);
    done.complete(true);
  }

  timer = Timer(within, () {
    connected.removeListener(changed);
    if (!done.isCompleted) done.complete(false);
  });
  connected.addListener(changed);
  return done.future;
}

/// Sends [data] from [offset] in pieces of [pace]'s size, with [send] (one
/// UploadChunk, answering how many bytes the backend has). Answers what the
/// last piece's answer says; one that isn't where the next piece starts
/// (the backend has more, or less) ends it there for the caller to go on
/// from.
Future<int> pacedUpload({
  required Future<int> Function(int offset, List<int> piece) send,
  required Pace pace,
  required int offset,
  required List<int> data,
}) async {
  var sent = 0;
  var received = offset;
  do {
    final end = math.min(data.length, sent + pace.next);
    final piece = data.sublist(sent, end);
    final started = DateTime.now();
    try {
      received = await send(offset + sent, piece);
    } on RequestFailure {
      pace.failed(piece.length);
      rethrow;
    }
    pace.took(piece.length, DateTime.now().difference(started));
    sent = end;
    if (received != offset + sent) return received;
  } while (sent < data.length);
  return received;
}
