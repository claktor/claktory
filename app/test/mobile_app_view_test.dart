// An app's view (C1F) on an iPhone: listed with the agent's files under ⋯,
// it opens as a page like a file, framed with the app's name as Claktory
// knows it, never only the title the app chose.

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/panes.dart' show appKind;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show leaf, row, snapshot, split;

Future<Harness> _phone(WidgetTester tester, {String appId = 'charts'}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
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
  final harness = Harness(workspace);
  await harness.show(tester, size: const Size(393, 852));
  await pumpFrames(tester);
  return harness;
}

Future<void> _openFromExtensions(WidgetTester tester, String name) async {
  await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
  await pumpFrames(tester);
  await tester.tap(find.text(name));
  await pumpFrames(tester, const Duration(seconds: 1));
}

void main() {
  testWidgets("an app's view opens from ⋯ as a page, named for its app", (
    tester,
  ) async {
    final harness = await _phone(tester);
    await _openFromExtensions(tester, 'Charts app · Days');
    final view = tester.widget<AppView>(find.byType(AppView));
    expect(view.appId, 'charts');
    expect(view.appName, 'Charts');
    expect(view.view, 'day-by-day');
    expect(view.transport, same(harness.backend));
    expect(find.text("Can't show this kind of file"), findsNothing);
    // The page's header: the app's name first, then the view's title.
    expect(find.text('Charts app · Days'), findsWidgets);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.byType(AppView), findsNothing);
  });

  testWidgets('showPane pushes a pane already in the layout, as the app '
      "strip's chips do", (tester) async {
    final harness = await _phone(tester);
    final routes = tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes;
    expect(routes.showPane('nobody', 'v1'), isFalse);
    expect(routes.showPane('a1', 'v1'), isTrue);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.byType(AppView), findsOneWidget);
    expect(find.text('Charts app · Days'), findsWidgets);
    // The agent showed already: nothing asked of the backend.
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
  });

  testWidgets("an app Claktory doesn't know is This app", (tester) async {
    await _phone(tester, appId: 'gone');
    await _openFromExtensions(tester, 'This app · Days');
    expect(tester.widget<AppView>(find.byType(AppView)).appName, '');
    expect(find.text('This app · Days'), findsWidgets);
  });
}
