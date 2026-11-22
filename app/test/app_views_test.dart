import 'dart:async';
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/surface.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// The backend's end of app views, as the tests say: each open answers
/// [next] (or what [answer] makes), and what's asked is recorded.
class FakeAppViewTransport implements AppViewTransport {
  final opens =
      <({String appId, String view, String params, List<String> catalogs})>[];
  final actions = <(String, Map<String, Object?>)>[];
  final closes = <String>[];
  final events = StreamController<AppViewEvent>.broadcast(sync: true);
  AppSurfaceData Function(int open)? answer;
  RequestFailure? refuseOpen;
  RequestFailure? refuseAction;

  @override
  final ValueNotifier<bool> viewsConnected = ValueNotifier(true);

  @override
  Future<AppSurfaceData> openView({
    required String appId,
    required String view,
    required String params,
    required List<String> catalogs,
  }) async {
    opens.add((appId: appId, view: view, params: params, catalogs: catalogs));
    if (refuseOpen case final failure?) throw failure;
    return answer!(opens.length);
  }

  @override
  Future<void> actInView(String surfaceId, String message) async {
    actions.add((surfaceId, jsonDecode(message) as Map<String, Object?>));
    if (refuseAction case final failure?) throw failure;
  }

  @override
  Future<void> closeView(String surfaceId) async => closes.add(surfaceId);

  @override
  Stream<AppViewEvent> get viewEvents => events.stream;

  void update(int seq, Map<String, Object?> message) => events.add(
    AppSurfaceUpdated(
      AppViewUpdateData(
        surfaceId: 'charts-1',
        seq: seq,
        message: jsonEncode({'version': '1.0', ...message}),
      ),
    ),
  );
}

String _snapshot(List<Map<String, Object?>> components, [Object? data]) =>
    jsonEncode({
      'version': '1.0',
      'createSurface': {
        'surfaceId': 'charts-1',
        'catalogId': appViewCatalogV1,
        'components': components,
        'dataModel': data ?? <String, Object?>{},
      },
    });

AppSurfaceData _surface({
  AppViewState state = AppViewState.open,
  String snapshot = '',
  int seq = 0,
  String error = '',
}) => AppSurfaceData(
  surfaceId: 'charts-1',
  appId: 'charts',
  title: 'Charts',
  state: state,
  snapshot: snapshot,
  seq: seq,
  error: error,
);

/// A dashboard: a heading, a card listing runs from a template, a field,
/// a switch, chips, tabs and a Refresh that sends what was typed.
final _dashboard = _snapshot(
  [
    {
      'id': 'root',
      'component': 'Column',
      'children': [
        'title',
        'runs',
        'name',
        'live',
        'kinds',
        'tabs',
        'refresh',
        'done',
      ],
    },
    {
      'id': 'title',
      'component': 'Text',
      'text': {'@path': '/title'},
      'variant': 'heading',
    },
    {'id': 'runs', 'component': 'Card', 'child': 'run_list'},
    {
      'id': 'run_list',
      'component': 'List',
      'children': {'componentId': 'run', 'path': '/runs'},
    },
    {
      'id': 'run',
      'component': 'Row',
      'children': ['run_name', 'run_count'],
      'justify': 'spaceBetween',
    },
    {
      'id': 'run_name',
      'component': 'Text',
      'text': {'@path': 'name'},
    },
    {
      'id': 'run_count',
      'component': 'Text',
      'text': {'@path': 'count'},
      'variant': 'caption',
    },
    {
      'id': 'name',
      'component': 'TextField',
      'label': 'Dashboard name',
      'value': {'@path': '/form/name'},
    },
    {
      'id': 'live',
      'component': 'CheckBox',
      'label': 'Refresh by itself',
      'value': {'@path': '/form/live'},
    },
    {
      'id': 'kinds',
      'component': 'ChoicePicker',
      'label': 'Show',
      'options': [
        {'label': 'Runs', 'value': 'runs'},
        {'label': 'Tasks', 'value': 'tasks'},
      ],
      'value': {'@path': '/form/kinds'},
      'variant': 'multipleSelection',
      'displayStyle': 'chips',
    },
    {
      'id': 'tabs',
      'component': 'Tabs',
      'tabs': [
        {'title': 'Today', 'child': 'today'},
        {'title': 'Week', 'child': 'week'},
      ],
    },
    {'id': 'today', 'component': 'Text', 'text': 'TODAY'},
    {'id': 'week', 'component': 'Text', 'text': 'WEEK'},
    {
      'id': 'refresh',
      'component': 'Button',
      'child': 'refresh_label',
      'variant': 'primary',
      'action': {
        'event': {
          'name': 'save',
          'context': {
            'name': {'@path': '/form/name'},
            'live': {'@path': '/form/live'},
            'kinds': {'@path': '/form/kinds'},
            'from': 'dashboard',
          },
        },
      },
    },
    {'id': 'refresh_label', 'component': 'Text', 'text': 'Save'},
    {
      'id': 'done',
      'component': 'ProgressBar',
      'value': {'@path': '/done'},
      'label': 'Imported',
    },
  ],
  {
    'title': 'Runs today',
    'runs': [
      {'name': 'Rivet', 'count': 4},
      {'name': 'Kai', 'count': 2},
    ],
    'form': {
      'name': 'Mine',
      'live': false,
      'kinds': ['runs'],
    },
    'done': 0.5,
  },
);

void main() {
  group('JSON pointers', () {
    test('read, write and delete, escaped steps and lists', () {
      Object? root = {
        'a': {'b/c': 1, 'd~e': 2},
        'list': [10, 20],
      };
      expect(readAtPointer(root, '/a/b~1c'), 1);
      expect(readAtPointer(root, '/a/d~0e'), 2);
      expect(readAtPointer(root, '/list/1'), 20);
      expect(readAtPointer(root, '/list/9'), isNull);
      expect(readAtPointer(root, '/nothing/here'), isNull);
      root = setAtPointer(root, '/x/y', 'z');
      expect(readAtPointer(root, '/x/y'), 'z');
      root = setAtPointer(root, '/list/-', 30);
      expect(readAtPointer(root, '/list'), [10, 20, 30]);
      root = setAtPointer(root, '/a/b~1c', null);
      expect((readAtPointer(root, '/a')! as Map).containsKey('b/c'), isFalse);
      expect(setAtPointer(root, '/', {'new': true}), {'new': true});
    });
  });

  group('a surface', () {
    test('takes its snapshot, then each update in order: an old one is '
        'ignored, a skipped one asks for a new snapshot', () {
      final surface = AppSurfaceModel(_surface(snapshot: _dashboard, seq: 3));
      expect(surface.hasContent, isTrue);
      expect(surface.read('/title'), 'Runs today');

      AppViewUpdateData update(int seq, Map<String, Object?> body) =>
          AppViewUpdateData(
            surfaceId: 'charts-1',
            seq: seq,
            message: jsonEncode({'version': '1.0', ...body}),
          );
      expect(
        surface.apply(
          update(3, {
            'updateDataModel': {'path': '/title', 'value': 'old'},
          }),
        ),
        isTrue,
      );
      expect(surface.read('/title'), 'Runs today', reason: 'already had');
      expect(
        surface.apply(
          update(4, {
            'updateDataModel': {'path': '/title', 'value': 'Runs this week'},
          }),
        ),
        isTrue,
      );
      expect(surface.read('/title'), 'Runs this week');
      expect(
        surface.apply(
          update(5, {
            'updateComponents': {
              'components': [
                {'id': 'title', 'component': 'Text', 'text': 'Fixed'},
              ],
            },
          }),
        ),
        isTrue,
      );
      expect(surface.components['title']!['text'], 'Fixed');
      expect(surface.apply(update(7, {})), isFalse, reason: '6 was missed');
      expect(surface.seq, 5);

      surface.apply(update(6, {'deleteSurface': {}}));
      expect(surface.hasContent, isFalse);
      expect(surface.state, AppViewState.closed);
    });
  });

  group('a view', () {
    late FakeAppViewTransport transport;
    setUp(() {
      transport = FakeAppViewTransport()
        ..answer = (_) => _surface(snapshot: _dashboard);
    });

    Future<void> show(WidgetTester tester, {Size? size}) =>
        Harness(pb.Workspace()).show(
          tester,
          size: size ?? const Size(1200, 900),
          home: AppView(
            transport: transport,
            appId: 'charts',
            appName: 'Charts',
            view: 'dashboard',
            params: '{"id":"d1"}',
          ),
        );

    testWidgets('opens with this app\'s catalog and draws it: text from the '
        'data model, a template for each run, inputs, tabs', (tester) async {
      await show(tester);
      await tester.pump();
      expect(transport.opens.single.appId, 'charts');
      expect(transport.opens.single.view, 'dashboard');
      expect(transport.opens.single.params, '{"id":"d1"}');
      expect(transport.opens.single.catalogs, [appViewCatalogV1]);

      expect(find.text('Runs today'), findsOneWidget);
      expect(find.text('Rivet'), findsOneWidget);
      expect(find.text('Kai'), findsOneWidget);
      expect(find.text('4'), findsOneWidget);
      expect(find.text('Dashboard name'), findsOneWidget);
      // Framed as the app's, and its field says where what's typed goes.
      expect(find.text('From Charts'), findsOneWidget);
      expect(find.text('Charts gets what you type here.'), findsOneWidget);
      expect(find.text('TODAY'), findsOneWidget);
      expect(find.text('WEEK'), findsNothing);
      await tester.tap(find.text('Week'));
      await tester.pump();
      expect(find.text('WEEK'), findsOneWidget);

      // The app pushes a change: it shows, in order.
      transport.update(1, {
        'updateDataModel': {
          'path': '/runs/1',
          'value': {'name': 'June', 'count': 9},
        },
      });
      await tester.pump();
      expect(find.text('June'), findsOneWidget);
      expect(find.text('Kai'), findsNothing);
    });

    testWidgets("what's typed and chosen stays here until Save sends it, "
        'in the action\'s context', (tester) async {
      await show(tester);
      await tester.pump();
      await tester.enterText(find.byType(EditableText), 'Launch');
      await tester.tap(find.byType(Toggle));
      await tester.tap(find.text('Tasks'));
      await tester.pump();
      expect(transport.actions, isEmpty);

      await tester.tap(find.text('Save'));
      await tester.pump();
      final (surfaceId, message) = transport.actions.single;
      expect(surfaceId, 'charts-1');
      final action = message['action']! as Map<String, Object?>;
      expect(message['version'], '1.0');
      expect(action['name'], 'save');
      expect(action['surfaceId'], 'charts-1');
      expect(action['sourceComponentId'], 'refresh');
      expect(action['timestamp'], isA<String>());
      expect(action['context'], {
        'name': 'Launch',
        'live': true,
        'kinds': ['runs', 'tasks'],
        'from': 'dashboard',
      });
    });

    testWidgets('a missed update, or the connection back, opens it again for '
        'a fresh snapshot; leaving closes it', (tester) async {
      await show(tester);
      await tester.pump();
      transport.update(2, {
        'updateDataModel': {'path': '/title', 'value': 'skipped one'},
      });
      await tester.pump();
      expect(transport.opens, hasLength(2));
      expect(find.text('Runs today'), findsOneWidget);

      transport.viewsConnected.value = false;
      await tester.pump();
      transport.viewsConnected.value = true;
      await tester.pump();
      expect(transport.opens, hasLength(3));

      await tester.pumpWidget(const SizedBox());
      expect(transport.closes, ['charts-1']);
    });

    testWidgets('an action on a surface that went opens it again', (
      tester,
    ) async {
      await show(tester);
      await tester.pump();
      transport.refuseAction = const RequestFailure(
        'Not open.',
        code: pb.ErrorCode.ERROR_CODE_NOT_FOUND,
      );
      await tester.tap(find.text('Save'));
      await tester.pump();
      expect(transport.opens, hasLength(2));
    });

    testWidgets("its states in words: opening, not running, a view it can't "
        'show', (tester) async {
      transport.answer = (_) => _surface(state: AppViewState.opening);
      await show(tester);
      await tester.pump();
      expect(find.text('Opening…'), findsOneWidget);

      transport.events.add(
        AppSurfaceChanged(_surface(state: AppViewState.stopped)),
      );
      await tester.pump();
      expect(find.text("Charts isn't running"), findsOneWidget);
      expect(
        find.text('It opens again by itself when the app starts.'),
        findsOneWidget,
      );

      transport.events.add(AppSurfaceChanged(_surface(snapshot: _dashboard)));
      await tester.pump();
      expect(find.text('Runs today'), findsOneWidget);

      transport.events.add(
        AppSurfaceChanged(_surface(state: AppViewState.unsupported)),
      );
      await tester.pump();
      expect(find.text('This view needs a newer Claktory'), findsOneWidget);
    });

    testWidgets('failed: why, over its last good contents dimmed, and Open '
        'again', (tester) async {
      await show(tester);
      await tester.pump();
      transport.events.add(
        AppSurfaceChanged(
          _surface(
            state: AppViewState.failed,
            snapshot: _dashboard,
            error: 'It sent a component this version doesn’t know.',
          ),
        ),
      );
      await tester.pump();
      expect(find.text("Charts couldn't show this"), findsOneWidget);
      expect(
        find.text('It sent a component this version doesn’t know.'),
        findsOneWidget,
      );
      expect(find.text('Runs today'), findsOneWidget);
      // What stays takes no input.
      await tester.tap(find.text('Save'), warnIfMissed: false);
      await tester.pump();
      expect(transport.actions, isEmpty);
      await tester.tap(find.text('Open again'));
      await tester.pump();
      expect(transport.opens, hasLength(2));
    });

    testWidgets("an open the backend refused says why, with Try again", (
      tester,
    ) async {
      transport.refuseOpen = const RequestFailure('Charts isn’t installed.');
      await show(tester);
      await tester.pump();
      expect(find.text("Couldn't open this view"), findsOneWidget);
      expect(find.text('Charts isn’t installed.'), findsOneWidget);
      transport.refuseOpen = null;
      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(find.text('Runs today'), findsOneWidget);
    });

    testWidgets('on a phone at the largest text, it fits', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await show(tester, size: const Size(390, 844));
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(find.text('Runs today'), findsOneWidget);
    });

    testWidgets('a deep view still fits a phone, and a long template is cut '
        'short, saying how many more', (tester) async {
      final deep = [
        for (var i = 0; i < 30; i++)
          {
            'id': i == 0 ? 'root' : 'card$i',
            'component': 'Card',
            'child': i == 29 ? 'leaf' : 'card${i + 1}',
          },
        {
          'id': 'leaf',
          'component': 'List',
          'children': {'componentId': 'item', 'path': '/items'},
        },
        {
          'id': 'item',
          'component': 'Text',
          'text': {'@path': 'name'},
        },
      ];
      transport.answer = (_) => _surface(
        snapshot: _snapshot(deep, {
          'items': [
            for (var i = 0; i < 600; i++) {'name': 'Item $i'},
          ],
        }),
      );
      await show(tester, size: const Size(390, 844));
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(find.text('Item 0'), findsOneWidget);
      expect(find.text('Item 499', skipOffstage: false), findsOneWidget);
      expect(find.text('Item 500', skipOffstage: false), findsNothing);
      expect(find.text('and 100 more', skipOffstage: false), findsOneWidget);
    });

    testWidgets("a form's fields say once, above them, who gets what's "
        'typed', (tester) async {
      Map<String, Object?> field(String id, String label) => {
        'id': id,
        'component': 'TextField',
        'label': label,
        'value': {'@path': '/$id'},
      };
      transport.answer = (_) => _surface(
        snapshot: _snapshot(
          [
            {
              'id': 'root',
              'component': 'Column',
              'children': ['title', 'card', 'search_card'],
            },
            {'id': 'title', 'component': 'Text', 'text': 'Edit chart'},
            {'id': 'card', 'component': 'Card', 'child': 'form'},
            {
              'id': 'form',
              'component': 'Column',
              'children': ['heading', 'name', 'range', 'series'],
            },
            {'id': 'heading', 'component': 'Text', 'text': 'Chart'},
            field('name', 'Name'),
            {
              'id': 'range',
              'component': 'Row',
              'children': ['min', 'max'],
            },
            field('min', 'Least'),
            field('max', 'Most'),
            {
              'id': 'series',
              'component': 'List',
              'children': {'componentId': 'serie', 'path': '/series'},
            },
            field('serie', 'Series'),
            // Holds itself: counted once, never forever.
            {'id': 'loop', 'component': 'Card', 'child': 'loop'},
          ],
          {
            'series': ['a', 'b'],
          },
        ),
      );
      await show(tester);
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(find.text('Charts gets what you type below.'), findsOneWidget);
      expect(find.text('Charts gets what you type here.'), findsNothing);
      // Above the first field, under what comes before it.
      final note = tester.getTopLeft(
        find.text('Charts gets what you type below.'),
      );
      expect(note.dy, greaterThan(tester.getTopLeft(find.text('Chart')).dy));
      expect(note.dy, lessThan(tester.getTopLeft(find.text('Name')).dy));

      // A lone field says it under it: the dashboard's, above.
    });

    testWidgets("without the app's name, it's \"This app\", never a title the "
        'app chose', (tester) async {
      transport.answer = (_) => AppSurfaceData(
        surfaceId: 'charts-1',
        appId: 'charts',
        title: 'Claktory Security',
        state: AppViewState.open,
        snapshot: _dashboard,
      );
      await Harness(pb.Workspace()).show(
        tester,
        home: AppView(transport: transport, appId: 'charts', appName: ''),
      );
      await tester.pump();
      expect(find.text('From This app'), findsOneWidget);
      expect(find.textContaining('Claktory Security'), findsNothing);
    });
  });
}
