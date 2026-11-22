import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/selection.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'resources_test.dart' as resources;

/// What's copied to the clipboard, as the app copies it.
List<String> watchClipboard(WidgetTester tester) {
  final copied = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        copied.add((call.arguments as Map)['text'] as String);
      }
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
  return copied;
}

/// A right click on [target], as on a Mac: it selects the word there and
/// opens the menu.
Future<void> rightClick(
  WidgetTester tester,
  Finder target, {
  Offset? at,
}) async {
  final gesture = await tester.startGesture(
    at ?? tester.getCenter(target),
    kind: PointerDeviceKind.mouse,
    buttons: kSecondaryMouseButton,
  );
  await gesture.up();
  await tester.pump();
}

Finder get copyItem =>
    find.descendant(of: find.byType(CopyMenu), matching: find.text('Copy'));

void main() {
  final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

  Rect menuRect(WidgetTester tester) => tester.getRect(
    find.descendant(
      of: find.byType(CopyMenu),
      matching: find.byType(IntrinsicWidth),
    ),
  );

  testWidgets('the Copy menu is compact and opens at the pointer', (
    tester,
  ) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            assistant: AssistantText(text: 'Hello'),
          ),
        ],
      ),
    );
    await harness.show(tester);
    // Its text joins the selection a frame or two after it shows.
    await pumpFrames(tester);
    final at = tester.getCenter(find.text('Hello'));
    await rightClick(tester, find.text('Hello'), at: at);
    expect(menuRect(tester).width, lessThan(200));
    expect(menuRect(tester).topLeft, at);
  });

  testWidgets("near the window's edges the Copy menu stays inside", (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1800, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    Future<void> menuAt(Offset anchor) => tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => Stack(
          children: [
            Positioned.fill(
              child: CopyMenu(anchor: anchor, onCopy: () {}, onDismiss: () {}),
            ),
          ],
        ),
      ),
    );
    await menuAt(const Offset(1795, 895));
    final rect = menuRect(tester);
    expect(rect.width, lessThan(200));
    expect(rect.right, 1800);
    expect(rect.bottom, 900);
  });

  testWidgets(
    "in the conversation, a right click selects a word and Copy copies it",
    (tester) async {
      final copied = watchClipboard(tester);
      final harness = Harness(workspace(agents: [builder(working: false)]));
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              assistant: AssistantText(text: 'Hello'),
            ),
            Entry(
              seq: Int64(2),
              toolCall: ToolCall(
                callId: 'c1',
                name: 'bash',
                input: '{"command": "ls"}',
                output: 'Cargo.toml',
                state: ToolCallState.TOOL_CALL_STATE_DONE,
              ),
            ),
          ],
        ),
      );
      await harness.show(tester);
      // Its text joins the selection a frame or two after it shows.
      await pumpFrames(tester);
      await rightClick(tester, find.text('Hello'));
      expect(find.byType(CopyMenu), findsOneWidget);
      expect(find.text('⌘C'), findsOneWidget);
      await tester.tap(copyItem);
      await tester.pump();
      expect(copied, ['Hello']);
      expect(find.byType(CopyMenu), findsNothing, reason: 'it closes');

      // A tool call's output, once it's open: dragged over, then copied
      // with a right click on the selection.
      await tester.tap(find.text('bash'));
      await settle(tester);
      final output = tester.getRect(
        find.text('Cargo.toml', findRichText: true).last,
      );
      final drag = await tester.startGesture(
        output.centerLeft + const Offset(1, 0),
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await drag.moveTo(output.centerRight - const Offset(1, 0));
      await tester.pump();
      await drag.up();
      await tester.pump(const Duration(milliseconds: 400));
      await rightClick(
        tester,
        find.byType(Scrollable).first,
        at: output.center,
      );
      await tester.tap(copyItem);
      await tester.pump();
      expect(copied.last, 'Cargo.toml');
    },
    variant: mac,
  );

  testWidgets('a click elsewhere closes the menu without copying', (
    tester,
  ) async {
    final copied = watchClipboard(tester);
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            assistant: AssistantText(text: 'Hello'),
          ),
        ],
      ),
    );
    await harness.show(tester);
    // Its text joins the selection a frame or two after it shows.
    await pumpFrames(tester);
    await rightClick(tester, find.text('Hello'));
    expect(find.byType(CopyMenu), findsOneWidget);
    await tester.tapAt(const Offset(600, 880));
    await tester.pump();
    expect(find.byType(CopyMenu), findsNothing);
    expect(copied, isEmpty);
  }, variant: mac);

  testWidgets("panels' text copies too", (tester) async {
    final copied = watchClipboard(tester);
    await resources.show(tester, resources.sample());
    await rightClick(tester, resources.inPanel('Processors').first);
    await tester.tap(copyItem);
    await tester.pump();
    expect(copied, ['Processors']);
  }, variant: mac);

  testWidgets(
    "settings' descriptions copy; buttons aren't part of a selection",
    (tester) async {
      final copied = watchClipboard(tester);
      final harness = Harness(workspace(agents: [builder(working: false)]));
      await harness.show(tester);
      // As a page of its own (a phone, or no agent to show it beside),
      // with its Done button; beside a conversation it's the same view.
      harness.page.value = AppPage.settings;
      await settle(tester);
      // Text joins the selection after the frame that builds it.
      await tester.pump();
      // Under Profile and Providers.
      await tester.scrollUntilVisible(
        find.text('Defaults'),
        200,
        scrollable: find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.pump();
      final heading = find.text('Defaults');
      await tester.scrollUntilVisible(
        heading,
        200,
        scrollable: find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.pump();
      await rightClick(
        tester,
        heading,
        at: tester.getTopLeft(heading) + const Offset(8, 8),
      );
      await tester.tap(copyItem);
      await tester.pump();
      expect(copied, ['Defaults']);

      await tester.scrollUntilVisible(
        find.textContaining('A soft glow'),
        200,
        scrollable: find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.pump();
      final glow = find.textContaining('A soft glow');
      await rightClick(
        tester,
        glow,
        at: tester.getTopLeft(glow) + const Offset(40, 8),
      );
      await tester.tap(copyItem);
      await tester.pump();
      expect(copied.last, 'soft');

      // A button's label isn't selected: nothing to copy, so no menu.
      await rightClick(tester, find.text('Done'));
      expect(find.byType(CopyMenu), findsNothing);

      // Everything on the page, copied, leaves the buttons' labels out.
      await rightClick(
        tester,
        glow,
        at: tester.getTopLeft(glow) + const Offset(40, 8),
      );
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.pump();
      expect(copied.last, contains('A soft glow'));
      expect(copied.last, contains('Defaults'));
      expect(copied.last, isNot(contains('Sign out')));
    },
    variant: mac,
  );
}
