import 'package:factory_app/changelog.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'focus_test.dart' show focusedOn;
import 'harness.dart';

ChangelogVersion version(
  String name,
  List<(ChangelogKind, String)> items, {
  bool unreleased = false,
}) => ChangelogVersion(
  version: name,
  released: unreleased
      ? Int64.ZERO
      : Int64(DateTime(2026, 11, 18, 12).millisecondsSinceEpoch),
  unreleased: unreleased,
  items: [
    for (final (kind, text) in items) ChangelogItem(kind: kind, text: text),
  ],
);

const added = ChangelogKind.CHANGELOG_KIND_ADDED;
const fixed = ChangelogKind.CHANGELOG_KIND_FIXED;
const other = ChangelogKind.CHANGELOG_KIND_OTHER;

/// Factory runs 2026.11.18-2054; 2026.11.18-2123 is offered.
Workspace offering({bool unreleased = false, bool latest = true}) =>
    focusedOn(
        '',
        version: '2026.11.18-2054',
        latest: latest ? '2026.11.18-2123' : '',
      )
      ..changelog.addAll([
        if (unreleased)
          version('', [(added, 'Rotating hints')], unreleased: true),
        if (latest)
          version('2026.11.18-2123', [
            (added, 'Clickable bubbles'),
            (fixed, 'A sidebar glitch'),
          ]),
        version('2026.11.18-2054', [(added, 'The Memory extension')]),
        version('2026.11.18-1948', [(other, 'First release')]),
      ]);

Future<Harness> show(WidgetTester tester, Workspace workspace) async {
  final harness = Harness(workspace)..appVersion = '2026.11.18-2054';
  await harness.show(tester);
  return harness;
}

Finder inPage(String text) =>
    find.descendant(of: find.byType(ChangelogView), matching: find.text(text));

void main() {
  test('the installed version is the newer of backend and app', () {
    final workspace = Workspace(version: '2026.11.18-2054');
    expect(installedVersion(workspace, 'dev'), '2026.11.18-2054');
    expect(installedVersion(workspace, '2026.11.18-2123'), '2026.11.18-2123');
    expect(installedVersion(workspace, '2026.11.18-1948'), '2026.11.18-2054');
    final offered = offering(unreleased: true);
    expect(
      [for (final v in newVersions(offered, 'dev')) v.version],
      ['2026.11.18-2123'],
    );
  });

  testWidgets("What's new shows every version on a timeline", (tester) async {
    final harness = await show(tester, offering(unreleased: true));
    // The sidebar's button (the banner has one too).
    await tester.tap(
      find.byWidgetPredicate(
        (widget) => widget is GlyphButton && widget.label == "What's new",
      ),
    );
    await settle(tester);
    expect(harness.page.value, AppPage.changelog);

    expect(inPage('Unreleased'), findsOneWidget);
    expect(inPage('Not released yet'), findsOneWidget);
    expect(inPage('2026.11.18-2123'), findsOneWidget);
    expect(inPage('Update to get it'), findsOneWidget);
    expect(inPage('2026.11.18-2054'), findsOneWidget);
    expect(inPage('This version'), findsOneWidget);
    expect(inPage('2026.11.18-1948'), findsOneWidget);
    // Each change under its kind; a line without one has none.
    expect(inPage('Clickable bubbles'), findsOneWidget);
    expect(inPage('Fixed'), findsOneWidget);
    expect(inPage('Added'), findsNWidgets(3));
    expect(inPage('First release'), findsOneWidget);
    double top(String text) => tester.getTopLeft(inPage(text)).dy;
    expect(top('Unreleased'), lessThan(top('2026.11.18-2123')));
    expect(top('Clickable bubbles'), lessThan(top('2026.11.18-2054')));

    // Esc goes back to the agent.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(ChangelogView), findsNothing);
  });

  testWidgets('without a changelog it says so', (tester) async {
    final harness = await show(tester, focusedOn(''));
    harness.page.value = AppPage.changelog;
    await settle(tester);
    expect(inPage("Claktory hasn't published a changelog yet."), findsOneWidget);
  });

  testWidgets("the Update banner offers What's new", (tester) async {
    final harness = await show(tester, offering());
    expect(find.text('Claktory 2026.11.18-2123 is ready.'), findsOneWidget);
    await tester.tap(find.text("What's new"));
    await settle(tester);
    expect(harness.page.value, AppPage.changelog);
    expect(inPage('Update to get it'), findsOneWidget);
  });

  testWidgets("no What's new on the banner when the changelog doesn't say", (
    tester,
  ) async {
    final workspace = offering()
      ..changelog.removeWhere((v) => v.version == '2026.11.18-2123');
    await show(tester, workspace);
    expect(find.text('Claktory 2026.11.18-2123 is ready.'), findsOneWidget);
    expect(find.text("What's new"), findsNothing);
  });
}
