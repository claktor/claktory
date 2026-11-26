// Find on a phone (Matt's ask: find wherever you are): a page whose pane can
// be searched shows the phone's find bar at its foot while its find is
// open, above the keyboard; its arrows go up and down the screen whichever
// way the matches run; Done closes it and the pane stops marking.
import 'dart:async';

import 'package:factory_app/find.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/mobile/header.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'app_test.dart' show builder, workspace;
import 'chats_test.dart' show chat, chatting;
import 'panes_test.dart' show snapshot;

/// A pane's find that records what it was asked.
class _Target extends FindTarget {
  _Target({this.up = false});

  final bool up;
  final queries = <String>[];
  final steps = <int>[];
  var cleared = 0;
  var _count = 0;
  var _current = -1;
  var _done = false;

  @override
  String get placeholder => 'Search this page';

  @override
  bool get nextIsUp => up;

  @override
  bool get searched => _done;

  @override
  int get count => _count;

  @override
  int get current => _current;

  @override
  void search(String query) {
    queries.add(query);
    _done = query.isNotEmpty;
    _count = query.isEmpty ? 0 : 12;
    _current = query.isEmpty ? -1 : 0;
    notifyListeners();
  }

  @override
  void step(int by) {
    steps.add(by);
    _current = (_current + by) % _count;
    notifyListeners();
  }

  @override
  void clear() {
    cleared++;
    _done = false;
    _count = 0;
    _current = -1;
    notifyListeners();
  }
}

/// Scratch shots (a zz test sets it).
Future<void> Function(WidgetTester, String)? shots;

PhoneRoutes _routes(WidgetTester tester) =>
    tester.widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope)).routes;

void main() {
  Future<_Target> page(
    WidgetTester tester, {
    bool up = false,
    List<Widget>? header,
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot());
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    addTearDown(tester.view.resetPadding);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    final target = _Target(up: up);
    addTearDown(target.dispose);
    unawaited(
      _routes(tester).push<void>(
        PhoneRoute(
          id: 'find-test',
          builder: (context) => Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (header != null)
                PhoneHeader(title: const Text('A page'), trailing: header),
              Expanded(
                child: FindScope(
                  target: target,
                  child: const Center(child: Text('The page')),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    await pumpFrames(tester);
    return target;
  }

  FindSession session(WidgetTester tester) =>
      FindScope.of(tester.element(find.text('The page')))!;

  testWidgets('open, the bar sits at the foot above the keyboard; typing '
      'searches and counts; Done closes and clears', (tester) async {
    final target = await page(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsNothing);
    session(tester).open();
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
    expect(find.text('Search this page'), findsWidgets);
    // Nothing searched yet: the arrows are dimmed and step nothing.
    await tester.tap(
      find.bySemanticsLabel('Next match').last,
      warnIfMissed: false,
    );
    await pumpFrames(tester);
    expect(target.steps, isEmpty);
    await tester.enterText(find.byKey(const ValueKey('phone-find')), 'lapwing');
    await tester.pump(const Duration(milliseconds: 300));
    await pumpFrames(tester);
    expect(target.queries, ['lapwing']);
    expect(find.text('1 of 12'), findsOneWidget);
    // The keyboard rises: the bar with it.
    tester.view.viewInsets = FakeViewPadding(
      bottom: 336 * tester.view.devicePixelRatio,
    );
    tester.view.padding = const FakeViewPadding(top: 59);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    final bar = tester.getRect(find.byKey(const ValueKey('phone-find-bar')));
    expect(bar.bottom, closeTo(852 - 336, 1));
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsNothing);
    expect(target.cleared, 1);
    expect(find.text('The page'), findsOneWidget);
  });

  testWidgets("a page with no action of its own: Search in its header "
      'opens its find', (tester) async {
    await page(tester, header: const []);
    final search = find.byKey(const ValueKey('phone-search'));
    expect(search, findsOneWidget);
    expect(tester.getSize(search), const Size(44, 44));
    await tester.tap(search);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
    // Open already: no Search.
    expect(search, findsNothing);
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(search, findsOneWidget);
  });

  testWidgets('a page with a ⋯ of its own: no Search in its header', (
    tester,
  ) async {
    await page(tester, header: [HeaderButton.more(onTap: () {})]);
    expect(find.byKey(const ValueKey('phone-search')), findsNothing);
  });

  testWidgets("a chat's ⋯ offers Search first; the keyboard's Search key "
      'steps and keeps the field', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(chatting([chat()]));
    harness.backend.chatTranscript.load(
      Transcript(
        agentId: 'c1',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(text: 'A mango, please'),
          ),
          Entry(
            seq: Int64(2),
            assistant: AssistantText(text: 'One mango'),
          ),
        ],
      ),
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    addTearDown(tester.view.resetPadding);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('More').last);
    await pumpFrames(tester);
    final rows = [
      for (final label in ['Search', 'Rename…', 'Delete…'])
        tester.getTopLeft(find.text(label).last).dy,
    ];
    expect(rows, orderedEquals([...rows]..sort()));
    await tester.tap(find.text('Search').last);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
    // Search on the keyboard: the next match, and the field keeps focus.
    final field = find.byKey(const ValueKey('phone-find'));
    await tester.enterText(field, 'mango');
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await pumpFrames(tester);
    final focused = FocusManager.instance.primaryFocus?.context;
    expect(
      focused != null &&
          find
              .descendant(of: field, matching: find.byWidget(focused.widget))
              .evaluate()
              .isNotEmpty,
      isTrue,
    );
  });

  // Needs you is one card a screen on a phone (#514): no page to search,
  // its header's end is ˄ ˅.
  for (final (name, page) in [('Settings', AppPage.settings)]) {
    testWidgets('$name: Search in its header opens its find', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(snapshot());
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      addTearDown(tester.view.resetPadding);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      harness.page.value = page;
      await pumpFrames(tester);
      // Settings' own list has no find; a section's page does.
      if (page == AppPage.settings) {
        expect(find.byKey(const ValueKey('phone-search')), findsNothing);
        await tester.tap(find.text('General'));
        await pumpFrames(tester);
      }
      await shots?.call(tester, 'header-$name');
      final search = find.byKey(const ValueKey('phone-search'));
      expect(search, findsOneWidget);
      await tester.tap(search);
      await pumpFrames(tester);
      expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
      await shots?.call(tester, 'bar-$name');
      // Done puts the keyboard away: no field of the page takes the keys
      // (General's first is the Claktory's name).
      await tester.tap(find.text('Done'));
      await pumpFrames(tester);
      expect(find.byKey(const ValueKey('phone-find-bar')), findsNothing);
      final focused = FocusManager.instance.primaryFocus?.context;
      expect(
        focused == null ||
            focused.findAncestorWidgetOfExactType<EditableText>() == null &&
                focused.widget is! EditableText,
        isTrue,
      );
      expect(tester.testTextInput.isVisible, isFalse);
    });
  }

  testWidgets('a pipe with no liaison: Search in its header\'s …; its composer '
      'steps aside for the bar, keeping what was typed', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      Workspace(
        projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Bugs',
            projectId: 'p1',
            state: PipeState.PIPE_STATE_OPEN,
            online: true,
            peer: PipePeer(claktoryName: "Bea's Claktory", ownerName: 'Bea'),
          ),
        ],
        selectedPipeId: 'pipe1',
      ),
    );
    tester.view.padding = const FakeViewPadding(top: 20);
    addTearDown(tester.view.resetPadding);
    await harness.show(tester, size: const Size(375, 667));
    await pumpFrames(tester);
    await tester.enterText(
      find.descendant(
        of: find.bySemanticsLabel(RegExp('Write to')).last,
        matching: find.byType(EditableText),
      ),
      'Half written',
    );
    // Its "…" (Move to group…, #359) leads with Search, as an agent's.
    await tester.tap(find.bySemanticsLabel('Bugs: more'));
    await pumpFrames(tester);
    await tester.tap(find.text('Search'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
    expect(find.text('Half written'), findsNothing);
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.text('Half written'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('in a document the down arrow is Next; where the next match '
      'is up the screen, the up arrow is', (tester) async {
    for (final up in [false, true]) {
      final target = await page(tester, up: up);
      session(tester).open();
      await pumpFrames(tester);
      await tester.enterText(find.byKey(const ValueKey('phone-find')), 'x');
      await tester.pump(const Duration(milliseconds: 300));
      await pumpFrames(tester);
      final next = tester.getCenter(find.bySemanticsLabel('Next match').last);
      final previous = tester.getCenter(
        find.bySemanticsLabel('Previous match').last,
      );
      // The up arrow first, at the start of the pair.
      expect(next.dx < previous.dx, up);
      await tester.tap(
        find.bySemanticsLabel('Next match').last,
        warnIfMissed: false,
      );
      await pumpFrames(tester);
      expect(target.steps.last, 1);
      _routes(tester).pop();
      await pumpFrames(tester);
    }
  });

  testWidgets('in a conversation: ⋯ offers Search, the bar takes the message '
      "box's place above the keyboard, the up arrow goes to older matches, and "
      'Done brings the box back with its draft', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      workspace(agents: [builder(working: false)])..selectedAgentId = 'a1',
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          for (var seq = 1; seq <= 6; seq++)
            Entry(
              seq: Int64(seq),
              timestampMs: Int64(seq * 1000),
              assistant: AssistantText(text: 'Lapwing count $seq.'),
            ),
        ],
      ),
    );
    harness.backend.matches = (request) => ConversationMatches(
      query: request.query,
      agentId: request.agentId,
      matches: [
        for (final seq in [6, 4, 2])
          ConversationMatch(
            seq: Int64(seq),
            snippet: 'Lapwing count $seq.',
            starts: [0],
            lengths: [7],
            hits: 1,
            timestampMs: Int64(seq * 1000),
          ),
      ],
      total: 3,
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    addTearDown(tester.view.resetPadding);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    // A draft in the box.
    await tester.enterText(find.byType(EditableText).last, 'Half written');
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Extensions').last);
    await pumpFrames(tester);
    await tester.tap(find.text('Search'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsOneWidget);
    expect(find.text('Half written'), findsNothing, reason: 'the box aside');
    await tester.enterText(find.byKey(const ValueKey('phone-find')), 'lapwing');
    await tester.pump(const Duration(milliseconds: 200));
    await pumpFrames(tester);
    expect(find.text('1 of 3'), findsOneWidget);
    tester.view.viewInsets = FakeViewPadding(
      bottom: 336 * tester.view.devicePixelRatio,
    );
    tester.view.padding = const FakeViewPadding(top: 59);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(
      tester.getRect(find.byKey(const ValueKey('phone-find-bar'))).bottom,
      closeTo(852 - 336, 1),
    );
    await shots?.call(tester, 'conversation-find');
    // Up the screen is older, and Next.
    final next = find.bySemanticsLabel('Next match').last;
    final previous = find.bySemanticsLabel('Previous match').last;
    expect(tester.getCenter(next).dx, lessThan(tester.getCenter(previous).dx));
    await tester.tap(next);
    await pumpFrames(tester);
    expect(find.text('2 of 3'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('phone-find-bar')), findsNothing);
    expect(find.text('Half written'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
