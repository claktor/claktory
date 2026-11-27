// Moving agents in the sidebar: the drag says whom the agent will report to
// and who comes along; afterwards its row offers Undo for a while (a tap,
// ⌘Z, or VoiceOver's action); into another project it asks first, naming
// each effect.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

Agent _agent(
  String id,
  String name, {
  String lead = '',
  String project = 'p1',
  String folder = '',
  bool working = false,
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  folderId: folder,
  leadId: lead,
  parentId: project.isEmpty || lead == 'rivet' ? '' : lead,
  modelAvailable: true,
  status: working ? AgentStatus.AGENT_STATUS_WORKING : null,
);

Workspace _workspace({List<Agent> extra = const []}) => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Claktory',
      path: '/Users/me/.factory/workspace/claktory',
      folders: [
        ProjectFolder(id: 'research', name: 'Research', path: '/r'),
        ProjectFolder(id: 'ops', name: 'Ops', path: '/o'),
      ],
    ),
    Project(
      id: 'p2',
      name: 'Bakery',
      path: '/Users/me/.factory/workspace/bakery',
    ),
  ],
  agents: [
    _agent('rivet', 'Rivet', project: ''),
    _agent('mike', 'Mike', lead: 'rivet'),
    _agent('sam', 'Sam', lead: 'mike'),
    _agent('lena', 'Lena', lead: 'mike'),
    _agent('solo', 'Solo'),
    _agent('ana', 'Ana', project: 'p2'),
    ...extra,
  ],
);

Future<Harness> _show(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Rect _row(WidgetTester tester, String name) => tester.getRect(
  find.ancestor(of: find.text(name), matching: find.byType(Row)).first,
);

/// Drags [name] to [target], reading what its feedback says before letting
/// go.
Future<String?> _drag(WidgetTester tester, String name, Offset target) async {
  final from = tester.getCenter(find.text(name).first);
  final gesture = await tester.startGesture(
    from,
    kind: PointerDeviceKind.mouse,
  );
  await tester.pump(const Duration(milliseconds: 50));
  for (var i = 1; i <= 10; i++) {
    await gesture.moveTo(Offset.lerp(from, target, i / 10)!);
    await tester.pump(const Duration(milliseconds: 16));
  }
  String? says;
  for (final text in ['Reports to ', 'Moves to ']) {
    final found = find.textContaining(text);
    if (found.evaluate().isNotEmpty) {
      says = tester.widget<Text>(found.first).data;
    }
  }
  await gesture.up();
  await settle(tester);
  return says;
}

ArrangeAgent _arranged(Harness harness) =>
    harness.backend.sent.lastWhere((m) => m.hasArrangeAgent()).arrangeAgent;

void main() {
  testWidgets('the drag names who comes along', (tester) async {
    await _show(
      tester,
      _workspace(extra: [_agent('kid', 'Kid', lead: 'solo')]),
    );
    expect(
      await _drag(tester, 'Mike', _row(tester, 'Ana').center),
      'Reports to Ana · its 2 agents come too',
    );
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(
      await _drag(tester, 'Solo', _row(tester, 'Lena').center),
      'Reports to Lena · its 1 agent comes too',
    );
  });

  testWidgets("on a phone, the drag's words stay on the screen, however "
      'far right it was grabbed', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      _workspace(
        extra: [
          _agent('kid', 'Kid', lead: 'solo'),
          _agent('kid2', 'Kiddo', lead: 'solo'),
        ],
      )..selectedAgentId = 'solo',
    );
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
    await pumpFrames(tester);
    Rect inDrawer(String name) => tester.getRect(
      find.ancestor(of: find.text(name).last, matching: find.byType(Row)).first,
    );
    final from = Offset(320, inDrawer('Solo').center.dy);
    final gesture = await tester.startGesture(
      from,
      kind: PointerDeviceKind.touch,
    );
    await tester.pump(const Duration(milliseconds: 600));
    final to = Offset(320, inDrawer('Lena').center.dy);
    for (var i = 1; i <= 10; i++) {
      await gesture.moveTo(Offset.lerp(from, to, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    final says = find.text('Reports to Lena · its 2 agents come too');
    expect(says, findsOneWidget);
    expect(tester.getRect(says).right, lessThanOrEqualTo(390));
    expect(tester.getRect(says).left, greaterThanOrEqualTo(0));
    await gesture.up();
    await settle(tester);
  });

  for (final scale in [1.0, 3.0]) {
    testWidgets("by touch, Undo is a finger's target at the row's end, at "
        'text ×$scale', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(_workspace()..selectedAgentId = 'solo');
      await harness.show(tester, size: const Size(390, 844));
      await settle(tester);
      await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
      await pumpFrames(tester);
      Rect inDrawer(String name) => tester.getRect(
        find
            .ancestor(of: find.text(name).last, matching: find.byType(Row))
            .first,
      );
      final from = inDrawer('Solo').center;
      final gesture = await tester.startGesture(
        from,
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 600));
      final to = inDrawer('Lena').center;
      for (var i = 1; i <= 10; i++) {
        await gesture.moveTo(Offset.lerp(from, to, i / 10)!);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await pumpFrames(tester);
      expect(find.text('Now reports to Lena'), findsOneWidget);
      expect(
        tester
            .renderObject<RenderParagraph>(find.text('Now reports to Lena'))
            .didExceedMaxLines,
        isFalse,
      );
      final undo = find.bySemanticsLabel('Undo');
      expect(undo, findsOneWidget);
      await tester.ensureVisible(undo);
      await tester.pump();
      final target = tester.getRect(undo);
      expect(target.height, greaterThanOrEqualTo(44));
      expect(target.width, greaterThanOrEqualTo(44));
      // A tap near its edge, well off the word, still undoes it.
      await tester.tapAt(target.bottomLeft + const Offset(2, -2));
      await pumpFrames(tester);
      expect(harness.last.restoreArrangement.token, 'moved');
    });
  }

  for (final mouse in [false, true]) {
    testWidgets(
      'held still at the list\'s edge, it keeps scrolling, and a drop '
      'there lands on the row under the ${mouse ? 'pointer' : 'finger'} now',
      (tester) async {
        debugDefaultTargetPlatformOverride = mouse
            ? TargetPlatform.macOS
            : TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = Harness(
          _workspace(
            extra: [
              for (var i = 0; i < 40; i++)
                _agent('a$i', 'Agent $i', lead: 'mike'),
            ],
          )..selectedAgentId = 'solo',
        );
        await harness.show(
          tester,
          size: mouse ? const Size(1200, 700) : const Size(390, 844),
        );
        await settle(tester);
        if (!mouse) {
          await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
          await pumpFrames(tester);
        }
        final scrollable = find
            .ancestor(
              of: find.text('Mike').last,
              matching: find.byType(Scrollable),
            )
            .first;
        final position = tester.state<ScrollableState>(scrollable).position;
        final list = tester.getRect(scrollable);
        final from = tester.getCenter(find.text('Sam').last);
        final gesture = await tester.startGesture(
          from,
          kind: mouse ? PointerDeviceKind.mouse : PointerDeviceKind.touch,
        );
        await tester.pump(Duration(milliseconds: mouse ? 50 : 600));
        final to = Offset(from.dx, list.bottom - 8);
        for (var i = 1; i <= 10; i++) {
          await gesture.moveTo(Offset.lerp(from, to, i / 10)!);
          await tester.pump(const Duration(milliseconds: 16));
        }
        final moved = position.pixels;
        for (var i = 0; i < 30; i++) {
          await tester.pump(const Duration(milliseconds: 50));
        }
        expect(position.pixels, greaterThan(moved + 200));

        final under = [
          for (var i = 0; i < 40; i++)
            if (find.text('Agent $i').evaluate().isNotEmpty &&
                (tester.getCenter(find.text('Agent $i').last).dy - to.dy)
                        .abs() <
                    48)
              'a$i',
        ];
        await gesture.up();
        await pumpFrames(tester);
        final arranged = _arranged(harness);
        expect(
          under.contains(arranged.parentId) ||
              under.contains(arranged.beforeId),
          isTrue,
          reason: '$under, $arranged',
        );
      },
    );
  }

  for (final (device, text, lead) in [
    ('an iPad', 1.0, 'Bartholomew'),
    ('a phone at AX5', 3.1, 'Pericolo'),
  ]) {
    testWidgets('on $device, a moved row with a waiting badge says whom it '
        'reports to in whole words, its name in sight', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = text;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final pad = text == 1.0;
      if (pad) {
        asIPad();
      } else {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
      }
      final snapshot = _workspace();
      snapshot.agents.firstWhere((a) => a.id == 'solo')
        ..quietWaiting = pad ? 2 : 0
        ..waitingSeqs.addAll([if (pad) Int64(7)]);
      snapshot.agents.firstWhere((a) => a.id == 'lena').name = lead;
      snapshot.selectedAgentId = 'solo';
      final harness = Harness(snapshot);
      await harness.show(
        tester,
        size: pad ? const Size(1032, 1376) : const Size(390, 844),
      );
      await settle(tester);
      if (!pad) {
        await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
        await pumpFrames(tester);
      }
      Rect row(String name) => tester.getRect(
        find
            .ancestor(
              of: find.descendant(
                of: find.byType(Sidebar),
                matching: find.text(name),
              ),
              matching: find.byType(Row),
            )
            .first,
      );
      final from = row('Solo').center;
      final gesture = await tester.startGesture(
        from,
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 600));
      final to = row(lead).center;
      for (var i = 1; i <= 10; i++) {
        await gesture.moveTo(Offset.lerp(from, to, i / 10)!);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await pumpFrames(tester);
      final says = find.text('Now reports to $lead');
      expect(says, findsOneWidget);
      await tester.ensureVisible(says);
      await tester.pump();
      final line = tester.renderObject<RenderParagraph>(says);
      expect(line.didExceedMaxLines, isFalse);
      // Each line ends at a space: as wide as its longest word at least.
      final word = TextPainter(
        text: TextSpan(text: lead, style: line.text.style),
        textDirection: TextDirection.ltr,
        textScaler: line.textScaler,
      )..layout();
      expect(line.size.width, greaterThanOrEqualTo(word.width));
      word.dispose();
      final name = tester.renderObject<RenderParagraph>(find.text('Solo').last);
      expect(name.didExceedMaxLines, isFalse, reason: 'its name, whole');
      final undo = tester.getRect(find.bySemanticsLabel('Undo'));
      expect(undo.height, greaterThanOrEqualTo(44));
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('after a drop, its row offers Undo for 8 seconds', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-1');
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    expect(find.text('Now reports to Sam · '), findsOneWidget);
    expect(find.text('Undo'), findsOneWidget);

    await tester.tap(find.text('Undo'));
    await settle(tester);
    expect(harness.last.restoreArrangement.token, 'tok-1');
    expect(find.text('Now reports to Sam · '), findsNothing);

    // Again, left alone: gone after 8 seconds, the role back.
    await _drag(tester, 'Solo', _row(tester, 'Lena').center);
    expect(find.text('Now reports to Lena · '), findsOneWidget);
    await tester.pump(const Duration(seconds: 7));
    expect(find.text('Now reports to Lena · '), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
    await settle(tester);
    expect(find.text('Now reports to Lena · '), findsNothing);
  });

  testWidgets('out from under its lead: now reports to no one', (tester) async {
    await _show(tester, _workspace());
    await _drag(
      tester,
      'Sam',
      _row(tester, 'Solo').topCenter + const Offset(0, 3),
    );
    expect(find.text('Now reports to no one · '), findsOneWidget);
  });

  testWidgets('a plain reorder, or an older Claktory, offers no Undo', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    await _drag(
      tester,
      'Lena',
      _row(tester, 'Sam').topCenter + const Offset(0, 3),
    );
    expect(find.text('Undo'), findsNothing);

    harness.backend.arranged = null;
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    expect(harness.last.arrangeAgent.parentId, 'sam');
    expect(find.text('Undo'), findsNothing);
  });

  testWidgets('an Undo refused says why', (tester) async {
    final harness = await _show(tester, _workspace());
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    harness.backend.refuse = 'The list changed since. Move it again.';
    await tester.tap(find.text('Undo'));
    await settle(tester);
    expect(find.text("Couldn't move Solo back"), findsOneWidget);
    expect(find.text('The list changed since. Move it again.'), findsOneWidget);
  });

  testWidgets('⌘Z undoes it on a Mac, but not in a text field', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await _show(tester, _workspace()..selectedAgentId = 'mike');
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);

    // The message box has the keys: its own undo.
    await tester.tap(find.byType(EditableText).first);
    await tester.pump();
    final before = harness.backend.sent.length;
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyZ);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(
      harness.backend.sent.skip(before).where((m) => m.hasRestoreArrangement()),
      isEmpty,
    );

    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyZ);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(harness.last.restoreArrangement.token, 'moved');
    expect(find.text('Undo'), findsNothing);
  });

  testWidgets("VoiceOver: Undo is one of the moved row's actions", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = await _show(tester, _workspace());
    await _drag(tester, 'Solo', _row(tester, 'Sam').center);
    final undo = CustomSemanticsAction.getIdentifier(
      const CustomSemanticsAction(label: 'Undo'),
    );
    final row = find
        .ancestor(
          of: find.textContaining('Now reports to Sam'),
          matching: find.byWidgetPredicate(
            (widget) =>
                widget is Semantics &&
                (widget.properties.customSemanticsActions?.isNotEmpty ?? false),
          ),
        )
        .first;
    final data = tester.getSemantics(row).getSemanticsData();
    expect(data.customSemanticsActionIds, contains(undo));
    tester.binding.renderViews.first.owner!.semanticsOwner!.performAction(
      tester.getSemantics(row).id,
      SemanticsAction.customAction,
      undo,
    );
    await settle(tester);
    expect(harness.last.restoreArrangement.token, 'moved');
    semantics.dispose();
  });

  testWidgets('into another project: asked first; Cancel moves nothing', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    harness.backend.arrangePreview = ArrangePreview();
    await _drag(tester, 'Solo', _row(tester, 'Ana').center);
    expect(harness.last.previewArrange.parentId, 'ana');
    expect(find.text('Move Solo to Bakery?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasArrangeAgent()), isEmpty);

    await _drag(tester, 'Solo', _row(tester, 'Ana').center);
    await tester.tap(find.text('Move'));
    await settle(tester);
    expect(harness.last.arrangeAgent.projectId, 'p2');
    expect(find.text('Now in Bakery · '), findsOneWidget);
  });

  testWidgets('dropped into a collapsed project, it opens to show the row', (
    tester,
  ) async {
    final snapshot = _workspace()
      ..projects.firstWhere((p) => p.id == 'p2').collapsed = true;
    final harness = await _show(tester, snapshot);
    harness.backend.arrangePreview = ArrangePreview();
    await _drag(tester, 'Solo', _row(tester, 'Bakery').center);
    await tester.tap(find.text('Move'));
    await settle(tester);
    expect(_arranged(harness).projectId, 'p2');
    expect(
      harness.last.setProjectCollapsed,
      SetProjectCollapsed(projectId: 'p2', collapsed: false),
    );
  });

  testWidgets('dropped into a collapsed project at the foot of the list, the '
      'list scrolls to show the row and its Undo', (tester) async {
    Workspace snapshot({required bool moved}) {
      final workspace = _workspace(
        extra: [
          for (var i = 0; i < 30; i++) _agent('a$i', 'Agent $i', lead: 'mike'),
        ],
      );
      final bakery = workspace.projects.firstWhere((p) => p.id == 'p2');
      bakery.collapsed = !moved;
      if (moved) {
        workspace.agents.firstWhere((a) => a.id == 'solo').projectId = 'p2';
      }
      return workspace;
    }

    final harness = Harness(snapshot(moved: false));
    harness.backend.arrangePreview = ArrangePreview();
    await harness.show(tester, size: const Size(1200, 600));
    await settle(tester);
    final list = find
        .ancestor(of: find.text('Mike'), matching: find.byType(Scrollable))
        .first;
    final view = tester.getRect(list);
    await tester.scrollUntilVisible(find.text('Bakery'), 100, scrollable: list);
    await settle(tester);
    expect(
      tester.getRect(find.text('Bakery')).bottom,
      greaterThan(view.bottom - 60),
    );
    await _drag(tester, 'Solo', _row(tester, 'Bakery').center);
    await tester.tap(find.text('Move'));
    await settle(tester);
    await harness.apply(tester, snapshot(moved: true));
    // Once Bakery has opened, the list scrolls.
    for (var i = 0; i < 12; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    final undo = tester.getRect(find.bySemanticsLabel('Undo'));
    expect(undo.top, greaterThanOrEqualTo(view.top));
    expect(undo.bottom, lessThanOrEqualTo(view.bottom));
  });

  testWidgets('dropped onto a collapsed agent, it opens too', (tester) async {
    final snapshot = _workspace();
    snapshot.agents.firstWhere((a) => a.id == 'lena').collapsed = true;
    snapshot.agents.add(_agent('kid', 'Kid', lead: 'lena'));
    final harness = await _show(tester, snapshot);
    await _drag(tester, 'Solo', _row(tester, 'Lena').center);
    expect(_arranged(harness).parentId, 'lena');
    expect(
      harness.last.setAgentCollapsed,
      SetAgentCollapsed(agentId: 'lena', collapsed: false),
    );
  });

  test('the confirm names every effect that applies, and only those', () {
    final workspace =
        _workspace(
            extra: [
              _agent('speed', 'Speedwagon', folder: 'research', lead: 'mike'),
              _agent('oku', 'Okuyasu', lead: 'speed', folder: 'research'),
              _agent(
                'koi',
                'Koichi',
                lead: 'speed',
                folder: 'ops',
                working: true,
              ),
              _agent('ro', 'Rohan', lead: 'oku'),
              _agent('bo', 'Bo', project: 'p1'),
            ],
          )
          ..pipes.add(
            Pipe(
              id: 'pf',
              projectId: 'p1',
              peer: PipePeer(ownerName: 'Fred'),
            ),
          );
    final effects = moveEffects(
      workspace,
      workspace.agents.firstWhere((a) => a.id == 'speed'),
      ArrangeAgent(agentId: 'speed', parentId: 'ana', projectId: 'p2'),
      ArrangePreview(
        handovers: [
          PipeHandover(pipeId: 'pf', agentId: 'speed', nextLeadId: 'bo'),
        ],
      ),
    );
    expect(effects.title, 'Move Speedwagon to Bakery?');
    expect(effects.points, [
      'The 3 agents under it move too: Okuyasu, Koichi and Rohan.',
      "They'll work in Bakery's folder, ~/.factory/workspace/bakery, instead "
          "of Research, Ops and Claktory's.",
      'They can then message only agents in Bakery.',
      "Their files stay in Research's, Ops's and Claktory's folders.",
      'Speedwagon will report to Ana.',
      'Speedwagon stops answering the pipe with Fred, and Bo takes over.',
      'Koichi is working now and carries on in Bakery from its next step.',
    ]);
  });

  test('out of any project: reporting to Rivet first, in the workspace', () {
    final workspace = _workspace();
    final solo = workspace.agents.firstWhere((a) => a.id == 'solo');
    final effects = moveEffects(
      workspace,
      solo,
      ArrangeAgent(agentId: 'solo', parentId: 'rivet', projectId: ''),
      ArrangePreview(
        handovers: [PipeHandover(pipeId: 'gone', agentId: 'solo')],
      ),
    );
    expect(effects.title, 'Move Solo out of Claktory?');
    expect(effects.points, [
      'Solo will report to Rivet.',
      "It'll work in the workspace folder instead of Claktory's.",
      'It can then message only Rivet and other agents outside projects.',
      "Its files stay in Claktory's folder.",
    ]);
  });

  test('more than three along are counted; a pipe no one takes over waits', () {
    final workspace =
        _workspace(
            extra: [
              for (var i = 0; i < 5; i++) _agent('r$i', 'R$i', lead: 'solo'),
            ],
          )
          ..pipes.add(
            Pipe(
              id: 'pf',
              projectId: 'p1',
              peer: PipePeer(ownerName: 'Fred'),
            ),
          );
    final points = moveEffects(
      workspace,
      workspace.agents.firstWhere((a) => a.id == 'solo'),
      ArrangeAgent(agentId: 'solo', projectId: 'p2'),
      ArrangePreview(
        handovers: [PipeHandover(pipeId: 'pf', agentId: 'solo')],
      ),
    ).points;
    expect(
      points,
      containsAll([
        'The 5 agents under it move too: R0, R1, R2 and 2 more.',
        'Solo will report to no one.',
        "Solo stops answering the pipe with Fred. Fred's messages wait for "
            'you until you choose who answers.',
      ]),
    );
  });
}
