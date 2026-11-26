import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/shared_build.dart';

/// The tunnel's Rust build folder, shared by every checkout
/// (hook/shared_build.dart).
void main() {
  late Directory temp;
  setUp(() => temp = Directory.systemTemp.createTempSync('shared_build'));
  tearDown(() => temp.deleteSync(recursive: true));

  test('one folder in the caches, unless cargo was told one', () {
    expect(
      sharedBuildDir({'HOME': '/Users/a'}, macOS: true),
      '/Users/a/Library/Caches/claktory-dev/app-rust-build',
    );
    expect(
      sharedBuildDir({'HOME': '/home/a'}, macOS: false),
      '/home/a/.cache/claktory-dev/app-rust-build',
    );
    expect(
      sharedBuildDir({'HOME': '/home/a', 'XDG_CACHE_HOME': '/c'}, macOS: false),
      '/c/claktory-dev/app-rust-build',
    );
    expect(
      sharedBuildDir({'HOME': '/Users/a', 'CARGO_BUILD_BUILD_DIR': '/b'}),
      '/b',
    );
    // Nowhere to share: each checkout builds alone.
    expect(sharedBuildDir({}), '');
  });

  test('the path crates are those Cargo.lock has no source for', () {
    expect(
      pathPackages(File('rust/Cargo.lock').readAsStringSync()),
      containsAll(['claktory_tunnel', 'iroh']),
    );
    expect(
      pathPackages(File('rust/Cargo.lock').readAsStringSync()),
      isNot(contains('iroh-relay')),
    );
  });

  /// Fingerprints in [profile]: the path crates' and crates.io's.
  Directory built(String profile) {
    final dir = Directory('${temp.path}/$profile')..createSync(recursive: true);
    for (final each in [
      'claktory_tunnel-b486e1d2c5c64b38',
      'iroh-8cc1acd9c3c76b41',
      'iroh-relay-079b7373ceb1b261',
      'tokio-0123456789abcdef',
    ]) {
      Directory('${dir.path}/.fingerprint/$each').createSync(recursive: true);
    }
    return dir;
  }

  List<String> left(Directory profile) => [
    for (final each in Directory('${profile.path}/.fingerprint').listSync())
      each.uri.pathSegments.lastWhere((s) => s.isNotEmpty),
  ]..sort();

  test('built last by another checkout, its path crates are built again; '
      "crates.io's stay", () {
    const packages = ['claktory_tunnel', 'iroh'];
    final mac = built('aarch64-apple-darwin/release');
    final host = built('release');
    File('${temp.path}/$ownerName').writeAsStringSync('/trees/b/app/rust/');

    expect(claimTunnelBuild(temp, '/trees/a/app/rust/', packages), isTrue);
    for (final profile in [mac, host]) {
      expect(left(profile), [
        'iroh-relay-079b7373ceb1b261',
        'tokio-0123456789abcdef',
      ]);
    }
    expect(
      File('${temp.path}/$ownerName').readAsStringSync(),
      '/trees/a/app/rust/',
    );

    // Built again by the same checkout: cargo's own checks are right.
    built('aarch64-apple-darwin/release');
    expect(claimTunnelBuild(temp, '/trees/a/app/rust/', packages), isFalse);
    expect(left(mac), hasLength(4));
    // Then by another.
    expect(claimTunnelBuild(temp, '/trees/b/app/rust/', packages), isTrue);
    expect(left(mac), hasLength(2));
  });

  test('a first build anywhere claims it', () {
    final dir = Directory('${temp.path}/none');
    expect(claimTunnelBuild(dir, '/a/', ['iroh']), isTrue);
    expect(File('${dir.path}/$ownerName').readAsStringSync(), '/a/');
  });

  test('the library a checkout ships is its own file, not a link to the '
      "shared folder's", () {
    final shared = File('${temp.path}/shared.dylib')..writeAsStringSync('mine');
    final link = '${temp.path}/target.dylib';
    Process.runSync('ln', [shared.path, link]);
    ownCopy(File(link));
    shared.writeAsStringSync('theirs');
    expect(File(link).readAsStringSync(), 'mine');
    ownCopy(File('${temp.path}/missing.dylib'));
  });

  test("a checkout's own build from before goes; what it ships stays", () {
    final root = Directory('${temp.path}/target');
    for (final profile in ['aarch64-apple-darwin/release', 'release']) {
      for (final name in ['deps', 'build', '.fingerprint', 'incremental']) {
        Directory('${root.path}/$profile/$name').createSync(recursive: true);
      }
    }
    final library = File(
      '${root.path}/aarch64-apple-darwin/release/libclaktory_tunnel.dylib',
    )..writeAsStringSync('lib');
    dropOwnBuild(root);
    expect(library.existsSync(), isTrue);
    for (final profile in ['aarch64-apple-darwin/release', 'release']) {
      expect(
        Directory('${root.path}/$profile').listSync().map(
          (each) => each.uri.pathSegments.lastWhere((s) => s.isNotEmpty),
        ),
        everyElement(isNot(anyOf('deps', 'build', '.fingerprint'))),
      );
    }
    dropOwnBuild(Directory('${temp.path}/none'));
  });

  test('the lock is held around the steps', () async {
    final dir = Directory('${temp.path}/lock');
    final order = <String>[];
    await withSharedBuildLock(dir, () async => order.add('in'));
    expect(order, ['in']);
    expect(File('${dir.path}/$lockName').existsSync(), isTrue);
  });
}
