// A question card's decision on a phone: the field, then the thumb row, Dismiss at
// the left and Approve at the right, wider, stacked at the largest text; and a lone button (Copy, beside a command) at the end too, a
// finger's size, at any text size.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

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

/// The card's field, whose right edge the row ends at.
Rect _field(WidgetTester tester) =>
    tester.getRect(find.byType(EditableText).first);

void main() {
  // Real widths: the thumb row stacks only when the labels can't fit.
  setUpAll(loadGeist);

  for (final width in [320.0, 393.0]) {
    testWidgets('at $width pt, the field first, then the thumb row: Dismiss '
        'at the left, Approve at the right and wider', (tester) async {
      await _card(tester, width);
      expect(tester.takeException(), isNull);
      await tester.ensureVisible(find.text('Approve'));
      await settle(tester);
      final field = _field(tester);
      final dismiss = _button(tester, 'Dismiss');
      final approve = _button(tester, 'Approve');
      expect(dismiss.top, greaterThan(field.top));
      expect(approve.height, greaterThanOrEqualTo(44));
      expect(approve.right, lessThanOrEqualTo(width));
      // One row, the no first, the primary wider.
      expect(dismiss.top, approve.top);
      expect(dismiss.right, lessThan(approve.left));
      expect(approve.width, greaterThan(dismiss.width));
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
      // At the end of the line (where the card's row ends), not its
      // start.
      await tester.ensureVisible(find.text('Approve'));
      await settle(tester);
      expect(
        tester.getRect(find.text('Copy')).center.dx,
        greaterThan(tester.getRect(find.text(_command)).left),
      );
      expect(copy.right, closeTo(_button(tester, 'Approve').right, 1));
    });
  }
}
