// Alerts kept in Needs you (Bruno 44236, from Matt; Secco 44270; Lucy's
// needs-you-cards.md §3.9; Trish 44382): each waits until the person reads
// it, a light row of its own; urgent ones above the cards, the rest under
// them under Alerts, three or more from one agent folded; opening one, or
// Mark as read, reads it, and it stays marked Read until they leave. With
// SHOTS set, the page is drawn to PNGs there:
//   SHOTS=/tmp/alerts flutter test test/needs_you_alerts_test.dart
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/inbox.dart';
import 'package:factory_app/needs_you_alerts.dart';
import 'package:factory_app/needs_you_entries.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/window_fixtures.dart' show TextFactor;
import 'phone_sizes_test.dart' show loadGeist;

final _now = DateTime.now();

Alert _alert(
  String id,
  String agent, {
  String title = 'The deploy finished',
  String text = '',
  bool urgent = false,
  bool withheld = false,
  int minutesAgo = 10,
  int seq = 0,
}) => Alert(
  id: id,
  agentId: agent,
  fromName: switch (agent) {
    'june' => 'June',
    'omar' => 'Omar',
    '' => 'Studio',
    _ => agent,
  },
  title: withheld ? '' : title,
  text: withheld ? '' : text,
  urgent: urgent,
  withheld: withheld,
  sentMs: Int64(
    _now.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  seq: Int64(seq),
);

Workspace _workspace(List<Alert> alerts, {bool card = true}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(id: 'june', name: 'June', projectId: 'p1', modelAvailable: true),
    Agent(id: 'omar', name: 'Omar', projectId: 'p1', modelAvailable: true),
  ],
  pipes: [
    if (card)
      Pipe(
        id: 'pipe1',
        name: 'Acme launch',
        projectId: 'p1',
        state: PipeState.PIPE_STATE_APPROVAL,
      ),
  ],
  alerts: alerts,
  alertsUnread: alerts.length,
  selectedAgentId: 'june',
);

Future<Harness> _open(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(workspace);
  await harness.show(tester, size: size);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  return harness;
}

List<String> _read(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadAlerts()) ...message.readAlerts.ids,
];

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  test('urgent alerts alone first; the rest one each, or folded by who sent '
      'them from three on', () {
    final items = alertItems(
      _workspace([
        _alert('a1', 'june', minutesAgo: 50),
        _alert('a2', 'omar', minutesAgo: 40),
        _alert('a3', 'june', minutesAgo: 30),
        _alert('a4', 'june', urgent: true, minutesAgo: 25),
        _alert('a5', 'omar', minutesAgo: 20),
        _alert('a6', 'june', minutesAgo: 10),
      ]),
    );
    expect(
      [for (final item in items) item.key],
      [
        'alert:a4',
        // June's three non-urgent ones fold; Omar's two stay apart.
        'alerts:june',
        'alert:a5',
        'alert:a2',
      ],
    );
    expect([
      for (final alert in alertsOf(_workspace([]).alerts, items[1])) alert,
    ], isEmpty);
  });

  test('each alert counts once in Needs you, all of them', () {
    final workspace = _workspace([
      for (var i = 0; i < 4; i++) _alert('a$i', 'june', minutesAgo: i),
    ], card: true)..alertsUnread = 140;
    // The pipe's approval, and every alert, though only the newest 100
    // are listed (and these four fold into one row).
    expect(needsYouCount(workspace), 1 + 140);
  });

  testWidgets('an urgent alert goes above the cards; the rest under them, '
      'under Alerts', (tester) async {
    await _open(
      tester,
      _workspace([
        _alert('a1', 'omar', title: 'Tests are green', minutesAgo: 3),
        _alert(
          'a2',
          'june',
          title: "account.claktory.com isn't answering",
          text: 'Sign-in is down for everyone.',
          urgent: true,
        ),
      ]),
    );
    final urgent = tester.getTopLeft(
      find.text("account.claktory.com isn't answering"),
    );
    final card = tester.getTopLeft(find.textContaining('wants to connect'));
    final heading = tester.getTopLeft(find.text('Alerts'));
    final later = tester.getTopLeft(find.text('Tests are green'));
    expect(urgent.dy, lessThan(card.dy));
    expect(card.dy, lessThan(heading.dy));
    expect(heading.dy, lessThan(later.dy));
    expect(find.text('Sign-in is down for everyone.'), findsOneWidget);
    expect(find.textContaining('Urgent', findRichText: true), findsOneWidget);
    expect(find.text('Mark as read'), findsNWidgets(2));
  });

  testWidgets('Mark as read reads it, and it stays, marked Read, once it '
      'leaves the Workspace', (tester) async {
    final alerts = [_alert('a1', 'june', title: 'Backup done')];
    final harness = await _open(tester, _workspace(alerts));
    await tester.tap(find.text('Mark as read'));
    await pumpFrames(tester);
    expect(_read(harness), ['a1']);
    await harness.apply(tester, _workspace([]));
    await pumpFrames(tester);
    expect(find.text('Backup done'), findsOneWidget);
    expect(find.text('Read'), findsOneWidget);
    expect(find.text('Mark as read'), findsNothing);
  });

  testWidgets('clicking an alert opens its conversation at it, and reads it', (
    tester,
  ) async {
    final harness = await _open(
      tester,
      _workspace([_alert('a1', 'omar', title: 'Backup done', seq: 33)]),
    );
    await tester.tap(find.text('Backup done'));
    await pumpFrames(tester);
    expect(_read(harness), ['a1']);
    expect(
      harness.backend.sent
          .where((m) => m.hasSelectAgent())
          .last
          .selectAgent
          .agentId,
      'omar',
    );
  });

  testWidgets('three or more from one agent fold into one row, which opens '
      'in place, with Mark all as read', (tester) async {
    final harness = await _open(
      tester,
      _workspace([
        _alert('a1', 'june', title: 'First', minutesAgo: 30),
        _alert('a2', 'june', title: 'Second', minutesAgo: 20),
        _alert('a3', 'june', title: 'Third', minutesAgo: 10),
      ], card: false),
    );
    expect(find.text('June · 3 alerts'), findsOneWidget);
    // The latest's title under it; the others folded.
    expect(find.text('Third'), findsOneWidget);
    expect(find.text('First'), findsNothing);
    expect(_read(harness), isEmpty, reason: 'opening a fold reads nothing');
    await tester.tap(find.text('June · 3 alerts'));
    await pumpFrames(tester);
    expect(find.text('First'), findsOneWidget);
    expect(find.text('Mark as read'), findsNWidgets(3));
    await tester.tap(find.text('Mark all 3 as read'));
    await pumpFrames(tester);
    expect(_read(harness), unorderedEquals(['a1', 'a2', 'a3']));
    await harness.apply(tester, _workspace([], card: false));
    await pumpFrames(tester);
    // Still there, read, until the person leaves.
    expect(find.text('June · 3 alerts'), findsOneWidget);
    expect(find.text('Read'), findsWidgets);
  });

  testWidgets("a withheld alert says only who sent it, and where it is", (
    tester,
  ) async {
    await _open(
      tester,
      _workspace([_alert('a1', 'june', withheld: true)], card: false),
    );
    expect(find.text('June sent you an alert'), findsOneWidget);
    expect(find.text("Read it in June's conversation."), findsOneWidget);
    expect(find.text('Open conversation'), findsOneWidget);
  });

  testWidgets("an alert others' alerts folded into says who else sent one", (
    tester,
  ) async {
    await _open(
      tester,
      _workspace([
        _alert('a1', 'june', title: 'tprime2 stopped answering')
          ..alsoFrom.addAll(['Fugo', 'Pericolo']),
        _alert('a2', 'omar', title: 'Low disk'),
      ], card: false),
    );
    expect(find.text('tprime2 stopped answering'), findsOneWidget);
    expect(find.text('Also from Fugo and Pericolo'), findsOneWidget);
    // One that nothing folded into says nothing more.
    expect(find.textContaining('Also from'), findsOneWidget);
  });

  testWidgets("Claktory's own alert is from it, by name", (tester) async {
    await _open(
      tester,
      _workspace([
        _alert('a1', '', title: "The disk can't be written"),
      ], card: false),
    );
    expect(find.textContaining('Studio', findRichText: true), findsWidgets);
    expect(find.text("The disk can't be written"), findsOneWidget);
  });

  testWidgets("a notification's tap shows its alert and reads it; a fold's, "
      'all of it, opened', (tester) async {
    final harness = await _open(
      tester,
      _workspace([
        _alert('a1', 'june', title: 'First', minutesAgo: 30),
        _alert('a2', 'june', title: 'Second', minutesAgo: 20),
        _alert('a3', 'june', title: 'Third', minutesAgo: 10),
        _alert('a4', 'omar', title: 'Alone'),
      ]),
    );
    NeedsYouEntries.of(harness.backend).revealAlert.value = 'a4';
    await pumpFrames(tester);
    expect(_read(harness), ['a4']);
    NeedsYouEntries.of(harness.backend).revealAlert.value = 'a3';
    await pumpFrames(tester);
    expect(_read(harness), ['a4', 'a3', 'a2', 'a1']);
    expect(find.text('First'), findsOneWidget, reason: 'opened');
    await tester.pump(const Duration(seconds: 3));
  });

  for (final (phone, large) in [(false, false), (true, false), (true, true)]) {
    final where = '${phone ? 'iphone' : 'mac'}${large ? '-ax5' : ''}';
    testWidgets('drawn on the $where', (tester) async {
      if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      if (large) {
        tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      }
      await _open(
        tester,
        _workspace([
          _alert('a1', 'june', title: 'Migration 1 of 3 done', minutesAgo: 50),
          _alert('a2', 'june', title: 'Migration 2 of 3 done', minutesAgo: 40),
          _alert('a3', 'june', title: 'Migration 3 of 3 done', minutesAgo: 30),
          _alert(
            'a4',
            'omar',
            title: 'The nightly build is green again',
            text: 'The flaky test is fixed on main.',
            minutesAgo: 20,
          ),
          _alert('a5', 'omar', withheld: true, minutesAgo: 15),
          _alert(
            'a6',
            '',
            title: "Claktory's disk is almost full",
            text: '2 GB left on Studio.',
            urgent: true,
            minutesAgo: 5,
          ),
        ]),
        size: phone ? const Size(393, 852) : const Size(1100, 900),
      );
      expect(tester.takeException(), isNull);
      await _shot(tester, 'alerts-$where');
      // An iPhone's Needs you is the list too (#599): alerts stay rows,
      // their folds opening in place.
      await tester.ensureVisible(find.textContaining('· 3 alerts'));
      await pumpFrames(tester);
      await tester.tap(find.textContaining('· 3 alerts'));
      await pumpFrames(tester);
      await tester.ensureVisible(find.text('Mark all 3 as read'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      await _shot(tester, 'alerts-$where-open');
    });
  }
}
