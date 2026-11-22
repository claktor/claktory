import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart'
    show androidPageSizeFlag, androidTriples, iosTriples, macTriples, rustFlags;

/// The tunnel's Rust library is built so an Android phone will load it: for
/// every Android target the toolchain installs and the app ships, aligned
/// for 16 KiB pages, naming no home folder (why, in hook/build.dart).
void main() {
  final toolchain = File('rust/rust-toolchain.toml').readAsStringSync();
  final pins = {
    for (final line in File('android/toolchain.properties').readAsLinesSync())
      if (line.contains('=') && !line.startsWith('#'))
        line.split('=').first.trim(): line.split('=').skip(1).join('=').trim(),
  };

  test('the hook builds for every Android target the toolchain installs', () {
    final android = RegExp(r'"([a-z0-9_]+-linux-android[a-z]*)"')
        .allMatches(toolchain)
        .map((m) => m.group(1))
        .toSet();
    expect(android, androidTriples.toSet());
    expect(pins['rust.targets']!.split(',').toSet(), androidTriples.toSet());
  });

  test('every Android ABI the app ships has its target', () {
    const tripleOfAbi = {
      'arm64-v8a': 'aarch64-linux-android',
      'x86_64': 'x86_64-linux-android',
    };
    final abis = pins['android.abis']!.split(',');
    expect(abis, isNotEmpty);
    for (final abi in abis) {
      expect(androidTriples, contains(tripleOfAbi[abi]), reason: abi);
    }
  });

  test('Android libraries are aligned for 16 KiB pages and remap home', () {
    expect(pins['android.pageSize'], '16384');
    expect(androidPageSizeFlag, contains('max-page-size=16384'));
    for (final triple in androidTriples) {
      final flags = rustFlags(triple, home: '/Users/someone');
      expect(flags, contains(androidPageSizeFlag), reason: triple);
      expect(flags, contains('--remap-path-prefix=/Users/someone=~'), reason: triple);
      expect(flags, isNot(contains('strip')), reason: triple);
    }
  });

  test("Android's flag reaches no Apple target", () {
    for (final triple in [...iosTriples, ...macTriples]) {
      expect(rustFlags(triple, home: '/h'), isNot(contains('max-page-size')), reason: triple);
    }
  });
}
