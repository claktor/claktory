import 'package:factory_app/map.dart';
import 'package:factory_app/map_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';

import 'package:factory_app/history/view.dart' show MapModeSwitch;
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'map_fixture.dart';
import 'phone_sizes_test.dart' show loadGeist;

/// The Map draws Workspace.map, which the backend computes; the app only
/// lays it out.
void main() {
  Future<Harness> showMap(
    WidgetTester tester, {
    Workspace? workspace,
    Size size = const Size(1440, 900),
  }) async {
    final harness = Harness(workspace ?? claktoryWorkspace());
    await harness.show(tester, size: size);
    harness.page.value = AppPage.map;
    // On a phone it's a page pushed over the conversation: let it slide in.
    await pumpFrames(tester);
    return harness;
  }

  Finder chip(String name) =>
      find.descendant(of: find.byType(MapView), matching: find.text(name));

  double opacityOf(WidgetTester tester, String name) => tester
      .widget<AnimatedOpacity>(
        find.ancestor(of: chip(name), matching: find.byType(AnimatedOpacity)),
      )
      .opacity;

  MapNode node(Workspace workspace, String id) =>
      workspace.map.nodes.firstWhere((node) => node.id == id);

  test("an agent's card lists every extension it has, the badges' first, "
      'where it works, whom it messages and its secrets', () {
    final workspace = claktoryWorkspace();
    final mike = mapAgentDetails(
      workspace,
      node(workspace, 'agent:coordinator'),
    );
    expect(
      [for (final e in mike.extensions) (e.label, e.important)],
      [
        ('Web', true),
        ('Claktory manual', true),
        ('Messaging', false),
        ('Checklist', false),
        ('Tasks', false),
      ],
    );
    expect(mike.folder?.label, 'App');
    expect(mike.reads.map((n) => n.label), ['Claktory', 'Research']);
    expect(mike.everyone, isFalse);
    // In the sidebar's order.
    final messages = mike.messages.map((n) => n.label).toList();
    expect(messages, [
      'Nora',
      'Iris',
      'Theo',
      'Hana',
      'Sam',
      'Lena',
      'Omar',
      'Priya',
      'Kenji',
      'Zoe',
      'Arjun',
      'Felix',
      'June',
    ]);
    expect(messages, isNot(contains('Disk Scout')));
    expect(messages, isNot(contains('Mike')));
    expect(mike.secrets.map((n) => n.label), ['App Store Connect']);

    // Rivet's badges stand for its extensions: Messaging is "Messages every
    // agent", Tasks "Stops any agent's tasks"; it messages every agent.
    final rivet = mapAgentDetails(workspace, node(workspace, 'agent:rivet'));
    expect(rivet.extensions.map((e) => e.label), [
      'Messages every agent',
      'Gives secrets to agents',
      "Stops any agent's tasks",
      'Web',
      'Own checkout',
      "Claktory's records",
    ]);
    expect(rivet.extensions.every((e) => e.important), isTrue);
    expect(rivet.everyone, isTrue);
    expect(rivet.folder?.label, 'Workspace');

    // Arjun messages Disk Scout, though it can't message him back.
    final arjun = mapAgentDetails(workspace, node(workspace, 'agent:devops'));
    expect(arjun.messages.map((n) => n.label), contains('Disk Scout'));
    final scout = mapAgentDetails(
      workspace,
      node(workspace, 'agent:disk-scout'),
    );
    expect(scout.messages, isEmpty);
    expect(scout.secrets, isEmpty);
  });

  testWidgets('resting the pointer on an agent shows its card beside it', (
    tester,
  ) async {
    await showMap(tester, workspace: claktoryWorkspace());
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(chip('Mike')));
    await tester.pump();
    expect(find.byType(MapAgentCard), findsNothing, reason: 'not at once');
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pump(const Duration(milliseconds: 200));
    final card = find.byType(MapAgentCard);
    expect(card, findsOneWidget);
    Finder inCard(String text) =>
        find.descendant(of: card, matching: find.text(text));
    for (final text in [
      'Project Manager',
      'Web',
      'Claktory manual',
      'Messaging',
      'Checklist',
      'Tasks',
      'App Store Connect',
    ]) {
      expect(inCard(text), findsOneWidget, reason: text);
    }
    expect(inCard('Memory'), findsNothing);
    expect(
      find.descendant(
        of: card,
        matching: find.textContaining('App  /Users/me/factory/app'),
      ),
      findsOneWidget,
    );
    // Beside Mike's chip, not over it.
    final chipRect = tester.getRect(
      find
          .ancestor(of: chip('Mike').first, matching: find.byType(MouseRegion))
          .first,
    );
    final cardRect = tester.getRect(card);
    expect(
      cardRect.left >= chipRect.right || cardRect.right <= chipRect.left,
      isTrue,
    );

    await mouse.moveTo(const Offset(1430, 890));
    await tester.pump();
    expect(find.byType(MapAgentCard), findsNothing);
  });

  testWidgets("an agent's card says what each of its secrets grants", (
    tester,
  ) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: MapAgentCard(
            details: MapAgentDetails(
              agent: MapNode(id: 'agent:devops', label: 'Arjun'),
              extensions: const [],
              folder: null,
              reads: const [],
              everyone: false,
              messages: const [],
              secrets: [
                MapNode(
                  id: 'secret:s1',
                  label: 'CF_CLAKTORY_DNS',
                  detail: 'Cloudflare: DNS Edit on claktory.com',
                ),
                MapNode(id: 'secret:s2', label: 'Hetzner'),
              ],
            ),
          ),
        ),
      ),
    );
    expect(
      find.text('CF_CLAKTORY_DNS  Cloudflare: DNS Edit on claktory.com'),
      findsOneWidget,
    );
    expect(find.text('Hetzner'), findsOneWidget);
  });

  testWidgets("apps are on it, and an agent's card says what it may do "
      'with each', (tester) async {
    final workspace = claktoryWorkspace(apps: true);
    await showMap(tester, workspace: workspace);
    expect(chip('Telegram'), findsOneWidget);
    final rivet = mapAgentDetails(workspace, node(workspace, 'agent:rivet'));
    expect(rivet.apps.single.app.label, 'Telegram');
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(child: MapAgentCard(details: rivet)),
      ),
    );
    expect(
      find.text(
        'Telegram  Send you Telegram messages · Read your chat with the bot'
        '  Hears: messages you send the bot',
      ),
      findsOneWidget,
    );
  });

  testWidgets("an agent's card says whom it reports to and whom it leads", (
    tester,
  ) async {
    final workspace = claktoryWorkspace(leads: true);
    final mike = mapAgentDetails(
      workspace,
      node(workspace, 'agent:coordinator'),
    );
    expect(mike.lead?.label, 'Rivet');
    // In the sidebar's order.
    expect(mike.reports.map((n) => n.label).take(3), ['Nora', 'Iris', 'Theo']);
    expect(mike.reports.map((n) => n.label), contains('Zoe'));
    final zoe = mapAgentDetails(workspace, node(workspace, 'agent:frontend-3'));
    expect(zoe.lead?.label, 'Mike');
    expect(zoe.reports, isEmpty);
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(child: MapAgentCard(details: mike)),
      ),
    );
    expect(find.text('Reports to'), findsOneWidget);
    expect(find.text('Rivet'), findsOneWidget);
    expect(find.text('Leads'), findsOneWidget);
    // Just under the heading, after Reports to and before Extensions.
    final under = tester.getRect(
      find.text(mike.reports.map((n) => n.label).join(', ')).first,
    );
    final heading = tester.getRect(find.text('Leads'));
    expect(under.top - heading.bottom, lessThan(4));
    expect(tester.getRect(find.text('Reports to')).top, lessThan(heading.top));
    expect(heading.top, lessThan(tester.getRect(find.text('Extensions')).top));
    // No lead and no one under it: neither section.
    final plain = mapAgentDetails(
      claktoryWorkspace(),
      node(claktoryWorkspace(), 'agent:coordinator'),
    );
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(child: MapAgentCard(details: plain)),
      ),
    );
    expect(find.text('Reports to'), findsNothing);
    expect(find.text('Leads'), findsNothing);
  });

  testWidgets('the key says what badges are', (tester) async {
    await showMap(tester);
    expect(find.text('Badges: extra reach or power'), findsOneWidget);
  });

  testWidgets('on a phone a tap shows the card, with Open conversation', (
    tester,
  ) async {
    final harness = await showMap(
      tester,
      workspace: claktoryWorkspace(),
      size: const Size(393, 852),
    );
    await tester.tap(chip('Mike'));
    await settle(tester);
    expect(harness.page.value, AppPage.map);
    expect(find.byType(MapAgentCard), findsOneWidget);
    expect(find.text('Claktory manual'), findsOneWidget);
    await tester.tap(find.text('Close'));
    await settle(tester);
    expect(find.byType(MapAgentCard), findsNothing);

    await tester.tap(chip('Mike'));
    await settle(tester);
    await tester.tap(find.text('Open conversation'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'coordinator');
  });

  testWidgets('every agent, project, folder and secret is on it', (
    tester,
  ) async {
    await showMap(tester);
    for (final name in [
      'Rivet',
      'Mike',
      'Sam',
      'Iris',
      'Disk Scout',
      'Claktory',
      'Disk',
      'App',
      'Research',
      'Secrets',
      'OpenAI',
      'Cloudflare',
    ]) {
      // Projects, folders and the vault are painted; agents and secrets
      // are chips.
      if (const {
        'Claktory',
        'Disk',
        'App',
        'Research',
        'Secrets',
      }.contains(name)) {
        continue;
      }
      expect(chip(name), findsOneWidget, reason: name);
    }
    expect(find.text('Done'), findsOneWidget);
  });

  testWidgets('clicking an agent opens its conversation', (tester) async {
    final harness = await showMap(tester);
    await tester.tap(chip('Sam'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'builder');
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('hovering an agent lights up those it reaches and dims the '
      'rest', (tester) async {
    await showMap(tester);
    expect(opacityOf(tester, 'Disk Scout'), 1);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(chip('Iris')));
    await tester.pump();
    // Iris messages her project, is nested under Mike and has OpenAI;
    // Disk Scout is out of her reach.
    expect(opacityOf(tester, 'Iris'), 1);
    expect(opacityOf(tester, 'Mike'), 1);
    expect(opacityOf(tester, 'OpenAI'), 1);
    expect(opacityOf(tester, 'Cloudflare'), lessThan(1));
    expect(opacityOf(tester, 'Disk Scout'), lessThan(1));

    await mouse.moveTo(const Offset(1430, 890));
    await tester.pump();
    expect(opacityOf(tester, 'Disk Scout'), 1);
  });

  testWidgets('each kind of line has a switch', (tester) async {
    await showMap(
      tester,
      workspace: claktoryWorkspace(apps: true, leads: true),
    );
    Finder toggle(String name) => find.byWidgetPredicate(
      (w) =>
          w is Semantics &&
          w.properties.label == name &&
          w.properties.toggled != null,
    );
    bool shown(String name) =>
        tester.widget<Semantics>(toggle(name)).properties.toggled!;
    for (final name in mapLayerNames.values) {
      expect(toggle(name), findsOneWidget, reason: name);
      expect(shown(name), isTrue);
    }
    await tester.tap(toggle('Secrets'));
    await tester.pump();
    expect(shown('Secrets'), isFalse);
    await tester.tap(toggle('Secrets'));
    await tester.pump();
    expect(shown('Secrets'), isTrue);
  });

  testWidgets('a backend too old to send the map says so', (tester) async {
    await showMap(tester, workspace: claktoryWorkspace()..clearMap());
    expect(find.text('Update Claktory to see its map.'), findsOneWidget);
  });

  testWidgets('on a phone it fills the screen, and Back goes back', (
    tester,
  ) async {
    final harness = await showMap(tester, size: const Size(393, 852));
    expect(find.byType(MapView), findsOneWidget);
    expect(chip('Mike'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(MapView), findsNothing);
  });

  group('at the largest text', () {
    // The real font's widths: the test font's squares wrap everything.
    setUpAll(loadGeist);

    testWidgets("on a wide window, the board keeps its own type size and "
        "the key's pills their words (Zucchero's row 517)", (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await showMap(tester, size: const Size(1366, 1024));
      expect(tester.takeException(), isNull);
      expect(find.byType(MapView), findsOneWidget);
      // A card on the board: drawn at the size it's laid out for.
      final kai = chip('Mike').first;
      expect(
        MediaQuery.textScalerOf(tester.element(kai)),
        TextScaler.noScaling,
      );
      // The key's pills hold their words whole, at most twice the size.
      for (final word in ['Now', 'History']) {
        final words = find.descendant(
          of: find.byType(MapModeSwitch),
          matching: find.text(word),
        );
        final paragraph = tester.renderObject<RenderParagraph>(words);
        expect(paragraph.didExceedMaxLines, isFalse, reason: word);
        final pill = tester.getRect(
          find.ancestor(of: words, matching: find.byType(AnimatedContainer)),
        );
        expect(
          pill.height,
          greaterThanOrEqualTo(tester.getSize(words).height),
          reason: word,
        );
      }
    });
  });
}
