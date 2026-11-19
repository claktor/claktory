import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// The app's one-time move of its user defaults out of the sandbox's
/// container (macos/Runner/SandboxMigration.swift), compiled and run with
/// its checks (test/native/sandbox_migration_check.swift). Needs a Mac with
/// Swift.
void main() {
  final swiftc = Platform.isMacOS
      ? Process.runSync('which', ['swiftc']).stdout.toString().trim()
      : '';
  test(
    'the sandbox\'s defaults come over once, missing keys only',
    () async {
      final out = await Directory.systemTemp.createTemp('factory-migration');
      addTearDown(() => out.delete(recursive: true));
      final binary = '${out.path}/check';
      final build = await Process.run(swiftc, [
        '-o',
        binary,
        'macos/Runner/SandboxMigration.swift',
        'test/native/sandbox_migration_check.swift',
      ]);
      expect(build.exitCode, 0, reason: '${build.stdout}${build.stderr}');
      final run = await Process.run(binary, []);
      expect(run.stdout.toString().trim(), 'ok', reason: '${run.stderr}');
      expect(run.exitCode, 0);
    },
    skip: swiftc.isEmpty ? 'needs swiftc on a Mac' : false,
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
