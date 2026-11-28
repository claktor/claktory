// An app's view is framed as the app's ("From Calendar" over a card)
// unless Claktory's own header around it names the app ("Calendar app"):
// then the header carries that, stays put over the view and nothing the
// app draws covers it.
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';

const _header = Key('header');

/// A view of [count] lines, the first claiming to be Claktory's own.
String _snapshot(int count) => jsonEncode({
  'version': '1.0',
  'createSurface': {
    'surfaceId': 'cal-1',
    'catalogId': appViewCatalogV1,
    'components': [
      {
        'id': 'root',
        'component': 'Column',
        'children': [for (var i = 0; i < count; i++) 't$i'],
      },
      for (var i = 0; i < count; i++)
        {
          'id': 't$i',
          'component': 'Text',
          'text': i == 0 ? 'Claktory Settings' : 'Line $i',
          'variant': 'h1',
        },
    ],
  },
});

Future<void> _show(
  WidgetTester tester, {
  required bool named,
  bool flag = false,
  String appName = 'Calendar',
  int lines = 1,
}) async {
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'cal-1',
      appId: 'calendar',
      title: 'Calendar',
      state: AppViewState.open,
      snapshot: _snapshot(lines),
    );
  final view = AppView(
    transport: transport,
    appId: 'calendar',
    appName: appName,
    chromeNamesApp: flag,
  );
  await Harness(pb.Workspace()).show(
    tester,
    size: const Size(800, 600),
    home: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Stands for the expanded pane's or the phone page's header.
        const SizedBox(
          key: _header,
          height: 44,
          child: Center(child: Text('Calendar app')),
        ),
        Expanded(child: named ? ChromeNamesApp(child: view) : view),
      ],
    ),
  );
  await tester.pump();
}

/// The hairline card around the view.
Finder _card() => find.byWidgetPredicate(
  (w) =>
      w is Container &&
      w.decoration is BoxDecoration &&
      (w.decoration! as BoxDecoration).border is Border,
);

void main() {
  testWidgets('beside a conversation: "From Calendar" over the view, in a '
      'card; the name with what would hide or reorder it by its code', (
    tester,
  ) async {
    await _show(tester, named: false);
    expect(find.text('From Calendar'), findsOneWidget);
    expect(_card(), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await _show(tester, named: false, appName: 'Calen\u202Edar');
    expect(find.text(r'From Calen\u{202E}dar'), findsOneWidget);
  });

  testWidgets("under a header that names the app, by its scope or AppView's "
      'own flag: neither the line nor the card', (tester) async {
    await _show(tester, named: true);
    expect(find.text('Claktory Settings'), findsOneWidget);
    expect(find.textContaining('From '), findsNothing);
    expect(_card(), findsNothing);
    await tester.pumpWidget(const SizedBox());
    await _show(tester, named: false, flag: true);
    expect(find.textContaining('From '), findsNothing);
    expect(_card(), findsNothing);
  });

  testWidgets('the header stays over the view: what the app draws at its top '
      'and scrolls never covers it', (tester) async {
    await _show(tester, named: true, lines: 40);
    final header = tester.getRect(find.byKey(_header));
    expect(
      tester.getRect(find.text('Claktory Settings')).top,
      greaterThanOrEqualTo(header.bottom),
    );
    await tester.drag(find.text('Line 3'), const Offset(0, -300));
    await tester.pump();
    expect(
      tester.getRect(find.text('Claktory Settings')).top,
      lessThan(header.bottom),
      reason: 'scrolled up under where the header is',
    );
    expect(tester.getRect(find.byKey(_header)), header);
    expect(find.text('Calendar app').hitTestable(), findsOneWidget);
    expect(find.text('Claktory Settings').hitTestable(), findsNothing);
  });
}
