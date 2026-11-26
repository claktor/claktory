import 'dart:typed_data';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart'
    show buildHome, iosTriples, macTriples, rustFlags, windowsTriples;
import '../hook/macho.dart';

/// The tunnel's Rust library is built so an iPhone will load it: for the
/// iOS the app runs on, and stripped by Xcode rather than rustc (why, in
/// hook/build.dart).
void main() {
  final hook = File('hook/build.dart').readAsStringSync();

  test('the tunnel is built for the iOS the Xcode project targets', () {
    final built = RegExp(r"iosDeploymentTarget = '([0-9.]+)'")
        .firstMatch(hook)
        ?.group(1);
    expect(built, isNotNull, reason: 'hook/build.dart names no iOS target');
    final project = File('ios/Runner.xcodeproj/project.pbxproj')
        .readAsStringSync();
    final targets = RegExp(r'IPHONEOS_DEPLOYMENT_TARGET = ([0-9.]+);')
        .allMatches(project)
        .map((m) => m.group(1))
        .toSet();
    expect(targets, {built});
  });

  test("rustc doesn't strip it for any iOS target the toolchain builds", () {
    final toolchain = File('rust/rust-toolchain.toml').readAsStringSync();
    final ios = RegExp(r'"([a-z0-9_]+-apple-ios[a-z-]*)"')
        .allMatches(toolchain)
        .map((m) => m.group(1))
        .toSet();
    expect(ios, isNotEmpty);
    final unstripped = RegExp(r"'([a-z0-9_]+-apple-ios[a-z-]*)'")
        .allMatches(hook.split('const iosTriples').last.split(';').first);
    expect({for (final m in unstripped) m.group(1)}, ios);
    expect(hook, contains("'-C strip=none'"));
  });

  test("no target's library names the home folder it was built in", () {
    for (final triple in [...iosTriples, ...macTriples]) {
      expect(
        rustFlags(triple, home: '/Users/me'),
        contains('--remap-path-prefix=/Users/me=~'),
        reason: triple,
      );
    }
    expect(
      rustFlags('aarch64-apple-ios', home: '/h'),
      contains('-C strip=none'),
    );
    // The Mac's too: macOS 27's dyld checks the string pool's alignment.
    for (final triple in macTriples) {
      expect(rustFlags(triple, home: '/h'), contains('-C strip=none'));
    }
    expect(rustFlags('aarch64-apple-darwin', home: ''), '-C strip=none');
  });

  test("Windows' libraries don't name it either: its home is USERPROFILE, "
      'and one with a space stops the build rather than ship it', () {
    for (final triple in windowsTriples) {
      expect(
        rustFlags(triple, home: r'C:\Users\me'),
        r'--remap-path-prefix=C:\Users\me=~',
        reason: triple,
      );
    }
    expect(buildHome({'HOME': '/Users/me'}), '/Users/me');
    expect(buildHome({'USERPROFILE': r'C:\Users\me'}), r'C:\Users\me');
    expect(
      buildHome({'HOME': '/Users/me', 'USERPROFILE': r'C:\Users\x'}),
      '/Users/me',
    );
    expect(buildHome({}), '');
    expect(
      () => buildHome({'USERPROFILE': r'C:\Users\John Smith'}),
      throwsStateError,
    );
  });

  group('the hook refuses an Apple library dyld would refuse', () {
    /// A 64-bit Mach-O for [cpu] whose LC_SYMTAB puts its strings at
    /// [stroff].
    Uint8List thin(int stroff, {int cpu = 0x0100000C}) {
      final data = ByteData(32 + 24);
      data
        ..setUint32(0, 0xFEEDFACF, Endian.little)
        ..setUint32(4, cpu, Endian.little)
        ..setUint32(12, 6, Endian.little) // a dylib
        ..setUint32(16, 1, Endian.little)
        ..setUint32(20, 24, Endian.little)
        ..setUint32(32, 0x2, Endian.little)
        ..setUint32(36, 24, Endian.little)
        ..setUint32(48, stroff, Endian.little);
      return data.buffer.asUint8List();
    }

    Uint8List universal(List<Uint8List> slices) {
      const page = 64;
      final bytes = BytesBuilder();
      final header = ByteData(8 + 20 * slices.length)
        ..setUint32(0, 0xCAFEBABE)
        ..setUint32(4, slices.length);
      var offset = page;
      for (final (i, slice) in slices.indexed) {
        final cpu = ByteData.sublistView(slice).getUint32(4, Endian.little);
        header
          ..setUint32(8 + i * 20, cpu)
          ..setUint32(8 + i * 20 + 8, offset)
          ..setUint32(8 + i * 20 + 12, slice.length);
        offset += page;
      }
      bytes.add(header.buffer.asUint8List());
      for (final slice in slices) {
        bytes.add(Uint8List(page * (bytes.length ~/ page + 1) - bytes.length));
        bytes.add(slice);
      }
      return bytes.toBytes();
    }

    test('strings on 8: it loads', () {
      expect(misalignedStringPools(thin(15392368)), isEmpty);
    });

    test("strings 4 bytes off: refused, naming the slice", () {
      // Abbacchio's arm64 Mac slice, 2026-11-22.
      expect(misalignedStringPools(thin(15392364)), [
        'arm64: the symbol strings start at 15392364, 4 bytes off 8',
      ]);
    });

    test('a universal library: each slice checked', () {
      final both = universal([thin(16909960, cpu: 0x01000007), thin(15392364)]);
      expect(misalignedStringPools(both), [
        'arm64: the symbol strings start at 15392364, 4 bytes off 8',
      ]);
      expect(
        misalignedStringPools(
          universal([thin(16909960, cpu: 0x01000007), thin(15392368)]),
        ),
        isEmpty,
      );
    });

    test("Android's ELF isn't Mach-O: left alone", () {
      expect(
        misalignedStringPools(
          Uint8List.fromList([0x7F, 0x45, 0x4C, 0x46, ...List.filled(60, 0)]),
        ),
        isEmpty,
      );
    });
  });
}
