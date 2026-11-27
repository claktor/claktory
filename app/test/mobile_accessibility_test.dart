// What an iPhone needs of every screen (research mobile.md §7):
// every tap target at least 44×44pt, every one labelled, and nothing
// hidden behind the screen in view still reachable by VoiceOver. Each
// screen's check is on once its phase lands: the list and drawer with
// 1A, the conversation with 1B.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/identity.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'mike',
      projectId: 'p1',
      name: 'Mike',
      role: 'Coordinator',
      modelAvailable: true,
      model: 'anthropic/claude-opus-5-5',
      extensions: [AgentExtension(id: 'checklist', enabled: true)],
    ),
  ],
  extensions: [
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  ],
  selectedAgentId: 'mike',
);

void main() {
  Future<Harness> phone(WidgetTester tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    return harness;
  }

  Future<void> openMike(WidgetTester tester) async {
    if (find.byType(AgentView).evaluate().isEmpty) {
      await tester.tap(find.text('Mike').first);
      await pumpFrames(tester);
    }
    expect(find.byType(AgentView), findsOneWidget);
  }

  testWidgets('the drawer', (tester) async {
    final semantics = tester.ensureSemantics();
    await phone(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    // Denser than iOS's 44×44 by Matt's choice (docs/IDENTITY.md, "Phone"):
    // rows of one line are 36pt tall, spanning the drawer; a button at a
    // row's end is 44pt wide and as tall as the row.
    await expectLater(
      tester,
      meetsGuideline(
        const MinimumTapTargetGuideline(
          size: Size(44, 36),
          link: 'docs/IDENTITY.md',
        ),
      ),
    );
    await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
    // The conversation under it is out of reach.
    expect(find.semantics.byLabel('More options for Mike'), findsNothing);
    expect(find.semantics.byLabel('Close menu'), findsOne);
    semantics.dispose();
  });

  testWidgets("the drawer's search is the box up to Clear: a tap from "
      'VoiceOver brings the keyboard, and it keeps its name once typed in', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await phone(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    final search = find.semantics.byLabel('Search');
    expect(search, findsOne);
    final node = search.evaluate().single;
    expect(node.rect.height, greaterThanOrEqualTo(36));
    expect(node.rect.width, greaterThan(200), reason: 'the box, not its words');

    tester.semantics.tap(search);
    await pumpFrames(tester);
    final field = find.descendant(
      of: find.byWidgetPredicate(
        (widget) => widget is TextField && widget.placeholder == 'Search',
      ),
      matching: find.byType(EditableText),
    );
    expect(tester.widget<EditableText>(field.first).focusNode.hasFocus, isTrue);

    await tester.enterText(field.first, 'Mi');
    await pumpFrames(tester);
    expect(find.semantics.byLabel('Search'), findsOne);
    // Clear is a button of its own, not part of the field.
    expect(find.semantics.byLabel('Clear search'), findsOne);
    semantics.dispose();
  });

  testWidgets('a conversation', (tester) async {
    final semantics = tester.ensureSemantics();
    await phone(tester);
    await openMike(tester);
    await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
    await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
    semantics.dispose();
  });

  testWidgets("what's behind a conversation is out of VoiceOver's reach", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await phone(tester);
    await openMike(tester);
    expect(find.bySemanticsLabel('Settings'), findsNothing);
    expect(find.bySemanticsLabel('Usage'), findsNothing);
    semantics.dispose();
  });

  group("a phone's faint text is read: at least 4.5:1", () {
    double contrast(Color a, Color b) {
      final (light, dark) = a.computeLuminance() > b.computeLuminance()
          ? (a.computeLuminance(), b.computeLuminance())
          : (b.computeLuminance(), a.computeLuminance());
      return (light + 0.05) / (dark + 0.05);
    }

    Future<Palette> paletteOf(
      WidgetTester tester, {
      required Size size,
      required Brightness brightness,
    }) async {
      late Palette palette;
      await tester.pumpWidget(
        MediaQuery(
          data: MediaQueryData(size: size, platformBrightness: brightness),
          child: Builder(
            builder: (context) {
              palette = Palette.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      return palette;
    }

    for (final brightness in Brightness.values) {
      testWidgets('${brightness.name}, on both surfaces', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final phone = await paletteOf(
          tester,
          size: const Size(393, 852),
          brightness: brightness,
        );
        for (final surface in [phone.canvas, phone.sidebar]) {
          expect(contrast(phone.textFaint, surface), greaterThanOrEqualTo(4.5));
        }
        // Still fainter than muted text, so the two read apart.
        expect(
          contrast(phone.textMuted, phone.canvas),
          greaterThan(contrast(phone.textFaint, phone.canvas)),
        );
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        final mac = await paletteOf(
          tester,
          size: const Size(1280, 800),
          brightness: brightness,
        );
        expect(
          mac.textFaint,
          brightness == Brightness.dark
              ? const Color(0xFF71717A)
              : const Color(0xFF8B8B94),
          reason: "the Mac's own",
        );
      });
    }
  });
}
