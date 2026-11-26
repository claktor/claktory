import 'dart:io';

/// One Rust build folder for the tunnel, shared by every checkout of the
/// app on this computer (Bruno 37145: each checkout's own copy of the same
/// dependencies came to 34 GB across 16).
///
/// What cargo builds on the way (the dependencies, build scripts and
/// fingerprints) goes in this shared folder, cargo's `build-dir`. What a
/// checkout's app ships (the tunnel's library) still goes in the
/// checkout's own target folder. Cargo locks the build folder while it
/// builds in it, and the hook holds [lockName] around its own steps too.
///
/// Crates from crates.io are shared as they are: cargo names each build by
/// its version and features, so checkouts at different versions don't
/// collide. Path crates (the tunnel, vendor/'s copies) are the same name
/// in every checkout, so they're built again when another checkout built
/// last ([claimTunnelBuild]).
///
/// Empty when there's nowhere to put it (no home folder): then each
/// checkout builds alone, as before. A CARGO_BUILD_BUILD_DIR already set
/// wins.
String sharedBuildDir(Map<String, String> environment, {bool? macOS}) {
  final given = environment['CARGO_BUILD_BUILD_DIR'] ?? '';
  if (given.isNotEmpty) return given;
  final home = environment['HOME'] ?? '';
  if (home.isEmpty) return '';
  if (macOS ?? Platform.isMacOS) {
    return '$home/Library/Caches/claktory-dev/app-rust-build';
  }
  final caches = environment['XDG_CACHE_HOME'] ?? '';
  return '${caches.isEmpty ? '$home/.cache' : caches}/'
      'claktory-dev/app-rust-build';
}

/// The file the hook locks in the shared build folder while it builds.
const lockName = 'claktory-tunnel.lock';

/// The file in the shared build folder naming the checkout (its rust/
/// folder) whose path crates it holds.
const ownerName = 'claktory-owner';

/// The packages [cargoLock] builds from a path rather than a registry:
/// the tunnel itself and the copies Claktory keeps (vendor/), whose
/// source differs from one checkout to the next.
List<String> pathPackages(String cargoLock) => [
  for (final package in cargoLock.split('[[package]]').skip(1))
    if (!package.contains('\nsource = '))
      if (RegExp(r'^name = "([^"]+)"', multiLine: true).firstMatch(package)
          case final match?)
        match.group(1)!,
];

/// Makes cargo build [packages] (the path crates, [pathPackages]) again in
/// [buildDir] when another checkout than [owner] built there last; then
/// [owner] holds it.
///
/// Cargo names a path crate's build by its path within the workspace, the
/// same in every checkout, and counts it fresh while its files are older
/// than that build. So another checkout's tunnel (or copy of iroh), built
/// after this one's files last changed, would pass for this checkout's.
/// Their fingerprints go instead: cargo builds them again, and everything
/// from crates.io stays built. Answers whether it did.
bool claimTunnelBuild(Directory buildDir, String owner, List<String> packages) {
  buildDir.createSync(recursive: true);
  final mark = File('${buildDir.path}/$ownerName');
  if (mark.existsSync() && mark.readAsStringSync() == owner) return false;
  final names = [
    for (final name in packages)
      RegExp('^${RegExp.escape(name)}-[0-9a-f]{16}\$'),
  ];
  // Each profile's folder (release, debug), for the computer the build
  // runs on and under each target's triple.
  final profiles = [
    for (final each in buildDir.listSync().whereType<Directory>()) ...[
      each,
      ...each.listSync().whereType<Directory>(),
    ],
  ];
  for (final profile in profiles) {
    final fingerprints = Directory('${profile.path}/.fingerprint');
    if (!fingerprints.existsSync()) continue;
    for (final each in fingerprints.listSync().whereType<Directory>()) {
      final name = each.uri.pathSegments.lastWhere((s) => s.isNotEmpty);
      if (names.any((pattern) => pattern.hasMatch(name))) {
        each.deleteSync(recursive: true);
      }
    }
  }
  mark.writeAsStringSync(owner);
  return true;
}

/// Gives [library] a file of its own. Cargo hard-links what it hands on
/// to its file in the shared build folder, and another checkout's next
/// build there must never change this checkout's library.
void ownCopy(File library) {
  if (!library.existsSync()) return;
  final copy = File('${library.path}.own');
  library.copySync(copy.path);
  copy.renameSync(library.path);
}

/// Runs [action] holding the shared build folder's lock, so checking
/// whose tunnel it holds, building, and taking the library are one step.
Future<T> withSharedBuildLock<T>(
  Directory buildDir,
  Future<T> Function() action,
) async {
  buildDir.createSync(recursive: true);
  final lock = await File('${buildDir.path}/$lockName')
      .open(mode: FileMode.append);
  try {
    await lock.lock(FileLock.blockingExclusive);
    return await action();
  } finally {
    await lock.unlock();
    await lock.close();
  }
}

/// What cargo built in a checkout's own target folder [root] before the
/// build folder was shared: unused once it is (cargo keeps there only what
/// the app ships), so it goes. ~800 MB a checkout.
void dropOwnBuild(Directory root) {
  if (!root.existsSync()) return;
  final profiles = [
    for (final each in root.listSync().whereType<Directory>()) ...[
      each,
      ...each.listSync().whereType<Directory>(),
    ],
  ];
  for (final profile in profiles) {
    for (final name in const ['deps', 'build', '.fingerprint', 'incremental']) {
      final dir = Directory('${profile.path}/$name');
      if (dir.existsSync()) dir.deleteSync(recursive: true);
    }
  }
}
