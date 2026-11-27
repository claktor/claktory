// Settings on an iPad (line 128):
// wide, its index rows are a finger tall (44 pt) with the pointer's hover;
// ⌘F is "Search Settings" in the system's menu (hold ⌘) and puts the keys
// in its search field; ↑/↓ in the index step through the sections, after a
// row's tapped too; Esc clears the search, then closes Settings.
import 'package:factory_app/ipad/commands.dart' show workbenchCommands;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/ipad/columns.dart' show WorkbenchTier;
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show snapshot;
import 'as_ipad.dart';

Future<(Harness, WorkbenchFocus)> _settings(
  WidgetTester tester, {
  Size size = const Size(1366, 1024),
}) async {
  asIPad();
  final harness = Harness(snapshot());
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  // Settings as the page shown (beside the conversation it's a pane).
  harness.page.value = AppPage.settings;
  await settle(tester);
  expect(find.byType(SettingsView), findsOneWidget);
  return (harness, focus);
}

Finder _row(String id) => find.byKey(ValueKey('settings-row:$id'));

/// The section shown: its title in the section's own heading.
String _shownTitle(WidgetTester tester) {
  final rows = tester.widgetList<Semantics>(
    find.descendant(
      of: find.byType(SettingsView),
      matching: find.byWidgetPredicate(
        (w) => w is Semantics && w.properties.selected == true,
      ),
    ),
  );
  expect(rows, hasLength(1));
  final row = find
      .ancestor(
        of: find.byWidget(rows.single),
        matching: find.byWidgetPredicate(
          (w) =>
              w.key is ValueKey<String> &&
              (w.key! as ValueKey<String>).value.startsWith('settings-row:'),
        ),
      )
      .first;
  return (tester.widget(row).key! as ValueKey<String>).value.substring(
    'settings-row:'.length,
  );
}

void main() {
  for (final size in const [Size(1366, 1024), Size(688, 1032)]) {
    testWidgets('wide at ${size.width.toInt()}: the index\'s rows are a '
        'finger tall, with the pointer\'s hover', (tester) async {
      await _settings(tester, size: size);
      final rows = find.byWidgetPredicate(
        (w) =>
            w.key is ValueKey<String> &&
            (w.key! as ValueKey<String>).value.startsWith('settings-row:'),
      );
      expect(rows, findsWidgets);
      for (final row in rows.evaluate()) {
        expect(
          (row.renderObject! as RenderBox).size.height,
          greaterThanOrEqualTo(44),
        );
      }
      // The pointer over a row: hover, as the sidebar's rows.
      Color? fill(Finder row) {
        final boxes = tester.widgetList<Container>(
          find.descendant(of: row, matching: find.byType(Container)),
        );
        for (final box in boxes) {
          final decoration = box.decoration;
          if (decoration is BoxDecoration) return decoration.color;
        }
        return null;
      }

      final before = fill(_row('devices'));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      addTearDown(mouse.removePointer);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(_row('devices')));
      await tester.pump();
      expect(fill(_row('devices')), isNot(before));
    });
  }

  testWidgets('⌘F is Search Settings in the system\'s menu, and puts the '
      'keys in its search', (tester) async {
    final (_, focus) = await _settings(tester);
    final command = workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: null,
    ).singleWhere((command) => command.id == 'find');
    expect(command.title, 'Search Settings');
    expect(command.enabled, isTrue);
    command.run!();
    await tester.pump();
    final search = FocusManager.instance.primaryFocus;
    expect(search?.debugLabel, 'Search Settings');
  });

  testWidgets('↑/↓ step through the sections after a row\'s tapped; Esc '
      'clears the search, then closes Settings', (tester) async {
    final (harness, _) = await _settings(tester);
    await tester.tap(_row('devices'));
    await settle(tester);
    expect(_shownTitle(tester), 'devices');
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await settle(tester);
    final after = _shownTitle(tester);
    expect(after, isNot('devices'));
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await settle(tester);
    expect(_shownTitle(tester), 'devices');

    await tester.enterText(find.byType(EditableText).first, 'zoom');
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(
      tester
          .widget<EditableText>(find.byType(EditableText).first)
          .controller
          .text,
      '',
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('after a tap in the index, a field in the section still takes '
      'the keys when tapped (Polnareff)', (tester) async {
    await _settings(tester);
    await tester.tap(_row('profile'));
    await settle(tester);
    // The section's first field, after the index's search.
    final field = find.byType(EditableText).at(1);
    await tester.tap(field);
    await settle(tester);
    expect(
      FocusManager.instance.primaryFocus,
      tester.widget<EditableText>(field).focusNode,
    );
  });
}
