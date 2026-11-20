import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show showWith, workspace;
import 'folders_test.dart' as folders show snapshot;
import 'harness.dart';

/// Opens the place picker in Builder's open settings and says what the
/// place outside projects is called, then closes the picker.
Future<String> outsideLabel(WidgetTester tester) async {
  await tester.ensureVisible(find.text('Factory').last);
  await pumpFrames(tester);
  await tester.tap(find.text('Factory').last);
  await settle(tester);
  final label = find.textContaining('(no project)').evaluate().single.widget;
  await tester.sendKeyEvent(LogicalKeyboardKey.escape);
  await settle(tester);
  return (label as Text).data!;
}

/// A fresh snapshot, as the backend sends, with Builder's admin option [on].
Workspace withAdmin(bool on) {
  final snapshot = workspace();
  snapshot.agents[1].extensions[0].options['admin'] = OptionValue(on: on);
  return snapshot;
}

void main() {
  testWidgets('the place outside projects follows the admin option', (
    tester,
  ) async {
    final harness = await showWith(tester, withAdmin(false));
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    expect(await outsideLabel(tester), 'Workspace (no project)');

    // Turned on elsewhere (by Rivet, say) while the form is open.
    await harness.apply(tester, withAdmin(true));
    await settle(tester);
    expect(await outsideLabel(tester), 'Admin (no project)');

    // Turned off here, not saved yet.
    final admin = find.byWidgetPredicate(
      (widget) =>
          widget is Semantics &&
          widget.properties.label == 'Admin: can see and message every agent',
    );
    await tester.ensureVisible(admin.first);
    await pumpFrames(tester);
    await tester.tap(admin.first);
    await settle(tester);
    expect(await outsideLabel(tester), 'Workspace (no project)');
  });

  testWidgets('Save sends only what was changed in the form', (tester) async {
    final harness = await showWith(tester, withAdmin(false));
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    // Renamed elsewhere while the form is open.
    final renamed = withAdmin(false);
    renamed.agents[1].name = 'Maker';
    await harness.apply(tester, renamed);
    await settle(tester);

    final role = find.descendant(
      of: find.byWidgetPredicate(
        (w) =>
            w is TextField && w.placeholder == 'Such as Coordinator or Writer',
      ),
      matching: find.byType(EditableText),
    );
    await tester.ensureVisible(role);
    await tester.enterText(role, 'Builds things');
    await tester.pump();
    await tester.ensureVisible(find.text('Save'));
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.role, 'Builds things');
    expect(update.hasName(), isFalse, reason: "the other rename stands");
    expect(update.hasModel(), isFalse);
    expect(update.hasPrompt(), isFalse);
  });

  testWidgets("on a 375-point phone, a form's less common action goes on a "
      'line of its own', (tester) async {
    final harness = Harness(folders.snapshot());
    await harness.show(tester, size: const Size(375, 812));
    await pumpFrames(tester);
    await tester.tap(find.text('Factory').first);
    await pumpFrames(tester);

    await tester.ensureVisible(find.text('Delete project…'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull, reason: 'nothing overflows');
    final delete = tester.getRect(find.text('Delete project…'));
    final cancel = tester.getRect(find.text('Cancel'));
    final save = tester.getRect(find.text('Save'));
    expect(delete.top, greaterThan(cancel.bottom), reason: 'under them');
    expect(save.top, cancel.top, reason: 'Cancel and Save share a row');
    expect(save.right, lessThanOrEqualTo(375));
  });

  testWidgets("in a Mac window, it's on the buttons' row", (tester) async {
    final harness = Harness(folders.snapshot());
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Factory').first);
    await pumpFrames(tester);
    final delete = tester.getRect(find.text('Delete project…'));
    final cancel = tester.getRect(find.text('Cancel'));
    expect(delete.center.dy, closeTo(cancel.center.dy, 1));
    expect(delete.right, lessThan(cancel.left));
  });
}
