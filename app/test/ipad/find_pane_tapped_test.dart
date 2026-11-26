// The header's Find finds in the pane being read (Squalo's ST14, Zucchero
// rows 613–615): Settings beside the conversation on a full-width iPad by
// touch, the magnifier opened the conversation's find even after a tap in
// Settings, so "Sleep" found nothing. Now: the pane last tapped in the
// agent's view, else the conversation, rather than a pane only beside it
// (row 101). And while a pane is expanded over the window, what's under it
// (the header's Find with it) is out of VoiceOver's reach.
import 'package:factory_app/conversation_find.dart' show ConversationFind;
import 'package:factory_app/find.dart' show FindSession, FindSessions;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/panes.dart' show ExpandedPane;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/shell.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart' show SemanticsNode;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show beside, glyphButton, snapshot;
import 'as_ipad.dart';

/// Builder with Settings open beside its conversation, on a 13-inch iPad.
Future<Harness> _show(WidgetTester tester) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(snapshot(layout: beside('settings', '')))
    ..backend.appliesLayouts = true;
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          timestampMs: Int64(1000),
          assistant: AssistantText(text: 'Shipped.'),
        ),
      ],
    ),
  );
  await harness.show(
    tester,
    size: const Size(1376, 1032),
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  expect(find.byType(SettingsView), findsOneWidget);
  return harness;
}

FindSession _conversation() => FindSessions.instance.sessions.value.singleWhere(
  (session) => session.target is ConversationFind,
);

/// The searchable session inside Settings, whatever kind it is (a find bar
/// over the page today, its own search field after area 3).
FindSession _settings(WidgetTester tester) {
  final view = tester.element(find.byType(SettingsView));
  return FindSessions.instance.sessions.value.singleWhere(
    (session) =>
        FindSessions.instance.within(
          view,
          only: (target) => identical(target, session.target),
        ) !=
        null,
  );
}

Finder get _find => glyphButton('Find');

/// Whether VoiceOver's tree, as the window sends it, has a node [label]d.
bool _inTree(WidgetTester tester, String label) {
  var found = false;
  bool visit(SemanticsNode node) {
    if (node.label == label) found = true;
    node.visitChildren(visit);
    return !found;
  }

  for (final view in tester.binding.renderViews) {
    final root = view.owner?.semanticsOwner?.rootSemanticsNode;
    if (root != null) visit(root);
  }
  return found;
}

void main() {
  testWidgets('nothing tapped yet: Find opens the conversation\'s find, '
      'not the pane beside it', (tester) async {
    await _show(tester);
    await tester.tap(_find);
    await pumpFrames(tester);
    expect(_conversation().isOpen.value, isTrue);
    expect(_settings(tester).isOpen.value, isFalse);
  });

  testWidgets('after a tap in Settings, Find finds in Settings; after a tap '
      'in the conversation, there again', (tester) async {
    await _show(tester);
    final view = tester.element(find.byType(SettingsView));
    await tester.tapAt(tester.getCenter(find.byType(SettingsView)));
    await pumpFrames(tester);
    expect(FindSessions.instance.pressedWithin(view), same(_settings(tester)));
    await tester.tap(_find);
    await pumpFrames(tester);
    expect(_conversation().isOpen.value, isFalse);

    await tester.tap(find.text('Shipped.'));
    await pumpFrames(tester);
    expect(FindSessions.instance.pressedWithin(view), isNull);
    await tester.tap(_find);
    await pumpFrames(tester);
    expect(_conversation().isOpen.value, isTrue);
  });

  // Row 665: the BlockSemantics ST14 put around the expanded pane hid the
  // pane's own controls from VoiceOver too (Back, Collapse, Close and all
  // of Settings), on an iPad and a Mac. It's gone until what's under the
  // pane can be hidden on its own; meanwhile the header stays readable
  // under it, as before ST14.
  testWidgets('a pane expanded over the window blocks nothing from '
      'VoiceOver: the window stays readable, collapsed too', (tester) async {
    final handle = tester.ensureSemantics();
    await _show(tester);
    expect(_inTree(tester, 'Find'), isTrue);
    await tester.tap(glyphButton('Expand Settings'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    expect(
      find.ancestor(
        of: find.text('Back to Builder'),
        matching: find.byType(BlockSemantics),
      ),
      findsNothing,
    );
    expect(_inTree(tester, 'Find'), isTrue);
    await tester.tap(find.text('Back to Builder'));
    await settle(tester);
    expect(_inTree(tester, 'Find'), isTrue);
    handle.dispose();
  });
}
