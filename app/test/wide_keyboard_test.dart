// The wide window with an on-screen keyboard up, as on an Android tablet
// (Sale's row 12): the conversation and a pane beside it end above the
// keyboard, so the message box and Send stay in sight; the rail and the
// sidebar stay put under it, and nothing moves on a Mac, which has none.
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show workspace;

final _box = find.descendant(
  of: find.byType(MessageBox),
  matching: find.byType(EditableText),
);

/// A 10-inch Android tablet in landscape, its window wide enough for the
/// sidebar and panes.
const _tablet = Size(1280, 800);
const _keyboard = 370.0;

void main() {
  testWidgets('the keyboard up: the message box and Send sit above it, '
      'the box keeps the focus', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: _tablet);
    await pumpFrames(tester);
    await tester.tap(_box);
    await pumpFrames(tester);
    final sidebar = tester.getRect(find.byType(Sidebar));
    expect(
      tester.getRect(_box).bottom,
      greaterThan(_tablet.height - _keyboard),
    );

    tester.view.viewInsets = const FakeViewPadding(bottom: _keyboard);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final above = _tablet.height - _keyboard;
    expect(tester.getRect(_box).bottom, lessThanOrEqualTo(above));
    // The whole box, its buttons (Send on a touch screen) included.
    expect(
      tester.getRect(find.byType(MessageBox)).bottom,
      lessThanOrEqualTo(above),
    );
    // Still the one being written in: the keyboard's coming rebuilt
    // nothing that would take the focus (and the keyboard) away.
    expect(
      tester.state<EditableTextState>(_box).widget.focusNode.hasFocus,
      isTrue,
    );
    // The sidebar stays where it was, under the keyboard.
    expect(tester.getRect(find.byType(Sidebar)), sidebar);

    tester.view.resetViewInsets();
    await pumpFrames(tester);
    expect(tester.getRect(_box).bottom, greaterThan(above));
  });

  testWidgets('a pane beside the conversation ends above the keyboard too', (
    tester,
  ) async {
    final harness = Harness(
      workspace(openPanel: 'checklist')..selectedAgentId = 'mike',
    );
    await harness.show(tester, size: _tablet);
    await pumpFrames(tester);
    final pane = find.byKey(const ValueKey('checklist/mike'));
    expect(pane, findsOneWidget);
    expect(tester.getRect(pane).bottom, _tablet.height);

    tester.view.viewInsets = const FakeViewPadding(bottom: _keyboard);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(tester.getRect(pane).bottom, _tablet.height - _keyboard);
  });
}
