// Find in the Checklist, Memory, Settings and the panels (⌘F): each searches what it
// shows, brings the match into sight and marks it.
import 'package:factory_app/database.dart' show DatabaseView;
import 'package:factory_app/find.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/settings_shell.dart' show SettingsSearchField;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, now, showWith, workspace;
import 'harness.dart';
import 'database_test.dart' as database;
import 'memory_test.dart' as memory;
import 'monitors_test.dart' as monitors;

List<(String, bool)> _washes(WidgetTester tester) {
  final found = <(String, bool)>[];
  for (final widget in tester.widgetList<RichText>(find.byType(RichText))) {
    widget.text.visitChildren((span) {
      final background = span.style?.backgroundColor;
      if (span is TextSpan && background != null && span.text != null) {
        found.add((span.text!, background.a > 0.9));
      }
      return true;
    });
  }
  return found;
}

/// Clicks in [inside] (making its pane the one ⌘F finds in), opens find
/// and types [words].
Future<FindSession> _find(
  WidgetTester tester,
  Finder inside,
  String words,
) async {
  await tester.tapAt(tester.getTopLeft(inside) + const Offset(4, 4));
  await tester.pump();
  await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
  await pumpFrames(tester);
  final session = tester.widget<FindBar>(find.byType(FindBar)).session;
  await tester.enterText(
    find.byWidgetPredicate(
      (widget) => widget is EditableText && widget.focusNode == session.field,
    ),
    words,
  );
  await tester.pump(const Duration(milliseconds: 200));
  await pumpFrames(tester);
  return session;
}

void main() {
  testWidgets('the checklist: an item far up a long list is scrolled to and '
      'marked', (tester) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(
      ExtensionView(
        extensionId: 'checklist',
        agentId: 'a1',
        checklist: Checklist(
          items: [
            for (var i = 1; i <= 60; i++)
              ChecklistItem(
                id: i,
                text: i == 3 ? 'Find the needle' : 'Item $i',
                status: ChecklistStatus.CHECKLIST_STATUS_DONE,
                created: at(now.subtract(Duration(days: 60 - i))),
              ),
          ],
        ),
      ),
    );
    await settle(tester);
    await tester.pump();
    expect(find.text('Find the needle'), findsNothing);

    final session = await _find(tester, find.text('Item 60'), 'needle');
    expect(session.target.placeholder, 'Search this checklist');
    expect(session.target.countLabel, '1 of 1');
    await pumpFrames(tester);
    expect(find.text('Find the needle'), findsOneWidget);
    expect(_washes(tester), [('needle', true)]);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('memory: a match in a note opens it', (tester) async {
    final harness = Harness(memory.workspace());
    harness.backend.extensionView.apply(memory.memoryView(memory.memory()));
    await harness.show(tester);
    await settle(tester);
    expect(find.textContaining('release.sh'), findsNothing);

    final session = await _find(tester, find.text('Preferences'), 'release');
    expect(session.target.placeholder, 'Search this memory');
    // The note's summary, its words, and the change that wrote it.
    expect(session.target.countLabel, '1 of 3');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(session.target.countLabel, '2 of 3');
    expect(find.textContaining('release.sh', findRichText: true), findsWidgets);
    expect(_washes(tester), contains(('release', true)));
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  // Settings has a search of its own, always on screen (area 3, §4.3):
  // ⌘F goes there rather than to a find bar over the section.
  testWidgets('Settings: ⌘F goes to its search, which opens the section '
      'of what it finds', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1200, 700));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await pumpFrames(tester);
    final field = find.descendant(
      of: find.byType(SettingsSearchField),
      matching: find.byType(EditableText),
    );
    await tester.tap(inSettingsSection('profile', find.text('Profile')));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await pumpFrames(tester);
    expect(find.byType(FindBar), findsNothing);
    expect(tester.widget<EditableText>(field).focusNode.hasFocus, isTrue);
    await tester.enterText(field, 'disk');
    await pumpFrames(tester);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(
      inSettingsSection('resources', find.text('Resources')),
      findsOneWidget,
    );
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('a panel made of many widgets (the database): the words it '
      'shows, each match painted over', (tester) async {
    await database.show(
      tester,
      Database(
        tables: [
          for (var i = 0; i < 40; i++)
            DatabaseTable(name: i == 37 ? 'invoices' : 'table_$i'),
        ],
      ),
    );
    final row = find.byWidgetPredicate(
      (w) => w is Text && w.data == 'invoices',
      skipOffstage: false,
    );
    final bottom = tester.getBottomLeft(find.byType(DatabaseView)).dy;
    // Laid out (for find), but below the panel's edge to begin with.
    expect(tester.getTopLeft(row).dy, greaterThan(bottom));
    final session = await _find(tester, find.text('table_0'), 'invoices');
    expect(session.target.placeholder, 'Search the database');
    expect(session.target.countLabel, '1 of 1');
    await pumpFrames(tester);
    // Scrolled to: in sight now.
    // (The Text, not the find field that holds the same words.)
    expect(tester.getTopLeft(row).dy, lessThan(bottom));
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('monitors: every match in the words the panel shows', (
    tester,
  ) async {
    await monitors.show(tester, [
      monitors.monitor(1, 'Nightly build', when: 'every 24h'),
      monitors.monitor(2, 'Nightly tests', when: 'every 1h'),
      monitors.monitor(3, 'Inbox'),
    ]);
    final session = await _find(tester, find.text('Inbox'), 'nightly');
    expect(session.target.placeholder, 'Search monitors');
    expect(session.target.countLabel, '1 of 2');
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('in a narrow panel the bar fits inside it, and the list moves '
      'down under it, its first row in sight', (tester) async {
    await database.show(
      tester,
      Database(
        tables: [for (var i = 0; i < 5; i++) DatabaseTable(name: 'table_$i')],
      ),
    );
    final panel = tester.getRect(find.byType(DatabaseView));
    expect(panel.width, lessThan(480));
    final first = find.byWidgetPredicate(
      (w) => w is Text && w.data == 'table_0',
    );
    final before = tester.getTopLeft(first).dy;
    final session = await _find(tester, first, 'table_0');
    expect(session.target.countLabel, '1 of 1');
    final bar = tester.getRect(find.byType(FindBar));
    expect(bar.left, greaterThanOrEqualTo(panel.left));
    expect(bar.right, lessThanOrEqualTo(panel.right));
    final field = tester.getRect(
      find.byWidgetPredicate(
        (widget) => widget is EditableText && widget.focusNode == session.field,
      ),
    );
    expect(field.left, greaterThanOrEqualTo(panel.left));
    expect(field.width, greaterThan(60));
    // The rows start under the bar.
    expect(tester.getTopLeft(first).dy, greaterThan(bar.bottom));
    expect(tester.getTopLeft(first).dy, greaterThan(before));
    // Closed, they go back up.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(tester.getTopLeft(first).dy, before);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
