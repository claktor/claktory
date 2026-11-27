// The header's Find finds in the pane being read (Squalo's ST14, Zucchero
// rows 613–615): Settings beside the conversation on a full-width iPad by
// touch, the magnifier opened the conversation's find even after a tap in
// Settings, so "Sleep" found nothing. Now: the pane last tapped in the
// agent's view, else the conversation, rather than a pane only beside it
// (row 101). And while a pane is expanded over the window, what's under it
// (the header's ⋯, Find… in it) is out of VoiceOver's reach.
import 'package:factory_app/conversation_find.dart' show ConversationFind;
import 'package:factory_app/find.dart' show FindSession, FindSessions;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/panes.dart' show ExpandedPane;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/shell.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart' show SemanticsAction, SemanticsNode;
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
      child: Shell(
        rail: Semantics(
          button: true,
          label: 'Rail tile',
          onTap: () {},
          child: const SizedBox(width: 56, height: 56),
        ),
      ),
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

/// Find… in the agent's ⋯ (area 4), where the header's magnifier was.
Future<void> _openFind(WidgetTester tester) async {
  await tester.tap(glyphButton('More options for Builder'));
  await pumpFrames(tester);
  await tester.tap(find.text('Find…'));
}

/// Whether VoiceOver's tree, as the window sends it, has a node [label]d.
/// A pane in the window's overlay (an expanded one) is grafted under the
/// node of the place it came from (its OverlayPortal's traversal parent);
/// with that node gone, the window drops the pane and all in it, as on an
/// iPad (row 665), so it isn't counted here either.
bool _inTree(WidgetTester tester, String label) {
  final parents = <Object>{};
  void collect(SemanticsNode node) {
    if (node.traversalParentIdentifier case final Object id?) parents.add(id);
    node.visitChildren((child) {
      collect(child);
      return true;
    });
  }

  var found = false;
  bool visit(SemanticsNode node) {
    // Grafted under a node that isn't there: dropped, with all in it.
    if (node.traversalChildIdentifier case final Object id?
        when !parents.contains(id)) {
      return true;
    }
    if (node.label == label) found = true;
    node.visitChildren(visit);
    return !found;
  }

  for (final view in tester.binding.renderViews) {
    final root = view.owner?.semanticsOwner?.rootSemanticsNode;
    if (root != null) {
      collect(root);
      visit(root);
    }
  }
  return found;
}

void main() {
  testWidgets('nothing tapped yet: Find opens the conversation\'s find, '
      'not the pane beside it', (tester) async {
    await _show(tester);
    await _openFind(tester);
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
    await _openFind(tester);
    await pumpFrames(tester);
    expect(_conversation().isOpen.value, isFalse);

    await tester.tap(find.text('Shipped.'));
    await pumpFrames(tester);
    expect(FindSessions.instance.pressedWithin(view), isNull);
    await _openFind(tester);
    await pumpFrames(tester);
    expect(_conversation().isOpen.value, isTrue);
  });

  // Row 665: hidden with what's under it, the pane itself was dropped too
  // (BlockSemantics in the overlay took the node it's grafted under).
  testWidgets('a pane expanded over the window is read alone: its band and '
      'its own controls, never the header\'s ⋯ (Find… in it) under it; collapsed, the '
      'window again', (tester) async {
    final handle = tester.ensureSemantics();
    await _show(tester);
    expect(_inTree(tester, 'More options for Builder'), isTrue);
    await tester.tap(glyphButton('Expand Settings'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    // Its fade in done.
    await tester.pump(const Duration(seconds: 1));
    for (final label in [
      'Back to Builder',
      'Collapse Settings',
      'Close',
      // Settings' own: its search, an index row, a section's setting.
      'Search Settings',
      'Display',
      'Profile',
    ]) {
      expect(_inTree(tester, label), isTrue, reason: label);
    }
    expect(_inTree(tester, 'More options for Builder'), isFalse);
    expect(_inTree(tester, 'Message Builder\nSend'), isFalse);
    expect(_inTree(tester, 'Hide sidebar'), isFalse);
    expect(_inTree(tester, 'Rail tile'), isFalse);
    // Pressed by VoiceOver, Back goes back, as a click does (row 1599).
    final back = tester.semantics.find(
      find.bySemanticsLabel('Back to Builder'),
    );
    expect(back.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
    tester.binding.rootPipelineOwner.semanticsOwner?.performAction(
      back.id,
      SemanticsAction.tap,
    );
    // Some pipeline owners are the views'.
    for (final view in tester.binding.renderViews) {
      view.owner?.semanticsOwner?.performAction(back.id, SemanticsAction.tap);
    }
    await settle(tester);
    await tester.pump();
    expect(find.byType(ExpandedPane), findsNothing);
    expect(_inTree(tester, 'More options for Builder'), isTrue);
    expect(_inTree(tester, 'Hide sidebar'), isTrue);
    expect(_inTree(tester, 'Rail tile'), isTrue);
    expect(_inTree(tester, 'Collapse Settings'), isFalse);
    handle.dispose();
  });
}
