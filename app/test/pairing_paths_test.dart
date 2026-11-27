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
  // A link already saved. A new one, after confirmPairing, goes through
  // connectNewClaktory (mobile/local_network.dart).
  'lib/links.dart': 1,
  // connectNewClaktory, the first connection's Local Network sheet before
  // it: from phone.dart's _connect after confirmPairing (scanned or
  // pasted) and its account rows, from a link after confirmPairing,
  // from Add a Claktory (devices.dart's _connect): a pasted or scanned
  // code only after its PairingCheck's Connect, and its account rows; and
  // from Your Claktories' rows (one on the person's account); and from
  // the wide layout's first connect (connect_step.dart's _connect): one on
  // the account, or a pasted code only after confirmPairing.
  'lib/mobile/local_network.dart': 1,
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
    for (final path in ['lib/connect_step.dart']) {
      final source = File(path).readAsStringSync();
      expect(source, contains('confirmPairing(context, target)'), reason: path);
      expect(source, contains('_connectPasted(target)'), reason: path);
    }
    // Add a Claktory (area 2) asks in its own dialog, with confirmPairing's
    // pieces: a pasted code (Connect, Return) and a scanned one only reach
    // _check, which shows PairingCheck; _connect runs from that check's
    // Connect and from an account row (YourClaktories asks in place), and
    // from nowhere else.
    final add = File('lib/devices.dart').readAsStringSync();
    expect(add, contains('if (target != null) unawaited(_check(target));'));
    expect(
      add,
      contains(
        'if (pastedPairingCode(scanned) case final target?) '
        'await _check(target);',
      ),
    );
    expect(add, contains('await pairingOnAccount(context, target);'));
    expect(add, contains('PairingCheck(target: target, onAccount: onAccount)'));
    expect(add, contains('title: pairingCheckTitle,'));
    expect(add, contains('onPrimary: () => unawaited(_connect(target)),'));
    expect(
      add,
      contains('onConnect: (target) => unawaited(_connect(target)),'),
    );
    expect(
      RegExp(r'_connect\(').allMatches(add).length,
      3,
      reason:
          "_connect's definition, the check's Connect and the account "
          'rows: a new call is looked at here first',
    );
    expect(
      RegExp(r'connectNewClaktory\(').allMatches(add).length,
      1,
      reason: 'only in _connect',
    );
    // confirmPairing is those pieces, so both ask the same.
    final links = File('lib/links.dart').readAsStringSync();
    expect(
      links,
      contains('PairingCheck(target: target, onAccount: onAccount)'),
    );
    expect(links, contains('pairingOnAccount(context, target)'));
    final phone = File('lib/phone.dart').readAsStringSync();
    expect(phone, contains('confirmPairing(context, target)'));
  });
}
