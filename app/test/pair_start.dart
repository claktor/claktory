// The phone's first screen while it has no Claktory (#363): what's needed,
// Send me the link, then the ways in. Tests of what comes after go past it
// as the person would.
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// Past the first screen, if it shows: to scanning, or with [paste] to
/// pasting a code, or with [email] to signing in with an email.
Future<void> pastPairStart(
  WidgetTester tester, {
  bool paste = false,
  bool email = false,
}) async {
  if (find.text('Already set up?').evaluate().isEmpty) return;
  final way = find.text(
    email
        ? 'Sign in with your email'
        : paste
        ? 'Paste code instead'
        : 'Scan code instead',
  );
  await tester.ensureVisible(way);
  await pumpFrames(tester);
  await tester.tap(way);
  await pumpFrames(tester);
}
