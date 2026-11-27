// The role beside the agent's name in Needs you (Matt via Bruno 46581;
// Lucy's needs-you-cards.md §3.10): "June · Project Manager", quietly, on a
// request's row, an alert's ("Urgent · Omar · Backend · 14:32"), a fold's
// ("June · Project Manager · 3 alerts") and focus mode's header; cut first,
// gone before the name is cut; none for a liaison, Claktory's own or an
// agent without one. With SHOTS set, drawn to PNGs there:
//   SHOTS=/tmp/roles flutter test test/needs_you_roles_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/inbox.dart';
import 'package:factory_app/name_role_line.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
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
  bool urgent = false,
  int minutesAgo = 10,
}) => Alert(
  id: id,
  agentId: agent,
  fromName: switch (agent) {
    'june' => 'June',
    'omar' => 'Omar',
    _ => 'Studio',
  },
  title: title,
  urgent: urgent,
  sentMs: Int64(
    _now.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
);

Entry _ask(int seq) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _now.subtract(const Duration(minutes: 30)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(
    id: 'ask$seq',
    title: 'Ship the release tonight?',
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    revision: 1,
  ),
);

Workspace _workspace({List<Alert>? alerts, bool kai = false}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      role: 'Project Manager',
      projectId: 'p1',
      modelAvailable: true,
      waitingSeqs: [Int64(4)],
    ),
    Agent(
      id: 'omar',
      name: 'Omar',
      role: 'Backend',
      projectId: 'p1',
      modelAvailable: true,
    ),
    Agent(id: 'ana', name: 'Ana', projectId: 'p1', modelAvailable: true),
    if (kai)
      Agent(
        id: 'kai',
        name: 'Kai',
        role: 'Liaison',
        projectId: 'p1',
        pipeId: 'pipe1',
        modelAvailable: true,
        waitingSeqs: [Int64(6)],
      ),
  ],
  pipes: [if (kai) Pipe(id: 'pipe1', name: 'Acme launch', projectId: 'p1')],
  alerts:
      alerts ??
      [
        _alert('a1', 'june', title: 'Migration 1 of 3 done', minutesAgo: 50),
        _alert('a2', 'june', title: 'Migration 2 of 3 done', minutesAgo: 40),
        _alert('a3', 'june', title: 'Migration 3 of 3 done', minutesAgo: 30),
        _alert(
          'a4',
          'omar',
          title: 'The account service is down',
          urgent: true,
          minutesAgo: 5,
        ),
        _alert('a5', 'ana', title: 'Tests are green', minutesAgo: 20),
        _alert('a6', '', title: "Claktory's disk is almost full"),
      ],
  alertsUnread: alerts?.length ?? 6,
  selectedAgentId: 'june',
);

Future<Harness> _open(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1100, 900),
}) async {
  final harness = Harness(workspace);
  harness.backend.entriesById['june/4'] = _ask(4);
  harness.backend.entriesById['kai/6'] = _ask(6);
  await harness.show(tester, size: size);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  return harness;
}

/// A role's words beside a name, as NameRoleLine shows them (" · role",
/// read as ", role").
Finder _role(String role) => find.text(', $role', findRichText: true);

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

  test('the role of whoever asks: none for a liaison, Claktory\'s own, '
      'a pipe, or an agent without one', () {
    final workspace = _workspace(kai: true);
    expect(
      needsYouRole(workspace, NeedsYouItem.entry('june', 4)),
      'Project Manager',
    );
    expect(needsYouRole(workspace, NeedsYouItem.entry('kai', 6)), '');
    expect(needsYouRole(workspace, NeedsYouItem.alert('ana', 'a5')), '');
    expect(needsYouRole(workspace, NeedsYouItem.alert('', 'a6')), '');
    expect(
      needsYouWho(
        workspace,
        NeedsYouItem.alertGroup('june'),
        'June · 3 alerts',
      ),
      (name: 'June', role: 'Project Manager', after: ' · 3 alerts'),
    );
    expect(
      needsYouWho(workspace, NeedsYouItem.entry('kai', 6), 'Kai · Acme launch'),
      (name: 'Kai · Acme launch', role: '', after: ''),
    );
  });

  testWidgets('narrow, the role is cut first and goes before the name or '
      'the time is cut', (tester) async {
    Future<void> line(double width) => tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: SizedBox(
            width: width,
            child: const NameRoleLine(
              name: 'June',
              role: 'Project Manager',
              after: [TextSpan(text: ' · 14:32')],
              style: TextStyle(fontSize: 13),
              roleStyle: TextStyle(fontSize: 13),
            ),
          ),
        ),
      ),
    );
    await line(600);
    expect(find.text('June', findRichText: true), findsOneWidget);
    expect(_role('Project Manager'), findsOneWidget);
    await line(300);
    expect(tester.takeException(), isNull);
    expect(find.text('June', findRichText: true), findsOneWidget);
    expect(find.text(' · 14:32', findRichText: true), findsOneWidget);
    // Some of it shows, cut with an ellipsis.
    final role = tester.widget<RichText>(_role('Project Manager'));
    expect(role.overflow, TextOverflow.ellipsis);
    await line(200);
    expect(tester.takeException(), isNull);
    expect(_role('Project Manager'), findsNothing, reason: 'no room: gone');
    expect(find.text('June · 14:32', findRichText: true), findsOneWidget);
  });

  testWidgets('what follows the name wider than the row: one line, cut at '
      'its end, never overflowing', (tester) async {
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: SizedBox(
            width: 120,
            child: NameRoleLine(
              name: 'Rivet',
              role: 'Admin',
              after: [TextSpan(text: ' · 20784 days ago · Updated')],
              style: TextStyle(fontSize: 13),
              roleStyle: TextStyle(fontSize: 13),
            ),
          ),
        ),
      ),
    );
    expect(tester.takeException(), isNull);
    final line = tester.widget<RichText>(
      find.text('Rivet · 20784 days ago · Updated', findRichText: true),
    );
    expect(line.overflow, TextOverflow.ellipsis);
    expect(line.maxLines, 1);
  });

  testWidgets("on a Mac's Needs you: a request, an urgent alert and a fold "
      'each say the role; Claktory\'s own and an agent without one, the '
      'name alone', (tester) async {
    await _open(tester, _workspace());
    expect(tester.takeException(), isNull);
    // June's request, and her fold.
    expect(_role('Project Manager'), findsNWidgets(2));
    expect(find.text(' · 3 alerts', findRichText: true), findsOneWidget);
    // Omar's urgent alert.
    expect(_role('Backend'), findsOneWidget);
    expect(find.text('Urgent · Omar', findRichText: true), findsOneWidget);
    await _shot(tester, 'roles-mac');
  });

  testWidgets("VoiceOver says who asks once: the character beside the name "
      'reads nothing (Zucchero 48776)', (tester) async {
    final semantics = tester.ensureSemantics();
    await _open(tester, _workspace(alerts: const []));
    final labels = <String>[];
    void walk(SemanticsNode node) {
      labels.add(node.label);
      node.visitChildren((child) {
        walk(child);
        return true;
      });
    }

    walk(
      tester
          .binding
          .renderViews
          .first
          .owner!
          .semanticsOwner!
          .rootSemanticsNode!,
    );
    final label = labels.firstWhere(
      (label) => label.contains('Ship the release tonight'),
    );
    expect(RegExp('June').allMatches(label), hasLength(1), reason: label);
    expect(label, contains('June, Project Manager · 30 min ago'));
    semantics.dispose();
  });

  testWidgets("a liaison's request keeps its pipe's name, with no role", (
    tester,
  ) async {
    await _open(tester, _workspace(alerts: const [], kai: true));
    expect(
      find.textContaining('Kai · Acme launch', findRichText: true),
      findsWidgets,
    );
    expect(_role('Liaison'), findsNothing);
  });

  testWidgets("focus mode's header says it too (Mac)", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _open(tester, _workspace(alerts: const []));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyN);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(tester.takeException(), isNull);
    // The header, and the queue's row beside the stage (Lucy 46817).
    expect(_role('Project Manager'), findsNWidgets(2));
    await _shot(tester, 'roles-mac-focus');
  });

  for (final large in [false, true]) {
    testWidgets('on an iPhone at 390pt${large ? ', the largest text' : ''}: '
        "the list says it, and focus mode's stage", (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      if (large) {
        tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      }
      await _open(tester, _workspace(), size: const Size(390, 844));
      expect(tester.takeException(), isNull);
      expect(
        find.textContaining('Project Manager', findRichText: true),
        findsWidgets,
      );
      await _shot(tester, 'roles-iphone${large ? '-ax5' : ''}');
      if (large) return;
      // Focus mode's stage says it, and its "1 of 5" list (Lucy 46817).
      await tester.tap(find.bySemanticsLabel('Focus on Needs you'));
      await pumpFrames(tester);
      expect(
        find.textContaining('Project Manager', findRichText: true),
        findsWidgets,
      );
      await tester.tap(find.text('1 of 5'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(
        find.descendant(
          of: find.byType(Scrollable).last,
          matching: _role('Project Manager'),
        ),
        findsWidgets,
      );
      await _shot(tester, 'roles-iphone-queue');
    });
  }
}
