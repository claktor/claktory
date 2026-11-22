import 'dart:typed_data';

/// Why the Mach-O image [bytes] (thin or universal) wouldn't load where dyld
/// checks its symbol strings' alignment, one line per slice that fails; empty
/// when every slice passes, or [bytes] isn't Mach-O at all (Android's ELF).
///
/// An iPhone's dyld, and macOS 27's, refuse an image whose symbol string
/// pool (LC_SYMTAB's stroff) doesn't start on the pointer's size: "mis-aligned
/// LINKEDIT string pool". rustc's own strip can leave it 4 bytes off (why,
/// in build.dart), so the hook refuses such a library rather than ship an
/// app that can't load its tunnel. scripts/check-macho.py checks a whole
/// build the same way, and more.
List<String> misalignedStringPools(Uint8List bytes) {
  if (bytes.length < 8) return const [];
  final data = ByteData.sublistView(bytes);
  final fat = data.getUint32(0, Endian.big);
  if (fat == _fatMagic || fat == _fatMagic64) {
    final wide = fat == _fatMagic64;
    final count = data.getUint32(4, Endian.big);
    final problems = <String>[];
    for (var i = 0; i < count; i++) {
      final at = 8 + i * (wide ? 32 : 20);
      if (at + (wide ? 32 : 20) > bytes.length) break;
      final cpu = data.getUint32(at, Endian.big);
      final offset = wide
          ? data.getUint64(at + 8, Endian.big)
          : data.getUint32(at + 8, Endian.big);
      final size = wide
          ? data.getUint64(at + 16, Endian.big)
          : data.getUint32(at + 12, Endian.big);
      if (offset + size > bytes.length) {
        problems.add('${_cpuName(cpu)}: its slice is cut short');
        continue;
      }
      problems.addAll(
        _thin(Uint8List.sublistView(bytes, offset, offset + size)),
      );
    }
    return problems;
  }
  return _thin(bytes);
}

List<String> _thin(Uint8List bytes) {
  if (bytes.length < 28) return const [];
  final data = ByteData.sublistView(bytes);
  final magic = data.getUint32(0, Endian.little);
  if (magic != _magic && magic != _magic64) return const [];
  final wide = magic == _magic64;
  final cpu = data.getUint32(4, Endian.little);
  final commands = data.getUint32(16, Endian.little);
  final pointer = wide ? 8 : 4;
  var at = wide ? 32 : 28;
  for (var i = 0; i < commands && at + 8 <= bytes.length; i++) {
    final command = data.getUint32(at, Endian.little);
    final size = data.getUint32(at + 4, Endian.little);
    if (command == _lcSymtab && at + 24 <= bytes.length) {
      final stroff = data.getUint32(at + 16, Endian.little);
      if (stroff % pointer != 0) {
        return [
          '${_cpuName(cpu)}: the symbol strings start at $stroff, '
              '${stroff % pointer} bytes off $pointer',
        ];
      }
    }
    if (size < 8) break;
    at += size;
  }
  return const [];
}

const _magic = 0xFEEDFACE;
const _magic64 = 0xFEEDFACF;
const _fatMagic = 0xCAFEBABE;
const _fatMagic64 = 0xCAFEBABF;
const _lcSymtab = 0x2;

String _cpuName(int cpu) => switch (cpu) {
  0x0100000C => 'arm64',
  0x01000007 => 'x86_64',
  0x0000000C => 'arm',
  0x00000007 => 'i386',
  _ => 'cpu $cpu',
};
