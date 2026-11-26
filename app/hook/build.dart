import 'dart:io';

import 'package:flutter_rust_bridge_hooks/flutter_rust_bridge_hooks.dart';

import 'macho.dart';
import 'shared_build.dart';

/// The oldest iOS the app runs on, as in ios/Runner.xcodeproj
/// (IPHONEOS_DEPLOYMENT_TARGET; test/ios_build_test.dart keeps the two the
/// same). Without it rustc links for iOS 10 (the Simulator 14), an older
/// layout than the app's.
const iosDeploymentTarget = '15.0';

/// The iPhone and Simulator targets, which rustc leaves unstripped, as
/// it does the Mac's ([unstrippedTriples]).
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
/// its dSYM first.
const iosTriples = [
  'aarch64-apple-ios',
  'aarch64-apple-ios-sim',
  'x86_64-apple-ios',
];

/// The Mac's targets. macOS 27's dyld checks the string pool's alignment
/// too, and the Mac's library hit it (Abbacchio, 2026-11-22: the arm64
/// slice's strings 4 bytes off after rustc's strip, "mis-aligned LINKEDIT
/// string pool", the app unable to load its tunnel), so rustc leaves them
/// unstripped as well.
const macTriples = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];

/// The Apple targets rustc doesn't strip: every one.
const unstrippedTriples = [...iosTriples, ...macTriples];

/// Android's targets: phones (arm64) and emulators (x86_64 off Apple
/// silicon), 64-bit only (app/android/toolchain.properties). Flutter builds
/// armv7 too unless told otherwise, so Android builds always pass
/// `--target-platform android-arm64,android-x64` (app/android/DESIGN.md).
const androidTriples = ['aarch64-linux-android', 'x86_64-linux-android'];

/// Windows' targets (app/windows, windows.md): x64 first, Arm64 later.
const windowsTriples = ['x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'];

/// Android loads a library only if its segments are aligned for the
/// phone's page size, and phones with 16 KiB pages ship now (Play requires
/// it of apps targeting Android 15+). native_toolchain_rust adds this flag
/// itself only when it finds no rustflags for the target anywhere, and the
/// home-folder remap below sets them, so it's said here, explicitly.
const androidPageSizeFlag = '-C link-arg=-Wl,-z,max-page-size=16384';

/// The home folder of whoever builds: HOME on a Mac or Linux, USERPROFILE on
/// Windows, where HOME is normally unset (`C:\Users\name`); empty when
/// neither is set. One with a space stops the build, saying why: cargo
/// splits a target's RUSTFLAGS on spaces, so its remap can't be passed, and
/// leaving it out would ship the folder's path in the app.
String buildHome(Map<String, String> environment) {
  final home = environment['HOME'] ?? environment['USERPROFILE'] ?? '';
  if (home.contains(RegExp(r'\s'))) {
    throw StateError(
      'The home folder "$home" has a space, so the tunnel can\'t be built '
      'without its path in the app (cargo splits RUSTFLAGS on spaces). '
      'Build from an account whose home folder has none.',
    );
  }
  return home;
}

/// The flags rustc gets for [triple]. Every target's say "~" for the home
/// folder of whoever built it: a panic's location and the paths of the
/// crates it was built from would otherwise ship in the app, naming them
/// (`/Users/<name>/.cargo/registry/…`). Per target, since cargo reads a
/// target's own flags only when RUSTFLAGS isn't set.
String rustFlags(String triple, {required String home}) => [
  if (home.isNotEmpty) '--remap-path-prefix=$home=~',
  if (unstrippedTriples.contains(triple)) '-C strip=none',
  if (androidTriples.contains(triple)) androidPageSizeFlag,
].join(' ');

/// Builds the private tunnel's Rust crate (rust/) into the app, through
/// Flutter's native assets: no CocoaPods or Xcode changes, the same for iOS
/// later.
void main(List<String> args) async {
  await build(args, (input, output) async {
    final shared = sharedBuildDir(Platform.environment);
    Future<void> rust() => FlutterRustBridgeNativeAssetsBuilder(
      cratePath: 'rust',
      assetName: 'tunnel/frb_generated.io.dart',
      extraCargoEnvironmentVariables: {
        // Read by rustc only when it builds for iOS or the Simulator.
        'IPHONEOS_DEPLOYMENT_TARGET': iosDeploymentTarget,
        // After the release profile's own strip, so iOS's wins.
        for (final triple in [
          ...iosTriples,
          ...macTriples,
          ...androidTriples,
          ...windowsTriples,
        ])
          'CARGO_TARGET_${triple.replaceAll('-', '_').toUpperCase()}_RUSTFLAGS':
              rustFlags(triple, home: buildHome(Platform.environment)),
        // Dependencies built once for every checkout (shared_build.dart).
        if (shared.isNotEmpty) 'CARGO_BUILD_BUILD_DIR': shared,
      },
    ).run(input: input, output: output);
    List<File> libraries() => [
      for (final asset in output.build().assets.encodedAssets)
        if (asset.encoding['file'] case final String file)
          File.fromUri(Uri.parse(file)),
    ];
    if (shared.isEmpty) {
      await rust();
    } else {
      final dir = Directory(shared);
      await withSharedBuildLock(dir, () async {
        final crate = input.packageRoot.resolve('rust/');
        claimTunnelBuild(
          dir,
          crate.toFilePath(),
          pathPackages(
            File.fromUri(crate.resolve('Cargo.lock')).readAsStringSync(),
          ),
        );
        await rust();
        for (final library in libraries()) {
          ownCopy(library);
          // <target>/<triple>/<profile>/<library>
          dropOwnBuild(library.parent.parent.parent);
        }
      });
    }
    // An Apple library dyld would refuse fails the build, not the app.
    for (final library in libraries()) {
      if (!library.existsSync()) continue;
      final problems = misalignedStringPools(library.readAsBytesSync());
      if (problems.isNotEmpty) {
        throw StateError(
          '${library.path} would not load on an iPhone or a Mac '
          '("mis-aligned LINKEDIT string pool"):\n${problems.join('\n')}\n'
          'Leave it to Xcode to strip (-C strip=none, unstrippedTriples).',
        );
      }
    }
  });
}
