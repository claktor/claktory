// Finding your way on an iPhone, end to end in the Simulator against the
// dev Claktory (#198, phase 3B; docs/mobile.md §7, items 1–3, 8 and 9 as
// far as the Simulator can show them): the drawer by its button, the
// screen's edge and the dimmed conversation; each page from the drawer and
// back again by Back or the edge; Settings' sections and a deep link; the
// Map's Lines, Search, card and Fit, and its drags staying on the Map; a
// project's Files; drafts kept with their agents across switches; the page
// showing kept over the background and a reconnect; and a Claktory that
// can't be reached never trapping the phone there.
//
// It never sends anything and leaves things as it found them: the agent
// shown, no drafts of its own, no extra Claktory.
//
//   scripts/ios.sh --dev   # once, so the Simulator's Claktory Dev is paired
//   cd app && flutter test integration_test/mobile_navigation_test.dart \
//     -d <your simulator> --dart-define=FACTORY_DEV=true
//
// Run it again with the largest text (`xcrun simctl ui <device>
// content_size accessibility-extra-extra-extra-large`): every step holds
// there too. With --dart-define=PAIRING_CODE='factory://…' it pairs first
// when the phone has nothing paired. Never against the person's Claktory.
import 'dart:io';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/claktory_identity.dart' show claktoryName;
import 'package:factory_app/connections.dart';
import 'package:factory_app/extensions.dart' show openPanelOf;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/map.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');

/// With REPORT set (a file on the Mac, as the Simulator shares its disk),
/// each step's outcome is written there as it ends, then DONE: a test run
/// as the app, rather than by `flutter test`, has no other way to say.
const _report = String.fromEnvironment('REPORT');

void report(String line) {
  if (_report.isEmpty) return;
  File(_report).writeAsStringSync('$line\n', mode: FileMode.append);
}

/// With SHOTS set, each step asks for a screenshot named after it: a file
/// /tmp/mei-shots/request that a loop on the Mac answers (`xcrun simctl io
/// DEVICE screenshot`), touching done-NAME when it has.
const _shots = String.fromEnvironment('SHOTS');

/// A Claktory that can't be reached: a code in the right form for a
/// machine that isn't there.
final _ghost = parsePairingCode(
  'factory://?t=${'e' * 64}&name=nowhere-nav-test&e=${'f' * 64}',
)!;

late WidgetTester _tester;

/// Lets the app run for [ms] of real time: animations, the tunnel, the
/// backend's answers.
Future<void> settle([int ms = 900]) async {
  final end = DateTime.now().add(Duration(milliseconds: ms));
  while (DateTime.now().isBefore(end)) {
    await _tester.pump(const Duration(milliseconds: 50));
    await Future<void>.delayed(const Duration(milliseconds: 30));
  }
}

/// Pumps until [finder] finds something (or nothing, when [gone]).
Future<void> waitFor(
  Finder finder, {
  int seconds = 20,
  bool gone = false,
}) async {
  final end = DateTime.now().add(Duration(seconds: seconds));
  while (DateTime.now().isBefore(end)) {
    await settle(200);
    if (finder.evaluate().isNotEmpty != gone) return;
  }
  await shot('timeout-${DateTime.now().millisecondsSinceEpoch % 100000}');
  fail('Timed out waiting for ${gone ? 'no ' : ''}$finder');
}

Future<void> shot(String name) async {
  if (_shots.isEmpty) return;
  await settle(600);
  final full = '$_shots-$name';
  File('/tmp/mei-shots/request').writeAsStringSync(full);
  final done = File('/tmp/mei-shots/done-$full');
  for (var i = 0; i < 50 && !done.existsSync(); i++) {
    await settle(100);
  }
}

/// Runs one part, saying which on failure.
Future<void> step(String name, Future<void> Function() run) async {
  // ignore: avoid_print
  print('STEP $name');
  try {
    await run();
    report('PASS $name');
  } on TestFailure catch (failure) {
    await shot('fail');
    report('FAIL $name: ${failure.message}');
    fail('$name: ${failure.message}');
  } catch (error, stack) {
    await shot('fail');
    final where = stack
        .toString()
        .split('\n')
        .where((line) => line.contains('mobile_navigation_test.dart'))
        .take(3)
        .join(' < ');
    report('FAIL $name: $error at $where');
    rethrow;
  }
}

Size get _screen => _tester.view.physicalSize / _tester.view.devicePixelRatio;

AppScope get _scope =>
    AppScope.scope(_tester.element(find.byType(PhoneDrawerHost).first));

Finder get _drawer => find.byType(Sidebar);

/// The drawer's list: of what scrolls up and down in it (not its search
/// field, nor its header at large text sizes), the tallest.
Finder get _drawerList {
  final vertical = find.descendant(
    of: _drawer,
    matching: find.byWidgetPredicate(
      (widget) =>
          widget is Scrollable &&
          axisDirectionToAxis(widget.axisDirection) == Axis.vertical,
    ),
  );
  final tallest = vertical.evaluate().reduce(
    (a, b) => a.size!.height >= b.size!.height ? a : b,
  );
  // The element, which outlives the list's rebuilds as it scrolls.
  return find.byElementPredicate((element) => element == tallest);
}

Finder _label(String label) => find.bySemanticsLabel(label);

/// What's labelled [label] for VoiceOver by a Semantics of its own (a
/// Claktory's tile or row), found by the widget, which merging can't hide.
Finder _named(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

/// The message box of the conversation in front.
Finder get _box => find
    .descendant(of: find.byType(AgentView), matching: find.byType(EditableText))
    .last;

String get _boxText => _tester.widget<EditableText>(_box).controller.text;

/// The header's leading button, or the screen's edge, opens the drawer.
Future<void> openDrawer({bool swipe = false}) async {
  if (swipe) {
    await _tester.dragFrom(
      Offset(6, _screen.height / 2),
      Offset(_screen.width * 0.7, 0),
    );
  } else {
    await _tester.tap(_label('Menu').last);
  }
  await waitFor(_drawer);
  await settle(400);
}

Future<void> closeDrawer() async {
  await _tester.tap(_label('Close').last);
  await waitFor(_drawer, gone: true);
}

/// [text] in the open drawer, scrolled into view. The list is moved by
/// its position, not dragged: at large text sizes a drag from its middle
/// can start on the search field.
Future<Finder> reveal(String text) =>
    scrollTo(find.text(text, skipOffstage: false), _drawerList);

/// Scrolls [list] from its top until [target] (a finder that doesn't skip
/// what's offstage) is built in it, then brings it into view; what's found.
Future<Finder> scrollTo(Finder target, Finder list) async {
  final position = _tester.state<ScrollableState>(list).position;
  position.jumpTo(0);
  await settle(200);
  final built = find.descendant(
    of: list,
    matching: target,
    skipOffstage: false,
  );
  while (built.evaluate().isEmpty &&
      position.pixels < position.maxScrollExtent) {
    position.jumpTo(
      (position.pixels + position.viewportDimension / 2).clamp(
        0,
        position.maxScrollExtent,
      ),
    );
    await settle(150);
  }
  expect(built, findsWidgets, reason: 'nothing like $target in the list');
  await Scrollable.ensureVisible(_tester.element(built.first));
  await settle(400);
  return find.descendant(of: list, matching: target).first;
}

/// Back on the page in front: the button, or a swipe from the edge.
Future<void> back({bool swipe = false}) async {
  if (swipe) {
    await _tester.dragFrom(
      Offset(4, _screen.height / 2),
      Offset(_screen.width * 0.8, 0),
    );
  } else {
    await _tester.tap(_label('Back').last);
  }
  await settle(700);
}

void expectFingerSized(Finder finder, String what) {
  final size = _tester.getSize(finder);
  expect(size.width, greaterThanOrEqualTo(44), reason: '$what is too narrow');
  expect(size.height, greaterThanOrEqualTo(44), reason: '$what is too short');
}

/// The Settings section named [name], from Settings' list.
Future<void> openSection(String name) async {
  final row = await scrollTo(
    find.text(name, skipOffstage: false),
    find
        .descendant(
          of: find.byType(SettingsView),
          matching: find.byType(Scrollable),
        )
        .first,
  );
  await _tester.tap(row);
  await waitFor(
    find.byWidgetPredicate(
      (widget) => widget is SettingsView && widget.section != null,
    ),
  );
  await settle(600);
}

/// Shows [agent]'s conversation from the drawer.
Future<void> choose(Agent agent) async {
  await openDrawer();
  await _tester.tap(await reveal(agent.name));
  await waitFor(_drawer, gone: true);
  await waitFor(
    find.descendant(
      of: find.byType(AgentView),
      matching: find.text(agent.name),
    ),
  );
  await settle(600);
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('finding your way on an iPhone', (tester) async {
    _tester = tester;
    addTearDown(() => report('DONE'));
    await app.main();
    await settle(1500);
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(
        _code,
        isNotEmpty,
        reason: 'nothing paired: pass --dart-define=PAIRING_CODE=…',
      );
      if (find.text('Paste code instead').evaluate().isNotEmpty) {
        await tester.tap(find.text('Paste code instead'));
        await settle();
      }
      await tester.enterText(find.byType(EditableText), _code);
      await tester.tap(find.text('Connect'));
      await tester.pump(const Duration(milliseconds: 500));
      // A code asks first: the confirmation's Connect.
      await tester.tap(find.text('Connect').last);
    }
    await waitFor(find.byType(PhoneDrawerHost), seconds: 90);
    await settle(3000);
    // A chat or a pipe left in front by something else: an agent instead.
    if (find.byType(AgentView).evaluate().isEmpty) {
      final backend = _scope.backend;
      final agents = backend.workspace.workspace.agents;
      if (agents.isNotEmpty) await backend.selectAgent(agents.first.id);
    }
    await waitFor(find.byType(AgentView), seconds: 60);
    await settle(1500);

    final scope = _scope;
    final backend = scope.backend;
    final workspace = backend.workspace.workspace;
    final shown = workspace.selectedAgentId;
    // Those in the drawer: none in an archived project.
    final archived = {
      for (final project in workspace.projects)
        if (project.archived) project.id,
    };
    final agents = [
      for (final agent in workspace.agents)
        if (!archived.contains(agent.projectId)) agent,
    ];
    expect(agents.length, greaterThanOrEqualTo(2), reason: 'two agents');
    final first = agents.firstWhere(
      (agent) => agent.id == shown,
      orElse: () => agents.first,
    );
    final second = agents.firstWhere((agent) => agent.id != first.id);
    final drafts = {
      first.id: scope.drafts.of(first.id),
      second.id: scope.drafts.of(second.id),
    };
    // A panel left open over a conversation (backend state) would cover
    // it: closed while this runs, and open again after.
    final panels = <String, String>{
      for (final agent in [first, second])
        if (openPanelOf(workspace, agent) case final panel?) agent.id: panel.id,
    };
    for (final agent in panels.keys) {
      await backend.openPanel(agent, '');
    }
    final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
    // Whatever happens, leave things as they were.
    addTearDown(() async {
      for (final MapEntry(:key, :value) in panels.entries) {
        await backend.openPanel(key, value);
      }
      for (final MapEntry(:key, :value) in drafts.entries) {
        scope.drafts.set(key, value);
      }
      if (scope.connections.saved.contains(_ghost)) {
        scope.connections.forget(_ghost);
      }
    });
    await shot('00-start');

    await step('the drawer opens by Menu or the edge, and closes by Close, '
        'a swipe back or the dimmed conversation', () async {
      expectFingerSized(_label('Menu').last, 'Menu');
      await openDrawer();
      await shot('01-drawer');
      expect(_drawer, findsOneWidget);
      expectFingerSized(_label('Close').last, "The drawer's Close");
      // Not as wide as the screen: the conversation shows beside it.
      expect(
        tester.getSize(_drawer).width,
        lessThanOrEqualTo(drawerWidth(_screen.width)),
      );
      await closeDrawer();

      await openDrawer(swipe: true);
      await tester.dragFrom(
        Offset(_screen.width * 0.6, _screen.height / 2),
        Offset(-_screen.width * 0.55, 0),
      );
      await waitFor(_drawer, gone: true);

      // A short swipe springs back.
      await tester.dragFrom(Offset(6, _screen.height / 2), const Offset(50, 0));
      await settle();
      expect(_drawer, findsNothing);

      await openDrawer(swipe: true);
      await tester.tapAt(Offset(_screen.width - 6, _screen.height / 2));
      await waitFor(_drawer, gone: true);
    });

    await step('choosing an agent in the drawer shows its conversation and '
        'closes the drawer', () async {
      await choose(second);
      expect(_drawer, findsNothing);
      await waitFor(
        find.byWidgetPredicate(
          (widget) =>
              widget is AgentView &&
              backend.workspace.workspace.selectedAgentId == second.id,
        ),
      );
      await choose(first);
    });

    await step('each agent keeps its own draft across switches', () async {
      final mine = 'nav-test $stamp, to ${first.name}';
      final theirs = 'nav-test $stamp, to ${second.name}';
      await tester.enterText(_box, mine);
      await settle(400);
      await choose(second);
      expect(_boxText, isNot(contains('nav-test $stamp')));
      await tester.enterText(_box, theirs);
      await settle(400);
      await shot('10-draft');
      await choose(first);
      expect(_boxText, mine);
      expect(scope.drafts.of(second.id), theirs);
      await choose(second);
      expect(_boxText, theirs);
      await tester.enterText(_box, drafts[second.id]!);
      await choose(first);
      expect(_boxText, mine, reason: 'kept while the other was cleared');
    });

    await step('every page in the drawer pushes over the conversation, and '
        'Back or a swipe from the edge takes it away', () async {
      final pages = {
        'Map': AppPage.map,
        'Usage': AppPage.usage,
        'Marketplace': AppPage.marketplace,
        'Settings': AppPage.settings,
        "What's new": AppPage.changelog,
      };
      var swipe = false;
      for (final MapEntry(key: row, value: page) in pages.entries) {
        await openDrawer();
        final tap = await reveal(row);
        // The whole row takes the tap, as tall as the drawer's rows of
        // one line.
        final whole = find
            .ancestor(
              of: tap,
              matching: find.byWidgetPredicate(
                (widget) => widget.runtimeType.toString() == '_Row',
              ),
            )
            .first;
        expect(
          tester.getSize(whole).height,
          greaterThanOrEqualTo(36),
          reason: row,
        );
        await tester.tap(tap);
        await settle(900);
        expect(scope.page.value, page, reason: row);
        expect(_drawer, findsNothing, reason: '$row closes the drawer');
        expect(find.byType(AgentView), findsNothing, reason: '$row covers it');
        expectFingerSized(_label('Back').last, "$row's Back");
        await shot('20-${page.name}');
        await back(swipe: swipe);
        expect(scope.page.value, AppPage.agent, reason: '$row goes back');
        expect(find.byType(AgentView), findsOneWidget, reason: row);
        // Going back never opens the drawer as well.
        expect(_drawer, findsNothing, reason: '$row: no drawer after Back');
        swipe = !swipe;
      }
      expect(_boxText, contains('nav-test $stamp'), reason: 'draft kept');
    });

    await step("Settings: a section is a page of its own, and a deep link "
        'opens it', () async {
      await openDrawer();
      await tester.tap(await reveal('Settings'));
      await waitFor(find.byType(SettingsView));
      await openSection('Devices');
      expect(
        find.byWidgetPredicate(
          (widget) => widget is SettingsView && widget.section != null,
        ),
        findsOneWidget,
      );
      await shot('30-settings-devices');
      await back(swipe: true);
      expect(scope.page.value, AppPage.settings, reason: 'back to the list');
      expect(
        find.byWidgetPredicate(
          (widget) => widget is SettingsView && widget.section != null,
        ),
        findsNothing,
      );
      await back();
      expect(scope.page.value, AppPage.agent);

      // What an agent's show_screen does: Settings, at a section.
      backend.settingsSection.value = 'devices';
      scope.page.value = AppPage.settings;
      await settle(1500);
      expect(
        find.byWidgetPredicate(
          (widget) => widget is SettingsView && widget.section == 'devices',
        ),
        findsOneWidget,
        reason: 'the deep link opens Devices',
      );
      await back();
      await back();
      expect(scope.page.value, AppPage.agent);
    });

    await step('the Map: dragging it pans and opens nothing; Lines, Search, '
        "an agent's card and Fit", () async {
      scope.page.value = AppPage.map;
      await waitFor(find.byType(MapView));
      await settle();
      final board = tester.getCenter(find.byType(MapView));
      await tester.dragFrom(board, const Offset(140, 40));
      await settle();
      expect(scope.page.value, AppPage.map, reason: 'a drag stays on the Map');
      expect(_drawer, findsNothing);

      expectFingerSized(_label('Lines').last, 'Lines');
      await tester.tap(_label('Lines').last);
      await settle();
      await shot('40-lines');
      // Its switches, each a whole row to tap.
      final toggle = find.text('Messages').last;
      await tester.tap(toggle);
      await settle(400);
      await tester.tap(toggle);
      await settle(400);
      await tester.tap(_label('Close').last);
      await settle();

      expectFingerSized(_label('Search the map').last, 'Search the map');
      await tester.tap(_label('Search the map').last);
      await waitFor(find.text('On the map'));
      await tester.enterText(find.byType(EditableText).last, second.name);
      await settle();
      await shot('41-list');
      await tester.tap(find.text(second.name).last);
      await waitFor(find.text('Open conversation'));
      await shot('42-card');
      // At the largest text the card's buttons scroll with its details.
      await tester.ensureVisible(find.text('Close').last);
      await settle(400);
      await tester.tap(find.text('Close').last);
      await settle();
      final fit = find
          .ancestor(
            of: find.textContaining(RegExp(r'^Fit')),
            matching: find.byType(Semantics),
          )
          .first;
      expect(fit, findsOneWidget, reason: 'Fit, once the card is closed');
      expectFingerSized(fit, 'Fit');
      await tester.tap(fit);
      await settle();
      await back();
      expect(scope.page.value, AppPage.agent);
    });

    final project = workspace.projects.where((p) => !p.archived).firstOrNull;
    if (project != null) {
      await step("a project's Files is a page with Back", () async {
        await openDrawer();
        final more = await scrollTo(
          find.byWidgetPredicate(
            (widget) =>
                widget is Semantics &&
                widget.properties.label == '${project.name} options',
            skipOffstage: false,
          ),
          _drawerList,
        );
        // A finger wide, as tall as the drawer's rows of one line.
        expect(tester.getSize(more).width, greaterThanOrEqualTo(44));
        expect(tester.getSize(more).height, greaterThanOrEqualTo(36));
        await tester.tap(more);
        await settle();
        await tester.tap(find.text('Files').last);
        await settle(1200);
        expect(find.byType(AgentView), findsNothing);
        expect(_drawer, findsNothing);
        await shot('50-files');
        await back();
        expect(find.byType(AgentView), findsOneWidget);
      });
    }

    await step(
      'the page showing stays over the background and a reconnect',
      () async {
        scope.page.value = AppPage.settings;
        await settle();
        await openSection('Devices');
        final binding = WidgetsBinding.instance;
        binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
        binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
        // In the background no frames come, so no pumping meanwhile.
        await Future<void>.delayed(const Duration(milliseconds: 1500));
        binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
        binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
        await settle(3000);
        backend.reconnectNow();
        await settle(5000);
        expect(scope.page.value, AppPage.settings);
        expect(
          find.byWidgetPredicate(
            (widget) => widget is SettingsView && widget.section != null,
          ),
          findsOneWidget,
          reason: 'still on Devices',
        );
        expect(find.textContaining("Can't connect"), findsNothing);
        await back();
        await back();
        expect(_boxText, contains('nav-test $stamp'), reason: 'draft kept');
      },
    );

    await step("a Claktory that can't be reached never traps the phone "
        'there, nor gets the draft', () async {
      final home = scope.connections.current;
      expect(home, isNotNull, reason: 'a phone shows another machine');
      final homeName = claktoryName(backend);
      scope.connections.use(_ghost);
      await settle(3000);
      await shot('60-ghost');
      expect(find.textContaining('nav-test $stamp'), findsNothing);
      // The way out: the drawer's rail, or "‹ Claktories" over a step.
      final claktories = _label('Back to Claktories');
      if (claktories.evaluate().isNotEmpty) {
        await tester.tap(claktories.last);
        await waitFor(find.byType(ClaktoryList));
        await tester.tap(_named(homeName).last);
      } else {
        await openDrawer();
        await shot('61-ghost-drawer');
        final rail = find.byType(PhoneClaktoryRail);
        if (rail.evaluate().isNotEmpty) {
          await tester.tap(
            find.descendant(of: rail, matching: _named(homeName)).first,
          );
        } else {
          // At large text: the Claktories row at the drawer's top, then
          // the list of them.
          await tester.tap(find.text('Claktories').last);
          await waitFor(find.byType(ClaktoryList));
          await tester.tap(_named(homeName).last);
        }
      }
      await settle(2500);
      expect(scope.connections.current, home);
      if (_drawer.evaluate().isNotEmpty) await closeDrawer();
      await waitFor(find.byType(AgentView));
      expect(_boxText, contains('nav-test $stamp'), reason: 'draft kept');
      scope.connections.forget(_ghost);
      await settle(1500);
    });

    await step('leaving things as they were', () async {
      final back = agents.firstWhere(
        (agent) => agent.id == shown,
        orElse: () => first,
      );
      if (backend.workspace.workspace.selectedAgentId != back.id) {
        await choose(back);
      }
      scope.drafts.set(first.id, drafts[first.id]!);
      scope.drafts.set(second.id, drafts[second.id]!);
      await settle(1500);
      await shot('99-end');
    });
  });
}
