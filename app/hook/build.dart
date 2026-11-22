import 'dart:io';

import 'package:flutter_rust_bridge_hooks/flutter_rust_bridge_hooks.dart';

/// The oldest iOS the app runs on, as in ios/Runner.xcodeproj
/// (IPHONEOS_DEPLOYMENT_TARGET; test/ios_build_test.dart keeps the two the
/// same). Without it rustc links for iOS 10 (the Simulator 14), an older
/// layout than the app's.
const iosDeploymentTarget = '15.0';

/// The iPhone and Simulator targets, which rustc leaves unstripped.
///
/// An iPhone's dyld refuses a library whose symbol strings don't start on
/// 8 bytes ("mis-aligned LINKEDIT string pool"; the Simulator doesn't
/// check). rustc strips a release build with rust-objcopy, which puts the
/// strings straight after the indirect symbol table: when that table has
/// an odd number of entries they start 4 bytes off, and Xcode's strip,
/// run on it later, keeps them there. Measured with Rust 1.97.1 and Xcode
/// 27.2 on this crate for the Simulator: 349 entries at iOS 14, rustc's
/// strip 4 off, unstripped or stripped by Xcode on 8; 348 at iOS 15, on 8
/// only by luck. So rustc doesn't strip for iOS; Flutter strips a release
/// app's library with Xcode's strip (-x -S), which aligns them, and writes
/// its dSYM first. The Mac's library keeps rustc's strip.
const iosTriples = [
  'aarch64-apple-ios',
  'aarch64-apple-ios-sim',
  'x86_64-apple-ios',
];

/// The Mac's targets: rustc strips them itself.
const macTriples = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];

/// Android's targets: phones (arm64) and emulators (x86_64 off Apple
/// silicon), 64-bit only (app/android/toolchain.properties). Flutter builds
/// armv7 too unless told otherwise, so Android builds always pass
/// `--target-platform android-arm64,android-x64` (app/android/DESIGN.md).
const androidTriples = ['aarch64-linux-android', 'x86_64-linux-android'];

/// Android loads a library only if its segments are aligned for the
/// phone's page size, and phones with 16 KiB pages ship now (Play requires
/// it of apps targeting Android 15+). native_toolchain_rust adds this flag
/// itself only when it finds no rustflags for the target anywhere, and the
/// home-folder remap below sets them, so it's said here, explicitly.
const androidPageSizeFlag = '-C link-arg=-Wl,-z,max-page-size=16384';

/// The flags rustc gets for [triple]. Every target's say "~" for the home
/// folder of whoever built it: a panic's location and the paths of the
/// crates it was built from would otherwise ship in the app, naming them
/// (`/Users/<name>/.cargo/registry/…`). Per target, since cargo reads a
/// target's own flags only when RUSTFLAGS isn't set.
String rustFlags(String triple, {required String home}) => [
  if (home.isNotEmpty) '--remap-path-prefix=$home=~',
  if (iosTriples.contains(triple)) '-C strip=none',
  if (androidTriples.contains(triple)) androidPageSizeFlag,
].join(' ');

/// Builds the private tunnel's Rust crate (rust/) into the app, through
/// Flutter's native assets: no CocoaPods or Xcode changes, the same for iOS
/// later.
void main(List<String> args) async {
  await build(args, (input, output) async {
    await FlutterRustBridgeNativeAssetsBuilder(
      cratePath: 'rust',
      assetName: 'tunnel/frb_generated.io.dart',
      extraCargoEnvironmentVariables: {
        // Read by rustc only when it builds for iOS or the Simulator.
        'IPHONEOS_DEPLOYMENT_TARGET': iosDeploymentTarget,
        // After the release profile's own strip, so iOS's wins.
        for (final triple in [...iosTriples, ...macTriples, ...androidTriples])
          'CARGO_TARGET_${triple.replaceAll('-', '_').toUpperCase()}_RUSTFLAGS':
              rustFlags(triple, home: Platform.environment['HOME'] ?? ''),
      },
    ).run(input: input, output: output);
  });
}
