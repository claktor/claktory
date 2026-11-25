// A Button's menu (catalog v1's `menu`): opened over the page, never
// pushing it down; a dropdown on a Mac, a sheet on a phone; searchable
// past eight options; a choice sends its own action and closes it.
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/catalog.dart' show AppViewIcon;
import 'package:factory_app/app_views/menu.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';
import 'ipad/as_ipad.dart';

/// A page: the menu's button at the top, a line of words under it.
List<Map<String, Object?>> _page(int count, {int selected = 0}) => [
  {
    'id': 'root',
    'component': 'Column',
    'children': ['agent', 'note'],
  },
  {
    'id': 'agent',
    'component': 'Button',
    'child': 'agent_t',
    'variant': 'borderless',
    // For apps that can't open a menu: never sent by this one.
    'action': {
      'event': {
        'name': 'menu',
        'context': {'which': 'agent'},
      },
    },
    'menu': {
      'title': 'Agent',
      'options': [
        for (var i = 0; i < count; i++)
          {
            'label': i == 3 ? 'Zoë' : 'agent $i',
            if (i == selected) 'selected': true,
            'action': {
              'event': {
                'name': 'variable',
                'context': {'value': 'a$i'},
              },
            },
          },
      ],
    },
  },
  {'id': 'agent_t', 'component': 'Text', 'text': 'Agent: All'},
  {'id': 'note', 'component': 'Text', 'text': 'The page under it'},
];

Future<FakeAppViewTransport> _show(
  WidgetTester tester,
  List<Map<String, Object?>> components, {
  Size size = const Size(1200, 900),
}) async {
  final snapshot = jsonEncode({
    'version': '1.0',
    'createSurface': {
      'surfaceId': 'charts-1',
      'catalogId': appViewCatalogV1,
      'components': components,
      'dataModel': <String, Object?>{},
    },
  });
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'charts-1',
      appId: 'charts',
      title: 'Charts',
      state: AppViewState.open,
      snapshot: snapshot,
      seq: 0,
      error: '',
    );
  await Harness(pb.Workspace()).show(
    tester,
    size: size,
    home: AppView(transport: transport, appId: 'charts', appName: 'Charts'),
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
  return transport;
}

void _on(TargetPlatform platform) {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
}

Map<String, Object?> _sent(FakeAppViewTransport transport) =>
    transport.actions.single.$2['action']! as Map<String, Object?>;

void main() {
  group('on a Mac', () {
    testWidgets('a click opens a dropdown over the page, the chosen one '
        'checked; a choice sends its action and closes it', (tester) async {
      _on(TargetPlatform.macOS);
      final transport = await _show(tester, _page(4, selected: 1));
      final under = tester.getTopLeft(find.text('The page under it'));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsOneWidget);
      expect(find.text('agent 2'), findsOneWidget);
      // Over the page, not in it: nothing under the button moved.
      expect(tester.getTopLeft(find.text('The page under it')), under);
      expect(
        tester.getTopLeft(find.text('agent 0')).dy,
        greaterThan(tester.getBottomLeft(find.text('Agent: All')).dy),
      );
      // Few options: no search.
      expect(find.text('Search'), findsNothing);
      final checked = find.descendant(
        of: find.byType(AppMenuList),
        matching: find.bySemanticsLabel('agent 1'),
      );
      expect(checked, findsOneWidget);
      expect(
        tester.getSemantics(checked),
        matchesSemantics(
          label: 'agent 1',
          isButton: true,
          hasSelectedState: true,
          isSelected: true,
          hasTapAction: true,
        ),
      );
      expect(transport.actions, isEmpty, reason: 'its own action never goes');

      await tester.tap(find.text('agent 2'));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      final sent = _sent(transport);
      expect(sent['name'], 'variable');
      expect(sent['sourceComponentId'], 'agent');
      expect(sent['context'], {'value': 'a2'});
    });

    testWidgets('a click outside or Esc closes it, sending nothing', (
      tester,
    ) async {
      _on(TargetPlatform.macOS);
      final transport = await _show(tester, _page(4));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      await tester.tapAt(const Offset(1000, 800));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      // A second click on the button closes it too.
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      expect(transport.actions, isEmpty);
    });

    testWidgets('fifty options: a search field takes the keys at once, '
        'matching without case or accents; the arrows and Return choose', (
      tester,
    ) async {
      _on(TargetPlatform.macOS);
      final transport = await _show(tester, _page(50, selected: 40));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      // Opened at what's chosen.
      expect(
        tester
            .getRect(find.text('agent 40'))
            .overlaps(tester.getRect(find.byType(AppMenuList))),
        isTrue,
      );
      await tester.enterText(find.byType(EditableText), 'zoe');
      await tester.pumpAndSettle();
      expect(find.text('Zoë'), findsOneWidget);
      expect(find.text('agent 40'), findsNothing);
      await tester.enterText(find.byType(EditableText), 'agent 4');
      await tester.pumpAndSettle();
      // agent 4, agent 40 … agent 49.
      expect(find.text('agent 49'), findsOneWidget);
      expect(find.text('agent 39'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      expect(_sent(transport)['context'], {'value': 'a41'});
    });

    testWidgets('a click opens it with the search field taking the keys, '
        'the rows starting whole under it', (tester) async {
      _on(TargetPlatform.macOS);
      await _show(tester, _page(30, selected: 17));
      // Something on the page has the keys already, as in the app (the
      // button itself, after Tab): the field takes them all the same.
      Focus.of(tester.element(find.text('Agent: All'))).requestFocus();
      await tester.pump();
      expect(FocusManager.instance.primaryFocus, isNotNull);
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      final field = find.descendant(
        of: find.byType(AppMenuList),
        matching: find.byType(EditableText),
      );
      expect(
        tester.widget<EditableText>(field).focusNode.hasFocus,
        isTrue,
        reason: 'typing at once filters',
      );
      // Opened at the chosen one, the list's first row whole: never half
      // under the field.
      final list = find.descendant(
        of: find.byType(AppMenuList),
        matching: find.byType(SingleChildScrollView),
      );
      final top = tester.getRect(list).top;
      final rows = [
        for (var i = 0; i < 30; i++)
          if (find.text('agent $i').evaluate().isNotEmpty)
            tester.getRect(
              find
                  .ancestor(
                    of: find.text('agent $i'),
                    matching: find.byType(Semantics),
                  )
                  .first,
            ),
      ];
      expect(rows, isNotEmpty);
      for (final row in rows) {
        expect(
          row.top >= top - 0.5 || row.bottom <= top + 0.5,
          isTrue,
          reason: 'a row cut by the list\'s top: $row, the list at $top',
        );
      }
    });

    testWidgets('typing at once after a click filters', (tester) async {
      _on(TargetPlatform.macOS);
      await _show(tester, _page(12, selected: 7));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      final field = find.descendant(
        of: find.byType(AppMenuList),
        matching: find.byType(EditableText),
      );
      final state = tester.state<EditableTextState>(field);
      state.updateEditingValue(const TextEditingValue(text: 'zo'));
      await tester.pumpAndSettle();
      expect(find.text('Zoë'), findsOneWidget);
      expect(find.text('agent 7'), findsNothing);
    });

    testWidgets('nothing matching says so', (tester) async {
      _on(TargetPlatform.macOS);
      await _show(tester, _page(20));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(EditableText), 'nobody');
      await tester.pumpAndSettle();
      expect(find.text('No matches'), findsOneWidget);
    });
    testWidgets('headings over runs of a section, kept for what a search '
        'finds; an icon after the words', (tester) async {
      _on(TargetPlatform.macOS);
      final boards = [
        ('Costs', 'Starred', 'star'),
        ('Runs', 'Starred', 'star'),
        ('Agents', 'All dashboards', 'house'),
        ('Claktory', 'All dashboards', 'no-such-icon'),
        ('Costs', 'All dashboards', ''),
        ('Fidelity', 'All dashboards', ''),
        ('Runs', 'All dashboards', ''),
        ('Tokens', 'All dashboards', ''),
        ('Week', 'All dashboards', ''),
      ];
      final transport = await _show(tester, [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['board'],
        },
        {
          'id': 'board',
          'component': 'Button',
          'child': 'board_t',
          'menu': {
            'title': 'Dashboards',
            'options': [
              for (final (i, (label, section, icon)) in boards.indexed)
                {
                  'label': label,
                  'section': section,
                  if (icon.isNotEmpty) 'icon': icon,
                  if (i == 3) 'selected': true,
                  'action': {
                    'event': {
                      'name': 'board',
                      'context': {'i': i},
                    },
                  },
                },
            ],
          },
        },
        {'id': 'board_t', 'component': 'Text', 'text': 'Claktory'},
      ]);
      await tester.tap(find.text('Claktory'));
      await tester.pumpAndSettle();
      final menu = find.byType(AppMenuList);
      double top(String words) => tester
          .getTopLeft(
            find.descendant(of: menu, matching: find.text(words)).first,
          )
          .dy;
      expect(top('Starred'), lessThan(top('Costs')));
      expect(top('All dashboards'), greaterThan(top('Runs')));
      expect(top('All dashboards'), lessThan(top('Agents')));
      expect(
        tester.getSemantics(
          find.descendant(of: menu, matching: find.text('Starred')),
        ),
        matchesSemantics(label: 'Starred', isHeader: true),
      );
      // The house, and not the one this app can't draw.
      expect(
        find.descendant(of: menu, matching: find.byType(AppViewIcon)),
        findsOneWidget,
      );
      // A starred one's star filled (M4), not the outline icon.
      expect(
        find.descendant(
          of: menu,
          matching: find.byWidgetPredicate(
            (w) =>
                w is CustomPaint &&
                w.painter.runtimeType.toString() == '_FilledStar',
          ),
        ),
        findsNWidgets(2),
      );
      await tester.enterText(find.byType(EditableText), 'runs');
      await tester.pumpAndSettle();
      expect(
        find.descendant(of: menu, matching: find.text('Runs')),
        findsNWidgets(2),
      );
      expect(find.text('Starred'), findsOneWidget);
      expect(find.text('All dashboards'), findsOneWidget);
      await tester.enterText(find.byType(EditableText), 'week');
      await tester.pumpAndSettle();
      expect(find.text('Starred'), findsNothing, reason: 'none of it matches');
      expect(find.text('All dashboards'), findsOneWidget);
      await tester.tap(find.text('Week'));
      await tester.pumpAndSettle();
      expect(_sent(transport)['context'], {'i': 8});
    });
  });

  group('on a phone', () {
    testWidgets('a tap opens a sheet under its title, the page staying '
        'put; it opens at the chosen one, and a tap on one sends it', (
      tester,
    ) async {
      _on(TargetPlatform.iOS);
      final transport = await _show(
        tester,
        _page(50, selected: 40),
        size: const Size(390, 844),
      );
      final under = tester.getTopLeft(find.text('The page under it'));
      await tester.tap(find.text('Agent: All'));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsOneWidget);
      expect(find.text('Agent'), findsOneWidget, reason: "the sheet's title");
      expect(tester.getTopLeft(find.text('The page under it')), under);
      final list = tester.getRect(find.byType(AppMenuList));
      expect(list.bottom, greaterThan(844 / 2), reason: 'from the bottom');
      // Its search field, the keyboard not up until it's tapped.
      expect(find.byType(EditableText), findsOneWidget);
      expect(
        tester.testTextInput.isVisible,
        isFalse,
        reason: 'no keyboard by itself',
      );
      final chosen = tester.getRect(find.text('agent 40'));
      expect(chosen.top, greaterThan(list.top));
      expect(chosen.bottom, lessThan(list.bottom));
      // Each row a finger's target.
      expect(
        tester
            .getSize(
              find
                  .ancestor(
                    of: find.text('agent 41'),
                    matching: find.byType(Container),
                  )
                  .first,
            )
            .height,
        greaterThanOrEqualTo(44),
      );
      await tester.tap(find.text('agent 41'));
      await tester.pumpAndSettle();
      expect(find.byType(AppMenuList), findsNothing);
      expect(_sent(transport)['context'], {'value': 'a41'});
    });
  });

  testWidgets('on an iPad, a popover at the button: a tap on one sends '
      'it, a tap outside closes it', (tester) async {
    asIPad();
    final transport = await _show(
      tester,
      _page(50, selected: 40),
      size: const Size(1024, 1366),
    );
    final button = tester.getRect(find.text('Agent: All'));
    final under = tester.getTopLeft(find.text('The page under it'));
    await tester.tap(find.text('Agent: All'));
    await tester.pumpAndSettle();
    final list = tester.getRect(find.byType(AppMenuList));
    expect(
      list.width,
      lessThanOrEqualTo(320),
      reason: 'a popover, not a sheet',
    );
    expect(list.top, greaterThan(button.bottom), reason: 'under the button');
    expect(list.bottom, lessThan(1366 * 0.75));
    expect(tester.getTopLeft(find.text('The page under it')), under);
    final chosen = tester.getRect(find.text('agent 40'));
    expect(chosen.top, greaterThan(list.top));
    expect(chosen.bottom, lessThan(list.bottom));
    await tester.tapAt(const Offset(900, 1300));
    await tester.pumpAndSettle();
    expect(find.byType(AppMenuList), findsNothing);
    expect(transport.actions, isEmpty);
    await tester.tap(find.text('Agent: All'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('agent 41'));
    await tester.pumpAndSettle();
    expect(find.byType(AppMenuList), findsNothing);
    expect(_sent(transport)['context'], {'value': 'a41'});
  });
}
