import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart' show Text, TextSpan;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show glyphButton, leaf, row, snapshot, split;

/// Builder's conversation beside a pane showing [appId]'s "day-by-day"
/// view, titled Days.
Future<Harness> _show(WidgetTester tester, {String appId = 'charts'}) async {
  final workspace = snapshot(
    layout: split('s1', row, [
      leaf('chat', 'chat'),
      ViewNode(
        id: 'v1',
        leaf: ViewLeaf(
          kind: appKind,
          title: 'Days',
          appId: appId,
          view: 'day-by-day',
          params: '{"month":"2026-11"}',
        ),
      ),
    ]),
  );
  workspace.apps.add(
    AppInfo(id: 'charts', name: 'Charts', icon: 'chart-column'),
  );
  final harness = Harness(workspace)..backend.appliesLayouts = true;
  await harness.show(tester);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('an app view leaf is its view, framed with the app’s name', (
    tester,
  ) async {
    final harness = await _show(tester);
    final view = tester.widget<AppView>(find.byType(AppView));
    expect(view.appId, 'charts');
    expect(view.appName, 'Charts');
    expect(view.view, 'day-by-day');
    expect(view.params, '{"month":"2026-11"}');
    expect(view.transport, same(harness.backend));
    // Its header names the app first, always: the view's title is the
    // app's own words.
    expect(
      find.descendant(
        of: find.byType(PaneHeader),
        matching: find.text('Charts app · Days'),
      ),
      findsOneWidget,
    );
    expect(
      paneTitle(ViewLeaf(kind: appKind, title: 'Days')),
      'This app · Days',
    );
    expect(paneTitle(ViewLeaf(kind: appKind)), 'This app');
  });

  testWidgets('an app Claktory doesn’t know gets no name the app chose', (
    tester,
  ) async {
    await _show(tester, appId: 'gone');
    expect(tester.widget<AppView>(find.byType(AppView)).appName, '');
    expect(
      find.descendant(
        of: find.byType(PaneHeader),
        matching: find.text('This app · Days'),
      ),
      findsOneWidget,
    );
  });
  testWidgets('beside the conversation the view frames itself; expanded, '
      'the header names the app, "app" muted, and the view goes without', (
    tester,
  ) async {
    await _show(tester);
    Finder named() => find.ancestor(
      of: find.byType(AppView),
      matching: find.byType(ChromeNamesApp),
    );
    expect(named(), findsNothing);

    await tester.tap(glyphButton('Expand Charts app · Days'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    expect(named(), findsOneWidget);
    final title = tester.widget<Text>(
      find.descendant(
        of: find.byType(PaneHeader),
        matching: find.text('Charts app · Days'),
      ),
    );
    final parts = (title.textSpan! as TextSpan).children!.cast<TextSpan>();
    expect(
      [for (final part in parts) part.text],
      ['Charts', ' app', ' · Days'],
    );
    expect(parts[1].style?.color, isNotNull, reason: '"app" muted');
    expect(parts[0].style?.color, isNull);

    await tester.tap(glyphButton('Collapse Charts app · Days').first);
    await settle(tester);
    expect(named(), findsNothing);
  });
}
