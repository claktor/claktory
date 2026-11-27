import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/zoom.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({Map<String, double> zoom = const {}}) => Workspace(
  agents: [Agent(id: 'a1', name: 'Mike', modelAvailable: true)],
  selectedAgentId: 'a1',
  settings: Settings(userName: 'Matt', zoom: zoom.entries),
);

/// Zoom's keys and words are a Mac's.
final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

/// The size the app lays itself out in.
Size _laidOut(WidgetTester tester) =>
    MediaQuery.sizeOf(tester.element(find.byType(Overlay).last));

Future<void> _press(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool shift = false,
}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await tester.pump();
}

void main() {
  group('levels', () {
    test("steps through Chrome's levels and stops at either end", () {
      expect(stepZoom(1, 1), 1.1);
      expect(stepZoom(1, -1), 0.9);
      expect(stepZoom(0.67, -1), 0.5);
      expect(stepZoom(0.5, -1), 0.5);
      expect(stepZoom(2, 1), 2);
      // A level between steps goes to the next one.
      expect(stepZoom(1.2, 1), 1.25);
      expect(stepZoom(1.2, -1), 1.1);
      expect(zoomPercent(0.67), '67%');
      expect(zoomPercent(1.25), '125%');
    });
  });

  group('the window', () {
    testWidgets("lays out at this device's zoom, scaled to fill it", (
      tester,
    ) async {
      final harness = Harness(_workspace(zoom: {'local': 1.5, 'phone': 0.5}));
      await harness.show(tester);
      await settle(tester);
      expect(_laidOut(tester), const Size(800, 600));
      // Drawn at 1.5×: a 44-point band is 66 on the screen.
      final rect = tester.getRect(find.byType(Overlay).last);
      expect(rect, const Rect.fromLTWH(0, 0, 1200, 900));

      await harness.apply(tester, _workspace(zoom: {'phone': 0.5}));
      expect(_laidOut(tester), const Size(1200, 900));
    }, variant: _mac);

    testWidgets("a paired device reads its own id's zoom", (tester) async {
      final harness = Harness(_workspace());
      harness.backend.deviceId = 'laptop';
      harness.backend.workspace.apply(
        _workspace(zoom: {'local': 2, 'laptop': 0.75}),
      );
      await harness.show(tester);
      await settle(tester);
      expect(_laidOut(tester), const Size(1600, 1200));
      await tester.tap(find.byType(EditableText).first);
      await tester.pump();
      await _press(tester, LogicalKeyboardKey.equal);
      expect(harness.last.setZoom.zoom, 0.8);
    }, variant: _mac);

    testWidgets('clicks land where things are drawn', (tester) async {
      final harness = Harness(_workspace(zoom: {'local': 1.25}));
      await harness.show(tester);
      await settle(tester);
      await tester.tap(find.text('Mike').first);
      await settle(tester);
      expect(harness.backend.sent, isNotEmpty);
    }, variant: _mac);
  });

  group('keys', () {
    testWidgets('⌘+, ⌘= and ⌘− step, ⌘0 goes back to actual size', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await harness.show(tester);
      await settle(tester);
      // The message box has the keys, as a click gives it.
      await tester.tap(find.byType(EditableText).first);
      await tester.pump();

      await _press(tester, LogicalKeyboardKey.equal);
      expect(harness.last.setZoom.zoom, 1.1);
      // Shown at once, before the backend says so.
      expect(_laidOut(tester).width, closeTo(1200 / 1.1, 0.01));
      await harness.apply(tester, _workspace(zoom: {'local': 1.1}));

      await _press(tester, LogicalKeyboardKey.equal, shift: true);
      expect(harness.last.setZoom.zoom, 1.25);
      await harness.apply(tester, _workspace(zoom: {'local': 1.25}));

      await _press(tester, LogicalKeyboardKey.minus);
      expect(harness.last.setZoom.zoom, 1.1);
      await harness.apply(tester, _workspace(zoom: {'local': 1.1}));

      await _press(tester, LogicalKeyboardKey.digit0);
      expect(harness.last.setZoom.zoom, 1);
      await harness.apply(tester, _workspace());
      expect(_laidOut(tester), const Size(1200, 900));
    }, variant: _mac);

    testWidgets('a refused zoom goes back to what the backend keeps', (
      tester,
    ) async {
      final harness = Harness(_workspace(zoom: {'local': 0.9}));
      await harness.show(tester);
      await settle(tester);
      harness.backend.refuse = 'Zoom must be between 50% and 200%.';
      final zoom = ZoomScope.of(tester.element(find.byType(Overlay).last))!;
      await zoom.zoomIn();
      await tester.pump();
      expect(zoom.level, 0.9);
      expect(_laidOut(tester).width, closeTo(1200 / 0.9, 0.01));
    }, variant: _mac);

    testWidgets("the View menu's items zoom", (tester) async {
      final harness = Harness(_workspace(zoom: {'local': 1.5}));
      await harness.show(tester);
      await settle(tester);
      Future<void> menu(String method) =>
          tester.binding.defaultBinaryMessenger.handlePlatformMessage(
            'factory/menu',
            const StandardMethodCodec().encodeMethodCall(MethodCall(method)),
            (_) {},
          );
      await menu('zoomOut');
      expect(harness.last.setZoom.zoom, 1.25);
      await harness.apply(tester, _workspace(zoom: {'local': 1.25}));
      await menu('zoomIn');
      expect(harness.last.setZoom.zoom, 1.5);
      await harness.apply(tester, _workspace(zoom: {'local': 1.5}));
      await menu('actualSize');
      expect(harness.last.setZoom.zoom, 1);
    }, variant: _mac);

    testWidgets("the window's top band follows the zoom", (tester) async {
      final calls = <MethodCall>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        AppZoom.window,
        (call) async {
          if (call.method == 'band') calls.add(call);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          AppZoom.window,
          null,
        ),
      );
      final harness = Harness(_workspace());
      await harness.show(tester);
      await settle(tester);
      expect(calls, isEmpty);
      await harness.apply(tester, _workspace(zoom: {'local': 0.5}));
      expect(calls.single.method, 'band');
      expect(calls.single.arguments, 22.0);
    }, variant: _mac);
  });

  group('Settings → Display', () {
    testWidgets('shows the zoom, changes it and goes back to actual size', (
      tester,
    ) async {
      final harness = Harness(_workspace(zoom: {'local': 1.25}));
      await harness.show(tester);
      harness.page.value = AppPage.settings;
      await settle(tester);
      // Settings builds its sections as they scroll into view.
      await revealInSettings(tester, find.text('Actual size'));
      await settle(tester);
      expect(find.text('125%'), findsOneWidget);
      expect(
        find.text(
          'Only on this Mac. Press ⌘+ or ⌘− to zoom, and ⌘0 for actual size.',
        ),
        findsOneWidget,
      );

      await tester.tap(find.text('Actual size'));
      await tester.pump();
      expect(harness.last.setZoom.zoom, 1);
      await harness.apply(tester, _workspace());
      expect(find.text('100%'), findsOneWidget);
      expect(find.text('Actual size'), findsNothing);

      await tester.tap(find.text('100%'));
      await settle(tester);
      await tester.tap(find.text('90%').last);
      await settle(tester);
      expect(harness.last.setZoom.zoom, 0.9);
    }, variant: _mac);
  });
}
