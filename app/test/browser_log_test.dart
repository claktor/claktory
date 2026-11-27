// Settings → Browser (B3b's log): what agents did with browser control in
// the last 7 days, newest first, never what was typed or read; by agent,
// a page at a time, and Clear log… after asking. Only where the backend
// has browser control.

import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/browser/browser_log.dart';
import 'package:factory_app/controls.dart' show Select;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool browser = true}) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', modelAvailable: true),
    Agent(id: 'kai', name: 'Kai', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  version: '2026.11.22-0900',
  extensions: [
    Extension(id: 'memory', name: 'Memory'),
    if (browser) Extension(id: 'browser_control', name: 'Browser'),
  ],
  remote: RemoteAccess(
    devices: [PairedDevice(id: 'mac', name: 'Studio')],
  ),
);

final _now = DateTime.now().millisecondsSinceEpoch;
var _ids = 1000;

BrowserLogEntry _entry(
  String op,
  String outcome, {
  String agent = 'june',
  int minutesAgo = 1,
  int sequence = 0,
  String device = 'mac',
  int? id,
}) => BrowserLogEntry(
  id: Int64(id ?? _ids++),
  atMs: Int64(_now - minutesAgo * 60000),
  sessionId: 's1',
  agentId: agent,
  deviceId: op == 'request' ? '' : device,
  origin: 'https://example.com',
  op: op,
  sequence: Int64(sequence),
  outcome: outcome,
);

/// A session of June's, newest first: asked, allowed, read, a click the
/// person refused, a fill that may have happened, then the end.
List<BrowserLogEntry> _session() => [
  _entry('end', 'tab_closed', minutesAgo: 1),
  _entry('fill', 'outcome_unknown', minutesAgo: 2, sequence: 3),
  _entry('click', 'denied', minutesAgo: 3, sequence: 2),
  _entry('read', 'done', minutesAgo: 4, sequence: 1),
  _entry('grant', 'granted', minutesAgo: 5),
  _entry('request', 'assist', minutesAgo: 6),
];

Future<Harness> _show(
  WidgetTester tester, {
  bool browser = true,
  List<BrowserLogEntry>? log,
}) async {
  final harness = Harness(_workspace(browser: browser))
    ..backend.browserLog = log ?? _session();
  harness.page.value = AppPage.settings;
  await harness.show(tester, size: const Size(1200, 2400));
  await pumpFrames(tester);
  if (browser) {
    await revealInSettings(
      tester,
      find.byType(BrowserLogSettings),
      section: 'browser',
    );
  }
  await pumpFrames(tester);
  await pumpFrames(tester);
  return harness;
}

ListBrowserLog _lastAsk(Harness harness) => harness.backend.sent
    .lastWhere((message) => message.hasListBrowserLog())
    .listBrowserLog;

void main() {
  test('what each entry says', () {
    String what(String op, String outcome) =>
        browserLogWhat(_entry(op, outcome), agent: 'June', device: 'Studio');
    expect(what('request', 'read'), 'Asked to read example.com');
    expect(what('request', 'assist'), 'Asked to read and use example.com');
    expect(what('grant', 'granted'), 'You allowed it on Studio');
    expect(
      browserLogWhat(_entry('grant', 'granted')),
      'You allowed it',
      reason: 'a device that is gone',
    );
    expect(what('end', 'stopped'), 'Ended: you stopped it.');
    expect(what('end', 'agent_changed'), "Ended: June's settings changed.");
    expect(what('read', 'done'), 'Read the page');
    expect(what('navigate', 'done'), 'Went to another page');
    expect(what('hover', 'done'), 'Did something else');

    String outcome(String op, String word) =>
        browserLogOutcome(_entry(op, word));
    expect(outcome('read', 'done'), '');
    expect(outcome('click', 'denied'), 'You didn’t allow it');
    expect(outcome('fill', 'outcome_unknown'), 'Not sure it happened');
    expect(outcome('click', 'stale_target'), 'The page had changed');
    expect(outcome('click', 'something_new'), 'Didn’t work');
    expect(outcome('end', 'stopped'), '', reason: 'the end says it all');
  });

  testWidgets('only where the backend has browser control', (tester) async {
    final harness = await _show(tester, browser: false);
    expect(find.byType(BrowserLogSettings), findsNothing);
    expect(harness.backend.sent.where((m) => m.hasListBrowserLog()), isEmpty);
  });

  testWidgets('the log, newest first, with what never is in it', (
    tester,
  ) async {
    final harness = await _show(tester);
    expect(find.byType(BrowserLogSettings), findsOneWidget);
    expect(
      find.textContaining('What was typed and what pages said are never kept.'),
      findsOneWidget,
    );
    final ask = _lastAsk(harness);
    expect(
      (ask.agentId, ask.beforeMs, ask.beforeId, ask.limit),
      ('', Int64.ZERO, Int64.ZERO, 100),
    );
    final rows = tester
        .widgetList<BrowserLogRow>(find.byType(BrowserLogRow))
        .map((row) => row.entry.op)
        .toList();
    expect(rows, ['end', 'fill', 'click', 'read', 'grant', 'request']);
    for (final words in [
      'Ended: the tab closed.',
      'Filled in a field · Not sure it happened',
      'Clicked · You didn’t allow it',
      'Read the page',
      'You allowed it on Studio',
      'Asked to read and use example.com',
    ]) {
      expect(
        find.text(words, findRichText: true),
        findsOneWidget,
        reason: words,
      );
    }
    expect(find.text('example.com · Studio'), findsNWidgets(4));
    expect(find.textContaining('June · 1 min'), findsWidgets);
  });

  testWidgets('by agent, a page at a time', (tester) async {
    final many = [
      // Entries 98 to 101 share one millisecond, across the page's end.
      for (var i = 0; i < 130; i++)
        _entry(
          'read',
          'done',
          minutesAgo: i >= 98 && i <= 101 ? 99 : i + 1,
          sequence: 130 - i,
          id: 130 - i,
        ),
    ];
    final harness = await _show(
      tester,
      log: [
        ...many,
        _entry('read', 'done', agent: 'kai', minutesAgo: 200),
      ],
    );
    expect(find.byType(BrowserLogRow), findsNWidgets(100));
    await tester.ensureVisible(find.text('Show earlier'));
    await tester.pump();
    await tester.tap(find.text('Show earlier'));
    await pumpFrames(tester);
    expect(
      (_lastAsk(harness).beforeMs, _lastAsk(harness).beforeId),
      (many[99].atMs, many[99].id),
    );
    expect(find.byType(BrowserLogRow), findsNWidgets(131));
    expect(find.text('Show earlier'), findsNothing);

    // Only Kai's.
    final select = tester.widget<Select<String>>(
      find.descendant(
        of: find.byType(BrowserLogSettings),
        matching: find.byType(Select<String>),
      ),
    );
    expect(select.options.map((option) => option.label), [
      'All agents',
      'June',
      'Kai',
    ]);
    select.onChanged('kai');
    await pumpFrames(tester);
    expect(_lastAsk(harness).agentId, 'kai');
    expect(find.byType(BrowserLogRow), findsOneWidget);
  });

  testWidgets('Clear log… asks first', (tester) async {
    final harness = await _show(tester);
    await tester.ensureVisible(find.text('Clear log…'));
    await tester.tap(find.text('Clear log…'));
    await pumpFrames(tester);
    expect(find.text('Clear the browser log?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasClearBrowserLog()), isEmpty);

    await tester.tap(find.text('Clear log…'));
    await pumpFrames(tester);
    await tester.tap(find.text('Clear'));
    await pumpFrames(tester);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasClearBrowserLog()),
      hasLength(1),
    );
    expect(find.byType(BrowserLogRow), findsNothing);
    expect(find.text('Nothing in the last 7 days.'), findsOneWidget);
  });

  testWidgets('a new command shows without asking', (tester) async {
    final harness = await _show(tester, log: _session().sublist(3));
    expect(find.byType(BrowserLogRow), findsNWidgets(3));
    harness.backend.browserLog = _session();
    final next = _workspace()
      ..browserSessions.add(
        BrowserSession(id: 's1', state: 'active', commandsUsed: 3),
      );
    harness.backend.workspace.apply(next);
    await pumpFrames(tester);
    await pumpFrames(tester);
    expect(find.byType(BrowserLogRow), findsNWidgets(6));
  });

  testWidgets('a failure says why, with Try again', (tester) async {
    final harness = Harness(_workspace())
      ..backend.refuse = 'Browser control isn’t available yet.';
    harness.page.value = AppPage.settings;
    await harness.show(tester, size: const Size(1200, 2400));
    await pumpFrames(tester);
    await revealInSettings(
      tester,
      find.byType(BrowserLogSettings),
      section: 'browser',
    );
    await pumpFrames(tester);
    await pumpFrames(tester);
    expect(
      find.textContaining('Browser control isn’t available yet.'),
      findsOneWidget,
    );
    expect(find.text('Try again'), findsOneWidget);
  });
}
