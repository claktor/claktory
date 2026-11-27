import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/mobile/drawer.dart';
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
    await unfoldExtensions(tester);
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
    // A phone's project, in the drawer, offers its settings under More.
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Factory options'));
    await pumpFrames(tester);
    await tester.tap(find.text('Project settings…'));
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
    await tester.tap(find.text('Project settings…'));
    await pumpFrames(tester);
    final delete = tester.getRect(find.text('Delete project…'));
    final cancel = tester.getRect(find.text('Cancel'));
    expect(delete.center.dy, closeTo(cancel.center.dy, 1));
    expect(delete.right, lessThan(cancel.left));
  });

  testWidgets("agent settings: Rivet's line on top, the extensions folded "
      "(Lucy's agent view §6)", (tester) async {
    final harness = await showWith(tester, workspace()..highlights = true);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    expect(find.text('Or ask Rivet to change Builder'), findsOneWidget);
    // Above the model.
    expect(
      tester.getTopLeft(find.text('Or ask Rivet to change Builder')).dy,
      lessThan(tester.getTopLeft(find.text('Model')).dy),
    );
    // Messaging and Checklist are on: folded to how many, and Change….
    expect(find.text('2 on ·'), findsOneWidget);
    expect(find.text('Other agents can message it'), findsNothing);
    await unfoldExtensions(tester);
    expect(find.text('2 on ·'), findsNothing);
    expect(find.text('Other agents can message it'), findsOneWidget);

    // Unchanged, it goes straight to Rivet, its message box started.
    await tester.ensureVisible(find.text('Or ask Rivet to change Builder'));
    await pumpFrames(tester);
    await tester.tap(find.text('Or ask Rivet to change Builder'));
    await pumpFrames(tester);
    expect(find.text('Agent settings'), findsNothing);
    expect(harness.drafts.of('rivet'), 'Change Builder: ');
    expect(
      harness.backend.sent.where((m) => m.hasSelectAgent()).single
          .selectAgent
          .agentId,
      'rivet',
    );
    expect(find.text('Tell Rivet what to change'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasUpdateAgent()), isEmpty);
    // The glow and its words run out.
    await tester.pump(const Duration(seconds: 6));
  });

  testWidgets('with changes, asking Rivet first asks whether to save them', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace());
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    final role = find.descendant(
      of: find.byWidgetPredicate(
        (w) =>
            w is TextField && w.placeholder == 'Such as Coordinator or Writer',
      ),
      matching: find.byType(EditableText),
    );
    await tester.enterText(role, 'Builds things');
    await tester.pump();
    await tester.tap(find.text('Or ask Rivet to change Builder'));
    await pumpFrames(tester);
    expect(find.text('Save your changes to Builder?'), findsOneWidget);
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    expect(harness.last.hasSelectAgent(), isTrue);
    final update = harness.backend.sent
        .where((m) => m.hasUpdateAgent())
        .single
        .updateAgent;
    expect(update.role, 'Builds things');
    expect(harness.drafts.of('rivet'), 'Change Builder: ');
    expect(find.text('Agent settings'), findsNothing);

    // Discarded: nothing saved, still to Rivet.
    final other = await showWith(tester, workspace());
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await tester.enterText(role, 'Builds things');
    await tester.pump();
    await tester.tap(find.text('Or ask Rivet to change Builder'));
    await pumpFrames(tester);
    await tester.tap(find.text('Discard'));
    await pumpFrames(tester);
    expect(other.backend.sent.where((m) => m.hasUpdateAgent()), isEmpty);
    expect(other.drafts.of('rivet'), 'Change Builder: ');
  });

  testWidgets("Rivet's own settings don't offer to ask Rivet", (tester) async {
    await showWith(tester, workspace()..selectedAgentId = 'rivet');
    await tester.tap(find.text('Rivet').last);
    await pumpFrames(tester);
    expect(find.text('Agent settings'), findsOneWidget);
    expect(find.textContaining('Or ask Rivet'), findsNothing);
  });

  testWidgets('a long prompt folds to six lines, with Show all', (
    tester,
  ) async {
    final snapshot = workspace();
    snapshot.agents[1].prompt = [
      for (var line = 1; line <= 12; line++) 'Line $line of the prompt.',
    ].join('\n');
    await showWith(tester, snapshot);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    expect(find.text('Show all'), findsOneWidget);
    await tester.ensureVisible(find.text('Show all'));
    await pumpFrames(tester);
    await tester.tap(find.text('Show all'));
    await pumpFrames(tester);
    expect(find.text('Show all'), findsNothing);
  });

  testWidgets("a short prompt doesn't fold", (tester) async {
    final short = workspace();
    short.agents[1].prompt = 'Build things.';
    await showWith(tester, short);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    expect(find.text('Show all'), findsNothing);
  });
}
