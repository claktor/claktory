import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

// Every way this app starts showing another Claktory, by its source: a
// code that comes in (pasted, scanned, a link) connects only after the
// person's Connect in confirmPairing (Fugo, 7dfd46e1; Zucchero's row 60).
// A new call that switches Claktories fails here until it's looked at and
// listed, with why it needs no question or where it asks.

/// Where the app switches Claktories (AppScope.switchFactory, or
/// connections.use), and why each is allowed: how many in each file.
const _allowed = {
  // _show: This Mac, a saved Claktory, one on the account; a pasted code
  // only after confirmPairing (_connectPasted). Forget's back to this Mac.
  'lib/devices.dart': 2,
  // _connect: one on the account; a pasted code only after confirmPairing
  // (_connectPasted).
  'lib/connect_step.dart': 1,
  // _connect after confirmPairing (scanned or pasted); one on the account.
  'lib/phone.dart': 2,
  // A link, after confirmPairing; one saved already.
  'lib/links.dart': 2,
  // A Claktory on the person's account (or the caller's onConnect).
  'lib/your_claktories.dart': 1,
  // The rail: This Mac, a saved Claktory, the first saved.
  'lib/rail.dart': 3,
  // The shell's request (#245): only while nothing is saved, from a file
  // only a program on this Mac can write, its digits checked by the agent.
  'lib/setup_link.dart': 1,
  // The helper itself.
  'lib/scope.dart': 1,
};

final _switches = RegExp(r'switchFactory\(|connections\.use\(');

void main() {
  test('every way to another Claktory is one listed (a code asks first)', () {
    final found = <String, int>{};
    for (final file in Directory('lib').listSync(recursive: true)) {
      if (file is! File || !file.path.endsWith('.dart')) continue;
      final path = file.path.replaceAll(r'\', '/');
      final count = _switches.allMatches(file.readAsStringSync()).length;
      if (count > 0) found[path] = count;
    }
    // static void switchFactory(…) => …connections.use(…) is two matches.
    found['lib/scope.dart'] = (found['lib/scope.dart'] ?? 0) - 1;
    expect(found, _allowed);
  });

  test('each pasted or scanned code is asked about before it connects', () {
    // The paste fields: their Connect and Return go through confirmPairing.
    for (final path in ['lib/devices.dart', 'lib/connect_step.dart']) {
      final source = File(path).readAsStringSync();
      expect(source, contains('confirmPairing(context, target)'), reason: path);
      expect(source, contains('_connectPasted(target)'), reason: path);
    }
    final phone = File('lib/phone.dart').readAsStringSync();
    expect(phone, contains('confirmPairing(context, target)'));
  });
}
