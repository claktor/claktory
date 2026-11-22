// A question card's decision on a phone (Prosciutto, from Sorbet's shots):
// its caption on a line of its own when the buttons don't fit beside it,
// Dismiss and Approve together under it with Approve at the end, under the
// thumb; and a lone button (Copy, beside a command) at the end too, a
// finger's size, at any text size.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _command = 'launchctl kickstart -k gui/501/com.example.helper';

Future<void> _card(
  WidgetTester tester,
  double width, {
  double scale = 1,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.platformDispatcher.textScaleFactorTestValue = scale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(
    Workspace(
      agents: [
        Agent(
          id: 'theo',
          name: 'Theo',
          modelAvailable: true,
          waitingSeqs: [Int64(7)],
        ),
      ],
      selectedAgentId: 'theo',
    ),
  );
  harness.backend.transcript.load(
    Transcript(
      agentId: 'theo',
      entries: [
        Entry(
          seq: Int64(7),
          timestampMs: Int64(1000),
          askPerson: AskPerson(
            id: 'ask7',
            title: 'Restart the helper',
            body: 'Run this:\n\n```sh\n$_command\n```',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
          ),
        ),
      ],
    ),
  );
  await harness.show(tester, size: Size(width, 852));
  await settle(tester);
}

/// The button labelled [label], whole.
Rect _button(WidgetTester tester, String label) => tester.getRect(
  find.ancestor(of: find.text(label), matching: find.byType(Container)).first,
);

void main() {
  const caption = 'Theo carries on once you approve.';

  for (final width in [320.0, 393.0]) {
    testWidgets('at $width pt, the caption first, then the buttons, Approve '
        'at the end', (tester) async {
      await _card(tester, width);
      expect(tester.takeException(), isNull);
      final words = tester.getRect(find.text(caption));
      await tester.ensureVisible(find.text('Approve'));
      await settle(tester);
      final dismiss = _button(tester, 'Dismiss');
      final approve = _button(tester, 'Approve');
      expect(dismiss.top, greaterThan(words.top));
      expect(approve.top, greaterThan(words.top));
      expect(approve.height, greaterThanOrEqualTo(44));
      expect(approve.right, lessThanOrEqualTo(width));
      // Approve ends the line, at the caption's end.
      expect(
        approve.right,
        closeTo(tester.getRect(find.text(caption)).right, 1),
      );
      if (width >= 393) {
        // Room for both: together on one line, Dismiss first.
        expect(dismiss.top, approve.top);
        expect(dismiss.right, lessThan(approve.left));
      }
    });
  }

  testWidgets('at the largest text on a small phone, nothing cut, the '
      'buttons stacked the whole width', (tester) async {
    await _card(tester, 320, scale: 3.1);
    await tester.ensureVisible(find.text('Approve'));
    await settle(tester);
    expect(tester.takeException(), isNull);
    final dismiss = _button(tester, 'Dismiss');
    final approve = _button(tester, 'Approve');
    expect(approve.height, greaterThanOrEqualTo(44));
    // Stacked as iOS stacks an alert's buttons: each the whole width,
    // Approve last.
    expect(approve.top, greaterThan(dismiss.bottom));
    expect(approve.left, dismiss.left);
    expect(approve.right, dismiss.right);
    expect(approve.right, closeTo(tester.getRect(find.text(caption)).right, 1));
  });

  for (final scale in [1.0, 3.1]) {
    testWidgets('a lone button stays at the end, at ${scale}x', (tester) async {
      await _card(tester, 320, scale: scale);
      await tester.ensureVisible(find.text('Copy'));
      await settle(tester);
      expect(tester.takeException(), isNull);
      final copy = _button(tester, 'Copy');
      final code = tester.getRect(find.text(_command));
      // Beside the command while it leaves it room, else under it.
      if (scale > 1) expect(copy.top, greaterThan(code.top));
      expect(copy.height, greaterThanOrEqualTo(44));
      expect(copy.right, lessThanOrEqualTo(320));
      // At the end of the line (where the caption's line ends), not its
      // start.
      expect(copy.right, closeTo(tester.getRect(find.text(caption)).right, 1));
    });
  }
}
