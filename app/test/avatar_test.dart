import 'package:factory_app/avatar.dart';
import 'package:factory_app/map.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'map_fixture.dart';
import 'ipad/as_ipad.dart';
import 'mobile_settings_test.dart' show avatarChoices;
import 'panes_test.dart' show beside;

Person _matt({String initials = 'MS'}) => Person(
  name: initials.isEmpty ? '' : 'Matt Smith',
  initials: initials,
  avatarFrom: '#4F46E5',
  avatarTo: '#7C3AED',
  avatarChoices: avatarChoices,
);

Workspace _workspace({Person? person}) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt Smith'),
  person: person ?? _matt(),
);

BoxDecoration _circle(WidgetTester tester, Finder avatar) =>
    tester
            .widget<Container>(
              find.descendant(of: avatar, matching: find.byType(Container)),
            )
            .decoration!
        as BoxDecoration;

void main() {
  test('colors come as #RRGGBB; anything else is no color', () {
    expect(hexColor('#4F46E5'), const Color(0xFF4F46E5));
    expect(hexColor('7C3AED'), const Color(0xFF7C3AED));
    expect(hexColor(''), isNull);
    expect(hexColor('#12345'), isNull);
    expect(hexColor('#GGGGGG'), isNull);
  });

  testWidgets("the person's initials on their gradient, in a circle; with "
      'no name, a person glyph', (tester) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Row(
          children: [
            PersonAvatar(person: _matt(), size: 40),
            PersonAvatar(person: _matt(initials: ''), size: 40),
            PersonAvatar(person: Person(initials: 'M'), size: 40),
          ],
        ),
      ),
    );
    expect(find.text('MS'), findsOneWidget);
    expect(find.text('M'), findsOneWidget);
    final first = _circle(tester, find.byType(PersonAvatar).first);
    expect(first.shape, BoxShape.circle);
    expect((first.gradient! as LinearGradient).colors, const [
      Color(0xFF4F46E5),
      Color(0xFF7C3AED),
    ]);
    // Colors the backend didn't give: slate.
    final last = _circle(tester, find.byType(PersonAvatar).last);
    expect(
      (last.gradient! as LinearGradient).colors.first,
      const Color(0xFF475569),
    );
    expect(tester.getSize(find.byType(PersonAvatar).first), const Size(40, 40));
  });

  testWidgets('beside the person\'s messages, and on their own comments '
      'and decisions', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(text: 'Hello June'),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    final bubble = find.text('Hello June');
    expect(bubble, findsOneWidget);
    final avatar = find.ancestor(
      of: find.text('MS'),
      matching: find.byType(PersonAvatar),
    );
    // One beside the message (the sidebar's footer has the other).
    expect(avatar, findsWidgets);
    final beside = avatar.evaluate().where((element) {
      final box = element.renderObject! as RenderBox;
      final at = box.localToGlobal(Offset.zero);
      return (at.dy - tester.getBottomLeft(bubble).dy).abs() < 40 &&
          at.dx > tester.getTopRight(bubble).dx;
    });
    expect(beside, hasLength(1), reason: 'at the bubble\'s right');
  });

  testWidgets('in a narrow Settings pane on an iPad, the words go under '
      "the avatar and the name rather than squeezed beside them", (
    tester,
  ) async {
    asIPad();
    final workspace = _workspace();
    workspace.agents.first.layout = beside('settings', '');
    final harness = Harness(workspace)..backend.appliesLayouts = true;
    // Settings squeezed to 320 (Sorbet's iPad pass: it overflowed by 64).
    await harness.show(tester, size: const Size(848, 900));
    await settle(tester);
    final words = find.text(
      'Shows beside your messages and comments, on every device.',
    );
    await tester.ensureVisible(words);
    await settle(tester);
    expect(
      tester.getTopLeft(words).dy,
      greaterThan(tester.getBottomLeft(find.text('Shuffle colors')).dy),
    );
    // And the name's note under its field, not a word to a line beside it.
    final note = find.text('Your agents call you by it.');
    expect(
      tester.getTopLeft(note).dy,
      greaterThanOrEqualTo(tester.getBottomLeft(find.text('Matt Smith')).dy),
    );
    expect(tester.getSize(note).height, lessThan(40));
  });

  testWidgets("the footer's avatar opens Settings, whose Profile shuffles "
      'it', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await tester.tap(find.bySemanticsLabel('Profile'));
    await settle(tester);
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(layout.setLayout.toString(), contains('settings'));
    harness.page.value = AppPage.settings;
    await settle(tester);
    expect(find.text('Profile'), findsWidgets);
    expect(find.text('Avatar'), findsOneWidget);
    await tester.tap(find.text('Shuffle colors'));
    await settle(tester);
    expect(harness.backend.sent.last.hasShuffleAvatar(), isTrue);
    // A plain shuffle names no color.
    expect(harness.backend.sent.last.shuffleAvatar.hasChoice(), isFalse);

    // Or a color picked from the ones there are; the shown one is ringed
    // and picks nothing.
    await tester.tap(find.byKey(const ValueKey('avatar-color-0')));
    await settle(tester);
    expect(harness.backend.sent.last.shuffleAvatar.hasChoice(), isFalse);
    await tester.tap(find.byKey(const ValueKey('avatar-color-8')));
    await settle(tester);
    expect(harness.backend.sent.last.shuffleAvatar.choice, 8);

    // The backend's new colors show at once, everywhere.
    harness.backend.workspace.apply(
      _workspace(
        person: _matt()
          ..avatarFrom = '#0EA5E9'
          ..avatarTo = '#4F46E5',
      ),
    );
    await settle(tester);
    final circles = [
      for (final avatar in find.byType(PersonAvatar).evaluate())
        ((_circle(tester, find.byWidget(avatar.widget)).gradient!
                as LinearGradient)
            .colors
            .first),
    ];
    expect(circles, isNotEmpty);
    expect(circles, everyElement(const Color(0xFF0EA5E9)));
    expect(circles, everyElement(const Color(0xFF0EA5E9)));
  });

  testWidgets('the Map has the person over the admins, with no lines', (
    tester,
  ) async {
    final harness = Harness(claktoryWorkspace()..person = _matt());
    await harness.show(tester, size: const Size(1440, 900));
    harness.page.value = AppPage.map;
    await settle(tester);
    final you = find.descendant(
      of: find.byType(MapView),
      matching: find.text('You'),
    );
    expect(you, findsOneWidget);
    final rivet = find.descendant(
      of: find.byType(MapView),
      matching: find.text('Rivet'),
    );
    expect(tester.getTopLeft(you).dy, lessThan(tester.getTopLeft(rivet).dy));
    harness.page.value = AppPage.agent;
    await settle(tester);
  });
}
