import 'dart:convert' show Utf8Decoder;
import 'dart:io' show ZLibDecoder;
import 'dart:typed_data';

/// Why an archive is turned away: it isn't safe to read, or isn't one
/// this reader takes. A sentence for the log, not the person.
class UnsafeArchive implements Exception {
  const UnsafeArchive(this.reason);

  final String reason;

  @override
  String toString() => 'UnsafeArchive: $reason';
}

/// How much an archive may hold, checked before anything is inflated and
/// again while it is (sizes in an archive can lie).
class ArchiveLimits {
  const ArchiveLimits({
    this.maxEntries = 1000,
    this.maxExpanded = 200 * 1024 * 1024,
    this.maxRatio = 100,
    this.maxEntry = 100 * 1024 * 1024,
  });

  /// Entries in it, folders included.
  final int maxEntries;

  /// All its entries together, expanded.
  final int maxExpanded;

  /// How many times bigger an entry may get when expanded.
  final int maxRatio;

  /// One entry, expanded.
  final int maxEntry;
}

/// One file in a ZIP archive, as its central directory lists it.
class ZipEntry {
  const ZipEntry._({
    required this.name,
    required this.method,
    required this.compressedSize,
    required this.size,
    required this.localOffset,
  });

  /// Its path inside the archive, checked: relative, no "..", "/"
  /// separated.
  final String name;
  final int method;
  final int compressedSize;

  /// Its size expanded, as the archive says.
  final int size;
  final int localOffset;

  /// The last part of [name], lowercased.
  String get baseName => name.split('/').last.toLowerCase();
}

/// A ZIP archive held in memory, read with limits: only stored and
/// deflated entries; no encryption, ZIP64, symbolic links, absolute paths,
/// "..", or archives inside it. Nothing is written anywhere.
class BoundedZip {
  BoundedZip._(this._bytes, this.entries, this.limits);

  final Uint8List _bytes;
  final List<ZipEntry> entries;
  final ArchiveLimits limits;

  /// Whether [bytes] start like a ZIP archive.
  static bool looksLikeZip(Uint8List bytes) =>
      bytes.length >= 4 &&
      bytes[0] == 0x50 &&
      bytes[1] == 0x4b &&
      (bytes[2] == 3 || bytes[2] == 5) &&
      (bytes[3] == 4 || bytes[3] == 6);

  /// Reads [bytes]' central directory; throws [UnsafeArchive] for anything
  /// it won't read.
  static BoundedZip open(
    Uint8List bytes, {
    ArchiveLimits limits = const ArchiveLimits(),
  }) {
    final data = ByteData.sublistView(bytes);
    final end = _endOfDirectory(bytes, data);
    final count = data.getUint16(end + 10, Endian.little);
    final dirSize = data.getUint32(end + 12, Endian.little);
    final dirOffset = data.getUint32(end + 16, Endian.little);
    if (count == 0xFFFF || dirSize == 0xFFFFFFFF || dirOffset == 0xFFFFFFFF) {
      throw const UnsafeArchive('ZIP64 archives are not read');
    }
    if (count > limits.maxEntries) {
      throw UnsafeArchive('$count entries, more than ${limits.maxEntries}');
    }
    if (dirOffset + dirSize > end) {
      throw const UnsafeArchive('its directory runs past its end');
    }
    final entries = <ZipEntry>[];
    var at = dirOffset;
    var expanded = 0;
    for (var i = 0; i < count; i++) {
      if (at + 46 > end || data.getUint32(at, Endian.little) != 0x02014b50) {
        throw const UnsafeArchive('a broken directory entry');
      }
      final madeBy = data.getUint16(at + 4, Endian.little) >> 8;
      final flags = data.getUint16(at + 8, Endian.little);
      final method = data.getUint16(at + 10, Endian.little);
      final compressed = data.getUint32(at + 20, Endian.little);
      final size = data.getUint32(at + 24, Endian.little);
      final nameLength = data.getUint16(at + 28, Endian.little);
      final extraLength = data.getUint16(at + 30, Endian.little);
      final commentLength = data.getUint16(at + 32, Endian.little);
      final external = data.getUint32(at + 38, Endian.little);
      final local = data.getUint32(at + 42, Endian.little);
      if (at + 46 + nameLength > end) {
        throw const UnsafeArchive('a name runs past the directory');
      }
      final raw = bytes.sublist(at + 46, at + 46 + nameLength);
      at += 46 + nameLength + extraLength + commentLength;
      if (flags & 1 != 0) throw const UnsafeArchive('it is encrypted');
      if (compressed == 0xFFFFFFFF || size == 0xFFFFFFFF) {
        throw const UnsafeArchive('ZIP64 entries are not read');
      }
      final name = _checkedName(raw);
      // A Unix symbolic link: its mode's file type.
      if (madeBy == 3 && (external >> 16) & 0xF000 == 0xA000) {
        throw UnsafeArchive('a symbolic link: $name');
      }
      if (name.endsWith('/')) continue;
      if (_nested.any(name.toLowerCase().endsWith)) {
        throw UnsafeArchive('an archive inside it: $name');
      }
      if (method != 0 && method != 8) {
        throw UnsafeArchive('an entry packed a way it does not read: $name');
      }
      if (size > limits.maxEntry) {
        throw UnsafeArchive('$name expands to $size bytes');
      }
      if (method == 8 && size > compressed * limits.maxRatio + 1024) {
        throw UnsafeArchive('$name expands more than ${limits.maxRatio}×');
      }
      expanded += size;
      if (expanded > limits.maxExpanded) {
        throw const UnsafeArchive('it expands past its limit');
      }
      entries.add(
        ZipEntry._(
          name: name,
          method: method,
          compressedSize: compressed,
          size: size,
          localOffset: local,
        ),
      );
    }
    return BoundedZip._(bytes, entries, limits);
  }

  static const _nested = [
    '.zip',
    '.jar',
    '.tar',
    '.tgz',
    '.gz',
    '.bz2',
    '.xz',
    '.7z',
    '.rar',
  ];

  /// The end of central directory record: within the last 64 KiB and 22
  /// bytes (its comment's most).
  static int _endOfDirectory(Uint8List bytes, ByteData data) {
    final lowest = bytes.length - 22 - 0xFFFF;
    for (var at = bytes.length - 22; at >= 0 && at >= lowest; at--) {
      if (data.getUint32(at, Endian.little) == 0x06054b50) return at;
    }
    throw const UnsafeArchive('not a ZIP archive');
  }

  /// [raw] as a path inside the archive, or throws: relative, "/" only,
  /// no "." or ".." parts, no control characters.
  static String _checkedName(List<int> raw) {
    final String name;
    try {
      name = const Utf8Decoder().convert(raw);
    } on FormatException {
      throw const UnsafeArchive('a name that is not UTF-8');
    }
    if (name.isEmpty ||
        name.startsWith('/') ||
        name.contains('\\') ||
        name.contains(RegExp(r'[\x00-\x1f]')) ||
        RegExp(r'^[A-Za-z]:').hasMatch(name)) {
      throw UnsafeArchive('an unsafe path: $name');
    }
    final parts = name.split('/');
    for (var i = 0; i < parts.length; i++) {
      final part = parts[i];
      if (part == '..' || part == '.') {
        throw UnsafeArchive('an unsafe path: $name');
      }
      // Only a folder's name ends in "/", so only the last part is empty.
      if (part.isEmpty && i != parts.length - 1) {
        throw UnsafeArchive('an unsafe path: $name');
      }
    }
    return name;
  }

  /// [entry]'s bytes, expanded no further than it said it would (nor the
  /// limits); [checkpoint] is called between chunks, to stop.
  Uint8List read(ZipEntry entry, {void Function()? checkpoint}) {
    final data = ByteData.sublistView(_bytes);
    final at = entry.localOffset;
    if (at + 30 > _bytes.length ||
        data.getUint32(at, Endian.little) != 0x04034b50) {
      throw UnsafeArchive('a broken entry: ${entry.name}');
    }
    final nameLength = data.getUint16(at + 26, Endian.little);
    final extraLength = data.getUint16(at + 28, Endian.little);
    final start = at + 30 + nameLength + extraLength;
    final stop = start + entry.compressedSize;
    if (stop > _bytes.length) {
      throw UnsafeArchive('an entry runs past the end: ${entry.name}');
    }
    if (entry.method == 0) {
      if (entry.compressedSize != entry.size) {
        throw UnsafeArchive('a stored entry of two sizes: ${entry.name}');
      }
      return Uint8List.sublistView(_bytes, start, stop);
    }
    final out = _Capped(entry.size, entry.name);
    final inflate = ZLibDecoder(raw: true).startChunkedConversion(out);
    const chunk = 8 * 1024;
    for (var from = start; from < stop; from += chunk) {
      checkpoint?.call();
      final to = from + chunk < stop ? from + chunk : stop;
      inflate.add(Uint8List.sublistView(_bytes, from, to));
    }
    inflate.close();
    if (out.length != entry.size) {
      throw UnsafeArchive('an entry of another size: ${entry.name}');
    }
    return out.takeBytes();
  }
}

/// Where inflated bytes go: stops the moment there are more than [cap].
class _Capped implements Sink<List<int>> {
  _Capped(this.cap, this.name);

  final int cap;
  final String name;
  final _bytes = BytesBuilder(copy: false);

  int get length => _bytes.length;

  @override
  void add(List<int> data) {
    if (_bytes.length + data.length > cap) {
      throw UnsafeArchive('$name expands past what it said');
    }
    _bytes.add(data);
  }

  @override
  void close() {}

  Uint8List takeBytes() => _bytes.takeBytes();
}
