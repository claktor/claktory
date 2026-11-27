import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart' show assetFile, windowsTriples;

/// The tunnel's Rust library builds for Windows (app/windows, windows.md):
/// the hook finds the library it built by a Windows path as well as a file
/// URI, and the toolchain installs its targets (Avdol's first MSVC build).
void main() {
  test('the toolchain installs every Windows target the hook builds', () {
    final toolchain = File('rust/rust-toolchain.toml').readAsStringSync();
    final windows = RegExp(r'"([a-z0-9_]+-pc-windows-msvc)"')
        .allMatches(toolchain)
        .map((m) => m.group(1))
        .toSet();
    expect(windows, windowsTriples.toSet());
  });

  test("a Windows path's drive letter isn't read as a URI scheme", () {
    const path = r'C:\build\app\target\release\claktory_tunnel.dll';
    expect(assetFile(path).path, path);
    expect(
      assetFile('file:///Users/x/target/release/libclaktory_tunnel.dylib').path,
      '/Users/x/target/release/libclaktory_tunnel.dylib',
    );
  });
}
