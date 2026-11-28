// The catalog's Calendar (Lucy's ux/calendar.md): week, day and agenda,
// the all-day band, overlaps side by side, the now line, the details with
// the one button to an entry's place, and every move sent to the app.
import 'dart:async';
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/calendar.dart';
import 'package:factory_app/app_views/charts.dart' show AppTable;
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/open_place.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show SemanticsAction;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';

final calendarWorkspace = pb.Workspace()
  ..agents.addAll([
    pb.Agent(id: 'pericolo', name: 'Pericolo', role: 'DevOps', color: 'teal'),
    pb.Agent(id: 'giorno', name: 'Giorno', role: 'Backend', color: 'pink'),
    pb.Agent(id: 'lucy', name: 'Lucy', role: 'UX Designer', color: 'blue'),
    pb.Agent(id: 'fugo', name: 'Fugo', role: 'Security', color: 'red'),
  ]);

/// Thursday 3 December 2026, 14:20, as the calendar's now.
final calendarNow = DateTime(2026, 12, 3, 14, 20);

int calendarAt(int day, int hour, [int minute = 0]) =>
    DateTime(2026, 12, day, hour, minute).millisecondsSinceEpoch;

int calendarUtcDay(int day) =>
    DateTime.utc(2026, 12, day).millisecondsSinceEpoch;

const calendarColumns = [
  'id',
  'start',
  'end',
  'title',
  'agent',
  'color',
  'allDay',
  'kind',
  'repeat',
  'detail',
  'open',
  'live',
];

List<Object?> calendarRow({
  required String id,
  required int start,
  int? end,
  required String title,
  String agent = 'pericolo',
  bool allDay = false,
  String kind = 'event',
  String repeat = '',
  String detail = '',
  String? open,
  bool live = false,
}) => [
  id,
  start,
  end,
  title,
  agent,
  '',
  allDay,
  kind,
  repeat,
  detail,
  open,
  live,
];

/// A week like Lucy's mock's.
final calendarRows = [
  calendarRow(
    id: 'logs',
    start: DateTime.utc(2026, 11, 30).millisecondsSinceEpoch,
    end: calendarUtcDay(7),
    title: 'Logs',
    agent: 'fugo',
    allDay: true,
    kind: 'monitor',
    repeat: 'every 5 min',
  ),
  calendarRow(
    id: 'release',
    start: calendarUtcDay(3),
    title: 'v0.1.26 release',
    allDay: true,
  ),
  calendarRow(
    id: 'build',
    start: calendarAt(2, 10),
    end: calendarAt(2, 11, 42),
    title: 'Release build',
    kind: 'task',
    open: 'task:pericolo:task-3',
  ),
  calendarRow(
    id: 'cargo',
    start: calendarAt(3, 13, 10),
    title: 'cargo test, full suite',
    agent: 'giorno',
    kind: 'task',
    live: true,
  ),
  calendarRow(
    id: 'working',
    start: calendarAt(3, 13, 40),
    title: 'Working',
    agent: 'fugo',
    kind: 'run',
    live: true,
  ),
  calendarRow(
    id: 'qa',
    start: calendarAt(3, 15),
    end: calendarAt(3, 16, 30),
    title: 'QA: focus mode on Mac',
    agent: 'lucy',
  ),
  calendarRow(
    id: 'check',
    start: calendarAt(3, 15, 30),
    title: '#18 History video, check again',
    agent: 'lucy',
    kind: 'item',
    detail: "From Lucy's checklist, waiting on Formaggio.",
    open: 'item:lucy:18',
  ),
  calendarRow(
    id: 'fired',
    start: calendarAt(3, 12),
    title: 'Production logs',
    kind: 'fire',
  ),
  calendarRow(
    id: 'logs18',
    start: calendarAt(3, 18),
    title: 'Production logs',
    kind: 'monitor',
    repeat: 'every 6 hours',
    detail: "From Pericolo's monitor. Last ran at 12:00.",
    open: 'monitor:pericolo:m3',
  ),
  calendarRow(
    id: 'sweep',
    start: calendarAt(4, 9),
    title: 'Morning sweep',
    kind: 'monitor',
    repeat: 'weekdays at 9:00',
  ),
];

/// The app's tools on the bar, as the Calendar app sends them: Agent and
/// Show menus, then ⋯.
final calendarTools = <Map<String, Object?>>[
  ..._menuButton('agent_menu', 'Agent: All', ['All agents', 'Fugo', 'Giorno']),
  ..._menuButton('show_menu', 'Show: All', ['All', 'Monitors', 'Tasks']),
  ..._menuButton('more_menu', null, ['Week starts on Monday']),
];

List<Map<String, Object?>> _menuButton(
  String id,
  String? words,
  List<String> options,
) => [
  {
    'id': id,
    'component': 'Button',
    'child': '${id}_c',
    'variant': 'borderless',
    'action': {
      'event': {
        'name': 'menu',
        'context': {'which': id},
      },
    },
    'menu': {
      'options': [
        for (final (index, label) in options.indexed)
          {
            'label': label,
            'selected': index == 0,
            'action': {
              'event': {
                'name': 'choose',
                'context': {'which': id, 'value': label},
              },
            },
          },
      ],
    },
  },
  {
    'id': '${id}_c',
    'component': 'List',
    'direction': 'horizontal',
    'align': 'center',
    'children': words == null ? ['${id}_l'] : ['${id}_t', '${id}_i'],
  },
  if (words == null)
    {'id': '${id}_l', 'component': 'Icon', 'name': 'ellipsis'}
  else ...[
    {'id': '${id}_t', 'component': 'Text', 'text': words, 'variant': 'body'},
    {'id': '${id}_i', 'component': 'Icon', 'name': 'chevron-down'},
  ],
];

Map<String, Object?> calendarView({
  String view = 'week',
  Map<String, Object?> more = const {},
}) => {
  'id': 'cal',
  'component': 'Calendar',
  'data': {'@path': '/rows'},
  'view': view,
  'anchor': calendarAt(3, 12),
  'weekStart': 'monday',
  'action': {
    'event': {
      'name': 'calendar',
      'context': {'board': 'main'},
    },
  },
  ...more,
};

/// What the open hook was asked to do.
final _opened = <OpenTarget>[];

Future<FakeAppViewTransport> _show(
  WidgetTester tester,
  Map<String, Object?> calendar, {
  Size size = const Size(1200, 900),
  List<List<Object?>>? rows,
  Future<String?> Function(OpenTarget)? gone,
}) async {
  final snapshot = jsonEncode({
    'version': '1.0',
    'createSurface': {
      'surfaceId': 'calendar-1',
      'catalogId': appViewCatalogV1,
      'components': [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['cal'],
        },
        calendar,
      ],
      'dataModel': {
        'rows': {'columns': calendarColumns, 'rows': rows ?? calendarRows},
      },
    },
  });
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'calendar-1',
      appId: 'calendar',
      title: 'Calendar',
      state: AppViewState.open,
      snapshot: snapshot,
      seq: 0,
      error: '',
    );
  await Harness(calendarWorkspace).show(
    tester,
    size: size,
    home: OpenPlace(
      gone: gone ?? (_) async => null,
      open: (_, target) => _opened.add(target),
      child: AppView(
        transport: transport,
        appId: 'calendar',
        appName: 'Calendar',
      ),
    ),
  );
  await tester.pump();
  await tester.pump(Motion.base);
  return transport;
}

/// The moves sent to the app: (anchor, view), with the app's own context.
List<(DateTime, String)> _moves(FakeAppViewTransport transport) => [
  for (final (_, message) in transport.actions)
    if (message['action'] case {
      'name': 'calendar',
      'context': {
        'board': 'main',
        'anchor': final int anchor,
        'view': final String view,
      },
    })
      (DateTime.fromMillisecondsSinceEpoch(anchor), view),
];

Finder _entry(String title) => find.byWidgetPredicate(
  (w) => w is Semantics && (w.properties.label ?? '').startsWith('$title, '),
);

void main() {
  setUp(() {
    AppCalendar.clock = () => calendarNow;
    _opened.clear();
  });
  tearDown(() => AppCalendar.clock = DateTime.now);

  group('reading rows', () {
    test('entries by their columns, all-day ones on their UTC dates, the '
        'others in local time; rows without a start or a title left out', () {
      final entries = calendarEntries(
        AppTable(calendarColumns, [
          ...calendarRows,
          calendarRow(id: 'x', start: -1, title: 'Bad start'),
          calendarRow(id: 'y', start: calendarAt(3, 9), title: ''),
        ]),
      );
      expect(entries, hasLength(calendarRows.length));
      final release = entries.firstWhere((e) => e.id == 'release');
      expect(release.start, DateTime(2026, 12, 3));
      expect(release.end, DateTime(2026, 12, 4), reason: 'one day');
      final cargo = entries.firstWhere((e) => e.id == 'cargo');
      expect(cargo.live, isTrue);
      expect(cargo.end, isNull);
      expect(cargo.until(calendarNow), calendarNow);
      final logs = entries.firstWhere((e) => e.id == 'logs18');
      expect(logs.point, isTrue);
      expect(logs.open, isA<OpenMonitor>());
    });

    test("agents' words: what would hide or reorder them by its code, a "
        'title on one line, the details with their lines', () {
      final entry = calendarEntries(
        AppTable(calendarColumns, [
          calendarRow(
            id: 'x',
            start: calendarAt(3, 9),
            title: 'Invoice\u202Efdp.exe\nsecond',
            repeat: 'every\n5 min\u200B',
            detail: 'One.\nTwo.\u2066',
          ),
        ]),
      ).single;
      expect(entry.title, r'Invoice\u{202E}fdp.exe second');
      expect(entry.repeat, r'every 5 min\u{200B}');
      expect(entry.detail, 'One.\nTwo.\\u{2066}');
    });

    test('how long, never "0 min"', () {
      expect(calendarLength(const Duration(seconds: 40)), 'under a minute');
      expect(calendarLength(const Duration(minutes: 70)), '1 h 10 min');
    });

    test("an open cell's four shapes, typed; anything else none", () {
      expect(parseOpenTarget('agent:pericolo'), isA<OpenAgent>());
      final item = parseOpenTarget('item:lucy:18')! as OpenItem;
      expect((item.agent, item.item), ('lucy', 18));
      final monitor = parseOpenTarget('monitor:pericolo:m3')! as OpenMonitor;
      expect((monitor.agent, monitor.monitor), ('pericolo', 3));
      expect(
        (parseOpenTarget('monitor:pericolo:3')! as OpenMonitor).monitor,
        3,
      );
      final task = parseOpenTarget('task:giorno:task-7')! as OpenTask;
      expect((task.agent, task.task), ('giorno', 'task-7'));
      for (final bad in [
        null,
        3,
        '',
        'agent:',
        'item:lucy:x',
        'monitor:p:mx',
        'task:a:b:c',
        'page:settings',
        'agent:a b',
      ]) {
        expect(parseOpenTarget(bad), isNull, reason: '$bad');
      }
    });
  });

  testWidgets("a week on a Mac: the range as heading, today's number in "
      'signal, all-day entries in the band across their days, entries at '
      'their times, overlapping ones side by side, a running one to the now '
      'line', (tester) async {
    await _show(tester, calendarView());
    expect(tester.takeException(), isNull);
    expect(find.text('30 Nov – 6 Dec 2026'), findsOneWidget);
    expect(find.text('Mon 30'), findsOneWidget);
    expect(find.text('Thu'), findsOneWidget);
    expect(find.text('3'), findsOneWidget);
    // The views, the week chosen.
    for (final view in ['Day', 'Week', 'Agenda']) {
      expect(find.bySemanticsLabel(view), findsOneWidget);
    }
    // The band: the 5-minute monitor across the week, the release on
    // Thursday alone.
    // A collapsed repeat reads its agent, name and cadence there.
    expect(
      find.text('Fugo · Logs · every 5 min', findRichText: true),
      findsOneWidget,
    );
    final logs = tester.getRect(_entry('Logs'));
    final release = tester.getRect(_entry('v0.1.26 release'));
    expect(logs.width, greaterThan(release.width * 6));
    expect(release.top, greaterThan(logs.top));
    // The build from 10:00 to 11:42 on Wednesday: an hour and 42 minutes
    // of the grid's 48 an hour.
    final build = tester.getRect(_entry('Release build'));
    expect(build.height, closeTo(48 * (1 + 42 / 60) - 2, 1));
    // Its top scrolled away (the grid opens around now), its title stays
    // in sight at the top of what shows of it.
    final grid = tester.getRect(
      find
          .descendant(
            of: find.byType(AppCalendar),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byType(AppCalendar),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position
        .jumpTo(10 * 48 + 30);
    await tester.pump();
    expect(tester.getRect(_entry('Release build')).top, lessThan(grid.top));
    expect(
      tester.getRect(find.text('Release build')).top,
      closeTo(grid.top + 4, 4),
    );
    // The QA pass and the item at 15:30 overlap: side by side, half each.
    final qa = tester.getRect(_entry('QA: focus mode on Mac'));
    final item = tester.getRect(_entry('#18 History video, check again'));
    expect(item.left, greaterThan(qa.right - 1));
    expect(qa.width, closeTo(item.width, 1));
    // Both within Thursday, as wide as its 18:00 monitor's pill.
    final day = tester.getRect(_entry('Production logs').last);
    expect(qa.left, closeTo(day.left, 1));
    expect(item.right, lessThanOrEqualTo(day.right + 1));
    expect(item.top, greaterThan(qa.top));
    // The running test grows to now: 13:10 to 14:20.
    final cargo = tester.getRect(_entry('cargo test, full suite'));
    expect(cargo.height, closeTo(48 * (70 / 60) - 2, 1));
    // The now line's time in the gutter, read as now.
    expect(find.bySemanticsLabel('Now, 14:20'), findsOneWidget);
    // Read whole, with whose it is and when.
    expect(
      find.bySemanticsLabel(
        "Production logs, Pericolo's monitor, today at 18:00, every 6 hours",
      ),
      findsOneWidget,
    );
  });

  testWidgets("VoiceOver's press works the bar and an entry", (tester) async {
    final semantics = tester.ensureSemantics();
    final transport = await _show(tester, calendarView());
    void press(Finder finder) => tester
        .binding
        .renderViews
        .first
        .owner!
        .semanticsOwner!
        .performAction(tester.getSemantics(finder).id, SemanticsAction.tap);
    press(find.bySemanticsLabel('Next week'));
    await tester.pump();
    expect(_moves(transport), [(DateTime(2026, 12, 10), 'week')]);
    press(_entry('Production logs').last);
    await tester.pumpAndSettle();
    expect(find.text('Open monitor'), findsOneWidget);
    semantics.dispose();
  });

  testWidgets("what it says: a running one since when; a band's details its "
      'cadence alone, never the days asked for', (tester) async {
    await _show(tester, calendarView());
    expect(
      tester
          .widget<Semantics>(_entry('cargo test, full suite'))
          .properties
          .label,
      "cargo test, full suite, Giorno's task, today since 13:10, running for "
      '1 h 10 min',
    );
    expect(
      tester.widget<Semantics>(_entry('Production logs').last).properties.label,
      "Production logs, Pericolo's monitor, today at 18:00, every 6 hours",
    );
    await tester.tap(
      find.textContaining('Fugo · Logs', findRichText: true).first,
    );
    await tester.pumpAndSettle();
    expect(find.text('every 5 min'), findsOneWidget);
    expect(find.textContaining('All-day'), findsNothing);
  });

  testWidgets("a phone's sheet: Open is a button of its own, apart from the "
      "details' words", (tester) async {
    final semantics = tester.ensureSemantics();
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(tester, calendarView(view: 'day'), size: const Size(390, 844));
    await tester.tap(find.textContaining('#18 History', findRichText: true));
    await tester.pumpAndSettle();
    final node = tester.getSemantics(find.text('Open item'));
    expect(node.label, 'Open item');
    expect(node.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
    semantics.dispose();
  });

  testWidgets('every move goes to the app with where and how, after its own '
      'context: ‹ ›, Today, the views, the keys and a swipe', (tester) async {
    final transport = await _show(tester, calendarView());
    await tester.tap(find.bySemanticsLabel('Next week'));
    await tester.tap(find.bySemanticsLabel('Previous week'));
    await tester.tap(find.text('Today'));
    await tester.tap(find.bySemanticsLabel('Day'));
    await tester.tap(find.bySemanticsLabel('Agenda'));
    await tester.pump();
    expect(_moves(transport), [
      (DateTime(2026, 12, 10), 'week'),
      (DateTime(2026, 11, 26), 'week'),
      (DateTime(2026, 12, 3), 'week'),
      (DateTime(2026, 12, 3), 'day'),
      (DateTime(2026, 12, 3), 'agenda'),
    ]);
    transport.actions.clear();
    // The keys, once the calendar has focus.
    await tester.tap(find.text('Thu'));
    await tester.pump();
    for (final key in [
      LogicalKeyboardKey.arrowRight,
      LogicalKeyboardKey.arrowLeft,
      LogicalKeyboardKey.keyT,
      LogicalKeyboardKey.keyD,
      LogicalKeyboardKey.keyA,
      LogicalKeyboardKey.keyW,
    ]) {
      await tester.sendKeyEvent(key);
    }
    expect(_moves(transport), [
      (DateTime(2026, 12, 10), 'week'),
      (DateTime(2026, 11, 26), 'week'),
      (DateTime(2026, 12, 3), 'week'),
      (DateTime(2026, 12, 3), 'day'),
      (DateTime(2026, 12, 3), 'agenda'),
      (DateTime(2026, 12, 3), 'week'),
    ]);
    transport.actions.clear();
    // A swipe to the left: the next week.
    await tester.fling(
      find.text('QA: focus mode on Mac'),
      const Offset(-300, 0),
      1000,
    );
    await tester.pumpAndSettle();
    expect(_moves(transport), [(DateTime(2026, 12, 10), 'week')]);
  });

  testWidgets("an entry's details beside it on a Mac: what, when, who, where "
      'from, and Open monitor, which goes there', (tester) async {
    await _show(tester, calendarView());
    Finder ring() => find.byWidgetPredicate(
      (w) =>
          w is DecoratedBox &&
          w.position == DecorationPosition.foreground &&
          (w.decoration as BoxDecoration).border is Border &&
          ((w.decoration as BoxDecoration).border! as Border).top.width == 2,
    );
    expect(ring(), findsNothing);
    await tester.tap(_entry('Production logs').last);
    await tester.pumpAndSettle();
    expect(find.text('Today, 18:00 · every 6 hours'), findsOneWidget);
    // The entry it's about ringed while it's open.
    expect(
      find.descendant(of: _entry('Production logs').last, matching: ring()),
      findsOneWidget,
    );
    expect(find.text('Pericolo · DevOps', findRichText: true), findsOneWidget);
    expect(
      find.text("From Pericolo's monitor. Last ran at 12:00."),
      findsOneWidget,
    );
    await tester.tap(find.text('Open monitor'));
    await tester.pumpAndSettle();
    expect(_opened, hasLength(1));
    final target = _opened.single as OpenMonitor;
    expect((target.agent, target.monitor), ('pericolo', 3));
    expect(find.text('Open monitor'), findsNothing, reason: 'closed first');
    expect(ring(), findsNothing);
  });

  testWidgets("one whose place has gone says why, with no button", (
    tester,
  ) async {
    await _show(
      tester,
      calendarView(),
      gone: (_) async => 'This monitor was cancelled.',
    );
    await tester.tap(_entry('Production logs').last);
    await tester.pumpAndSettle();
    expect(find.text('This monitor was cancelled.'), findsOneWidget);
    expect(find.text('Open monitor'), findsNothing);
  });

  testWidgets('while it asks whether the place has gone, the details keep '
      "the button's row; past a second the button shows and stays", (
    tester,
  ) async {
    final answer = Completer<String?>();
    await _show(tester, calendarView(), gone: (_) => answer.future);
    await tester.tap(_entry('Production logs').last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.bySemanticsLabel('Open monitor'), findsNothing);
    final card = find
        .ancestor(
          of: find.text("From Pericolo's monitor. Last ran at 12:00."),
          matching: find.byType(Column),
        )
        .first;
    final waiting = tester.getSize(card);
    await tester.pump(const Duration(milliseconds: 600));
    expect(find.bySemanticsLabel('Open monitor'), findsOneWidget);
    expect(tester.getSize(card), waiting, reason: 'nothing moves');
    answer.complete('This monitor was cancelled.');
    await tester.pumpAndSettle();
    expect(find.text('This monitor was cancelled.'), findsNothing);
    expect(find.bySemanticsLabel('Open monitor'), findsOneWidget);
  });

  testWidgets('on a phone: no week, the agenda instead; today first with '
      'all-day entries, what is over folded, the now line, then what is '
      "coming; a row's details in a sheet with its button", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final transport = await _show(
      tester,
      calendarView(),
      size: const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    expect(find.bySemanticsLabel('Week'), findsNothing);
    expect(find.bySemanticsLabel('Agenda'), findsOneWidget);
    expect(find.text('Today  ', findRichText: true), findsNothing);
    expect(
      find.textContaining('Thursday 3 December', findRichText: true),
      findsOneWidget,
    );
    // What's over today folds: the 12:00 fire.
    expect(find.text('Earlier today · 1'), findsOneWidget);
    expect(find.text('12:00'), findsNothing);
    await tester.tap(find.text('Earlier today · 1'));
    await tester.pump();
    expect(find.text('12:00'), findsOneWidget);
    // Now between what's going and what's coming.
    final now = tester.getRect(find.bySemanticsLabel('Now, 14:20'));
    expect(
      tester.getRect(_entry('cargo test, full suite')).bottom,
      lessThanOrEqualTo(now.top + 0.5),
    );
    expect(
      tester.getRect(_entry('QA: focus mode on Mac')).top,
      greaterThanOrEqualTo(now.bottom - 0.5),
    );
    expect(find.text('13:10 – now'), findsOneWidget);
    // A collapsed repeat: its name, then whose and its cadence, which says
    // whether it ends.
    expect(find.text('Fugo · monitor · every 5 min'), findsOneWidget);
    expect(find.text('all-day'), findsNWidgets(2));
    expect(find.text('Giorno · task · running for 1 h 10 min'), findsOneWidget);
    expect(find.text('Fugo · working for 40 min'), findsOneWidget);
    // A row's details in a sheet.
    await tester.dragUntilVisible(
      find.text('#18 History video, check again'),
      find.byType(ListView),
      const Offset(0, -100),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('#18 History video, check again'));
    await tester.pumpAndSettle();
    expect(find.text('Open item'), findsOneWidget);
    await tester.tap(find.text('Open item'));
    await tester.pumpAndSettle();
    expect((_opened.single as OpenItem).item, 18);
    // Day, from the switch.
    await tester.tap(find.bySemanticsLabel('Day'));
    expect(_moves(transport).last, (DateTime(2026, 12, 3), 'day'));
  });

  testWidgets("a phone's day: the week's days over it, today in signal; a "
      "tap on one moves to it", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final transport = await _show(
      tester,
      calendarView(view: 'day'),
      size: const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    expect(find.text('Thursday 3 December'), findsOneWidget);
    for (final day in ['30', '1', '2', '3', '4', '5', '6']) {
      expect(find.text(day), findsOneWidget, reason: day);
    }
    expect(_entry('QA: focus mode on Mac'), findsOneWidget);
    expect(_entry('Release build'), findsNothing, reason: 'Wednesday');
    await tester.tap(find.text('5'));
    expect(_moves(transport).last, (DateTime(2026, 12, 5), 'day'));
  });

  testWidgets("where the device's time isn't the Mac's, the bar says so", (
    tester,
  ) async {
    final mine = DateTime(2026, 12, 3).timeZoneOffset.inMinutes;
    await _show(
      tester,
      calendarView(
        more: {
          'macZone': {'start': mine - 360, 'end': mine - 360},
        },
      ),
    );
    expect(find.text('Your time · Claktory is 6 hours behind'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      calendarView(
        more: {
          'macZone': {'start': mine, 'end': mine},
        },
      ),
    );
    expect(find.textContaining('Your time'), findsNothing);
  });

  testWidgets("nothing in what's shown: the app's words over it", (
    tester,
  ) async {
    await _show(
      tester,
      calendarView(
        more: {
          'emptyTitle': 'Nothing on the calendar this week',
          'emptyBody': 'Ask an agent to add something.',
        },
      ),
      rows: const [],
    );
    expect(find.text('Nothing on the calendar this week'), findsOneWidget);
    expect(find.text('30 Nov – 6 Dec 2026'), findsOneWidget, reason: 'the bar');
  });

  testWidgets("a phone draws the agenda for any view: an empty one has the "
      "agenda's own words, or the app's one title", (tester) async {
    Map<String, Object?> words({bool agenda = true}) => calendarView(
      more: {
        'emptyTitle': 'Nothing on the calendar this week',
        if (agenda) 'agendaEmptyTitle': 'Nothing on the calendar coming up',
        'emptyBody': 'Ask an agent to add something.',
      },
    );
    await _show(tester, words(), rows: const [], size: const Size(390, 844));
    expect(find.text('Nothing on the calendar coming up'), findsOneWidget);
    expect(find.text('Nothing on the calendar this week'), findsNothing);
    expect(find.text('Ask an agent to add something.'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      words(agenda: false),
      rows: const [],
      size: const Size(390, 844),
    );
    expect(find.text('Nothing on the calendar this week'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await _show(tester, words(), rows: const []);
    expect(
      find.text('Nothing on the calendar this week'),
      findsOneWidget,
      reason: "the week's own words over the week",
    );
  });

  testWidgets("a short block shows its when only whole, never half a line", (
    tester,
  ) async {
    await _show(
      tester,
      calendarView(view: 'day'),
      rows: [
        calendarRow(
          id: 'sync',
          start: calendarAt(3, 14),
          end: calendarAt(3, 14, 30),
          title: 'Launch sync',
        ),
        calendarRow(
          id: 'review',
          start: calendarAt(3, 15),
          end: calendarAt(3, 15, 45),
          title: 'Review',
        ),
      ],
    );
    final sync = tester.getRect(_entry('Launch sync'));
    final syncWhen = find.textContaining('14:00–14:30');
    expect(
      tester.getRect(find.text('Launch sync')).bottom,
      lessThanOrEqualTo(sync.bottom),
    );
    expect(
      tester.getRect(syncWhen).top,
      greaterThanOrEqualTo(sync.bottom),
      reason: 'out of the block, which clips it',
    );
    final review = tester.getRect(_entry('Review'));
    final reviewWhen = tester.getRect(find.textContaining('15:00–15:45'));
    expect(reviewWhen.bottom, lessThanOrEqualTo(review.bottom));
    expect(
      reviewWhen.top,
      tester.getRect(find.text('Review')).bottom,
      reason: 'under the title',
    );
  });
}
