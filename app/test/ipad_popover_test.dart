// On an iPad's wide window, action sheets are popovers pointing at what
// they're about (docs/IDENTITY.md, "Popovers"): under it when there's room,
// over it when there isn't, inside the window, no Cancel (a tap outside
// cancels). A compact iPad window and a phone keep their sheets from the
// bottom; a Mac its dialog.
import 'dart:async';

import 'package:factory_app/controls.dart' show SelectOption;
import 'package:factory_app/identity.dart';
import 'package:factory_app/ipad/columns.dart' show WorkbenchTier;
import 'package:factory_app/mobile/device.dart' as device;
import 'package:factory_app/mobile/popover.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'ipad/as_ipad.dart';
import 'ipad/window_fixtures.dart';

const _actions = [
  SheetAction('Rename…', 'rename'),
  SheetAction('Delete…', 'delete', destructive: true),
];

/// A window of [size] with a 44pt button at [at] that asks what to do.
Future<Future<String?> Function()> _window(
  WidgetTester tester,
  Size size, {
  required Offset at,
}) async {
  tester.view
    ..physicalSize = size
    ..devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  late BuildContext button;
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => DefaultTextStyle(
        style: TextStyles.body,
        child: Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => Stack(
                children: [
                  Positioned(
                    left: at.dx,
                    top: at.dy,
                    child: Builder(
                      builder: (context) {
                        button = context;
                        return const SizedBox(
                          key: Key('button'),
                          width: 44,
                          height: 44,
                        );
                      },
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    ),
  );
  return () => showActions<String>(button, title: 'Notes', actions: _actions);
}

final _popover = find.byWidgetPredicate(
  (widget) => widget.runtimeType.toString() == '_Popover',
);

Rect _panel(WidgetTester tester) => tester.getRect(
  find
      .ancestor(
        of: find.text('Rename…'),
        matching: find.byWidgetPredicate(
          (widget) => widget is SizedBox && widget.width == popoverWidth,
        ),
      )
      .first,
);

void main() {
  for (final fixture in windowFixtures) {
    if (fixture.idiom != Idiom.pad || fixture.textFactor != 1) continue;
    final wide = fixture.tier != WorkbenchTier.compact;
    testWidgets(
      '$fixture: ${wide ? 'a popover under the button, inside the window' : 'a sheet from the bottom'}',
      (tester) async {
        asIPad();
        final ask = await _window(
          tester,
          fixture.size,
          at: Offset(fixture.size.width - 80, 60),
        );
        final chosen = ask();
        await tester.pumpAndSettle();
        if (!wide) {
          expect(_popover, findsNothing);
          expect(find.byType(PhoneSheet), findsOneWidget);
          expect(find.text('Cancel'), findsOneWidget);
          await tester.tap(find.text('Cancel'));
          await tester.pumpAndSettle();
          expect(await chosen, isNull);
          return;
        }
        expect(_popover, findsOneWidget);
        expect(find.byType(PhoneSheet), findsNothing);
        expect(find.text('Cancel'), findsNothing, reason: 'a tap outside');
        final panel = _panel(tester);
        final button = tester.getRect(find.byKey(const Key('button')));
        expect(panel.width, popoverWidth);
        expect(panel.top, greaterThan(button.bottom), reason: 'under it');
        expect(panel.left, greaterThanOrEqualTo(8));
        expect(panel.right, lessThanOrEqualTo(fixture.size.width - 8));
        // A finger's rows.
        expect(
          tester.getSize(find.text('Rename…')).height,
          greaterThanOrEqualTo(17),
        );
        await tester.tap(find.text('Delete…'));
        await tester.pumpAndSettle();
        expect(await chosen, 'delete');
        expect(_popover, findsNothing);
      },
    );
  }

  testWidgets('near the bottom, over the button; a tap outside or Esc '
      'cancels', (tester) async {
    asIPad();
    const size = Size(1366, 1024);
    final ask = await _window(tester, size, at: const Offset(600, 960));
    var chosen = ask();
    await tester.pumpAndSettle();
    final button = tester.getRect(find.byKey(const Key('button')));
    expect(_panel(tester).bottom, lessThan(button.top));

    await tester.tapAt(const Offset(100, 100));
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
    expect(_popover, findsNothing);

    chosen = ask();
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
    expect(sheetsPresented.value, 0);
  });

  for (final keyboard in [false, true]) {
    testWidgets('Esc closes it with a field focused behind it, and only it '
        '(hardware keyboard: $keyboard)', (tester) async {
      asIPad();
      device.hardwareKeyboard.value = keyboard;
      addTearDown(() => device.hardwareKeyboard.value = false);
      tester.view
        ..physicalSize = const Size(1366, 1024)
        ..devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final field = FocusNode();
      final text = TextEditingController();
      addTearDown(field.dispose);
      addTearDown(text.dispose);
      var escapedBehind = 0;
      late BuildContext button;
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xFF000000),
          builder: (context, _) => DefaultTextStyle(
            style: TextStyles.body,
            child: Overlay(
              initialEntries: [
                OverlayEntry(
                  builder: (context) => CallbackShortcuts(
                    bindings: {
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          escapedBehind++,
                    },
                    child: Column(
                      children: [
                        SizedBox(
                          width: 400,
                          height: 44,
                          child: EditableText(
                            controller: text,
                            focusNode: field,
                            autofocus: true,
                            style: TextStyles.body,
                            cursorColor: const Color(0xFF000000),
                            backgroundCursorColor: const Color(0xFF000000),
                          ),
                        ),
                        Builder(
                          builder: (context) {
                            button = context;
                            return const SizedBox(width: 44, height: 44);
                          },
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      );
      await tester.pump();
      expect(field.hasFocus, isTrue);

      final chosen = showActions<String>(button, actions: _actions);
      await tester.pump(const Duration(milliseconds: 300));
      expect(_popover, findsOneWidget);
      // With a hardware keyboard, keys go to the popover, not the field
      // behind it; without one, the field keeps focus (and the keyboard
      // on screen stays put).
      expect(field.hasFocus, !keyboard);

      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump(const Duration(milliseconds: 300));
      expect(await chosen, isNull);
      expect(_popover, findsNothing);
      expect(escapedBehind, 0);
      expect(field.hasFocus, isTrue);
    });
  }

  testWidgets('a choice too', (tester) async {
    asIPad();
    late BuildContext button;
    tester.view
      ..physicalSize = const Size(1366, 1024)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => Builder(
                builder: (context) {
                  button = context;
                  return const SizedBox.shrink();
                },
              ),
            ),
          ],
        ),
      ),
    );
    final chosen = showChoice<int>(
      button,
      options: const [SelectOption(1, 'One'), SelectOption(2, 'Two')],
      selected: 1,
    );
    await tester.pumpAndSettle();
    expect(_popover, findsOneWidget);
    await tester.tap(find.text('Two'));
    await tester.pumpAndSettle();
    expect(await chosen, 2);
  });

  for (final size in [const Size(1366, 1024), const Size(500, 1024)]) {
    testWidgets('VoiceOver can tap every row (${size.width} wide)', (
      tester,
    ) async {
      asIPad();
      final semantics = tester.ensureSemantics();
      final ask = await _window(tester, size, at: const Offset(200, 60));
      final chosen = ask();
      await tester.pumpAndSettle();
      for (final label in ['Rename…', 'Delete…']) {
        expect(
          tester.getSemantics(find.bySemanticsLabel(label)),
          isSemantics(isButton: true, hasTapAction: true),
        );
      }
      tester.semantics.tap(find.semantics.byLabel('Delete…'));
      await tester.pumpAndSettle();
      expect(await chosen, 'delete');
      semantics.dispose();
    });
  }

  testWidgets('VoiceOver can tap a choice', (tester) async {
    asIPad();
    final semantics = tester.ensureSemantics();
    tester.view
      ..physicalSize = const Size(500, 1024)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    late BuildContext button;
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => Builder(
                builder: (context) {
                  button = context;
                  return const SizedBox.shrink();
                },
              ),
            ),
          ],
        ),
      ),
    );
    final chosen = showChoice<int>(
      button,
      options: const [SelectOption(1, 'One'), SelectOption(2, 'Two')],
      selected: 1,
    );
    await tester.pumpAndSettle();
    tester.semantics.tap(find.semantics.byLabel('Two'));
    await tester.pumpAndSettle();
    expect(await chosen, 2);
    semantics.dispose();
  });

  testWidgets("a Mac's wide window keeps its dialog", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final ask = await _window(
      tester,
      const Size(1366, 1024),
      at: const Offset(600, 60),
    );
    unawaited(ask());
    await tester.pumpAndSettle();
    expect(_popover, findsNothing);
    expect(find.byType(PhoneSheet), findsNothing);
    expect(find.text('Cancel'), findsOneWidget);
  });
}
