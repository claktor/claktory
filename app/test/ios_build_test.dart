import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart' show iosTriples, macTriples, rustFlags;

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
    expect(
      rustFlags('aarch64-apple-darwin', home: '/h'),
      isNot(contains('strip')),
    );
    expect(rustFlags('aarch64-apple-darwin', home: ''), isEmpty);
  });
}
