// The extension bar in a short Stage Manager window (984 × 746): it keeps
// every glyph a finger's 44 × 44 and scrolls rather than cutting the last
// ones off, the edge it's cut at fading; in a tall window it fills the
// height and doesn't scroll.
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/extensions.dart' show ExtensionBar;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

/// As many extensions as a well-equipped agent has on.
const _names = [
  'Messaging',
  'Ask',
  'Checklist',
  'Comments',
  'Database',
  'Devices',
  'Memory',
  'Monitors',
  'Resources',
  'Secrets',
  'Tasks',
  'Highlights',
  'Own checkout',
  'Viewer',
  'Web',
  'Browser',
  'Map',
  'Usage',
];

String _id(String name) => name.toLowerCase().replaceAll(' ', '-');

Workspace _workspace() => Workspace(
  agents: [
    Agent(
      id: 'maya',
      name: 'Maya',
      modelAvailable: true,
      extensions: [
        for (final name in _names) AgentExtension(id: _id(name), enabled: true),
      ],
    ),
  ],
  selectedAgentId: 'maya',
  extensions: [for (final name in _names) Extension(id: _id(name), name: name)],
);

Future<Harness> _show(WidgetTester tester, Size size) async {
  asIPad();
  final harness = Harness(_workspace());
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: Builder(
        builder: (context) {
          final agent = AppScope.of(context).workspace.selectedAgent!;
          return Workbench(
            rail: const SizedBox(width: 40),
            sidebar: (width) => SizedBox(width: width),
            sidebarWidth: 248,
            agent: agent,
            extensionBar: ExtensionBar(agent: agent),
            content: const SizedBox.expand(key: ValueKey('conversation')),
          );
        },
      ),
    ),
  );
  await settle(tester);
  return harness;
}

void main() {
  final bar = find.byType(WorkbenchBarScroll);
  Finder glyph(String name) =>
      find.byWidgetPredicate((w) => w is GlyphButton && w.label == name);
  WorkbenchBarScrollState fades(WidgetTester tester) =>
      tester.state<WorkbenchBarScrollState>(bar);

  testWidgets('in a short window the bar scrolls, every glyph 44 × 44, '
      'and the last one opens its panel', (tester) async {
    final harness = await _show(tester, const Size(984, 746));
    // Nothing overflows: the glyphs past the foot are scrolled to.
    expect(tester.takeException(), isNull);
    expect(tester.getSize(bar).height, 746);
    // A finger's target each, 44 × 44 at least (Apple's minimum).
    expect(tester.getSize(find.byType(ExtensionBar)).width, 44);
    for (final name in _names) {
      final size = tester.getSize(glyph(name));
      expect(size.width, greaterThanOrEqualTo(44), reason: name);
      expect(size.height, greaterThanOrEqualTo(44), reason: name);
    }
    final last = glyph(_names.last);
    expect(tester.getRect(last).top, greaterThan(746));
    expect(fades(tester).fadesBottom, isTrue);
    expect(fades(tester).fadesTop, isFalse);

    await tester.drag(bar, const Offset(0, -400));
    await pumpFrames(tester);
    expect(tester.getRect(last).bottom, lessThanOrEqualTo(746));
    expect(fades(tester).fadesBottom, isFalse);
    expect(fades(tester).fadesTop, isTrue);

    await tester.tap(last);
    await settle(tester);
    expect(harness.last.openPanel.extensionId, _id(_names.last));
  });

  testWidgets('in a tall window it fills the height and stays put', (
    tester,
  ) async {
    await _show(tester, const Size(984, 1366));
    expect(tester.takeException(), isNull);
    final scroll = tester
        .state<ScrollableState>(
          find.descendant(of: bar, matching: find.byType(Scrollable)),
        )
        .position;
    expect(scroll.maxScrollExtent, 0);
    // Its background and hairline run to the foot.
    expect(tester.getSize(find.byType(ExtensionBar)).height, 1366);
    expect(fades(tester).fadesBottom, isFalse);
    expect(fades(tester).fadesTop, isFalse);
  });
}
