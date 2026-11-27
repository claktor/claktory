// A popover beside what it's about (presentPopover's `beside`, for
// pinned agents): on an iPad's wide window a sidebar row's touch-and-hold
// menu opens to the row's right, its arrow on the popover's left edge at
// the row's middle, as iPadOS shows a sidebar row's menu. Where the right
// hasn't a whole popover's width, it's under or over as before; in a
// right-to-left window too (not mirrored yet, so the arrow never points
// the wrong way).
//
// SHOTS=dir draws the 13" landscape case as a PNG there, to look at.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/identity.dart';
import 'package:factory_app/mobile/popover.dart';
import 'package:flutter/rendering.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'as_ipad.dart';

const _actions = [
  SheetAction('Pin', 'pin'),
  SheetAction('Move up', 'up'),
  SheetAction('Agent settings…', 'settings'),
];

final _shot = GlobalKey();

/// A window of [size] with a sidebar row [rowWidth] wide at [top] that
/// asks what to do, beside it unless [beside] is false.
Future<Future<String?> Function()> _window(
  WidgetTester tester,
  Size size, {
  double top = 200,
  double rowWidth = 300,
  TextDirection direction = TextDirection.ltr,
  bool beside = true,
}) async {
  tester.view
    ..physicalSize = size
    ..devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  late BuildContext row;
  await tester.pumpWidget(
    RepaintBoundary(
      key: _shot,
      child: WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => Directionality(
          textDirection: direction,
          child: DefaultTextStyle(
            style: TextStyles.body,
            child: ColoredBox(
              color: Palette.of(context).canvas,
              child: Overlay(
                initialEntries: [
                  OverlayEntry(
                    builder: (context) => Stack(
                      children: [
                        Positioned(
                          left: 0,
                          top: top,
                          child: Builder(
                            builder: (context) {
                              row = context;
                              return ColoredBox(
                                key: const Key('row'),
                                color: Palette.of(context).sidebar,
                                child: SizedBox(
                                  width: rowWidth,
                                  height: 36,
                                  child: const Text('Bruno'),
                                ),
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
        ),
      ),
    ),
  );
  return () => showActions<String>(
    row,
    title: 'Bruno · Project Manager · Claktory',
    actions: _actions,
    beside: beside,
  );
}

Rect _panel(WidgetTester tester) => tester.getRect(
  find
      .ancestor(
        of: find.text('Pin'),
        matching: find.byWidgetPredicate(
          (widget) => widget is SizedBox && widget.width == popoverWidth,
        ),
      )
      .first,
);

/// The arrow's painted box.
Rect _arrow(WidgetTester tester) => tester.getRect(
  find
      .byWidgetPredicate(
        (widget) =>
            widget is CustomPaint &&
            widget.painter.runtimeType.toString() == '_ArrowPainter',
      )
      .first,
);

Future<void> _draw(WidgetTester tester, String name) async {
  final dir = Platform.environment['SHOTS'];
  if (dir == null) return;
  await tester.runAsync(() async {
    final boundary =
        _shot.currentContext!.findRenderObject()! as RenderRepaintBoundary;
    final image = await boundary.toImage();
    final png = await image.toByteData(format: ui.ImageByteFormat.png);
    File('$dir/$name.png').writeAsBytesSync(png!.buffer.asUint8List());
  });
}

void main() {
  testWidgets('13" landscape: beside the row, the arrow at the row\'s '
      'middle near the popover\'s top; an action answers', (tester) async {
    asIPad();
    final ask = await _window(tester, const Size(1366, 1024));
    final chosen = ask();
    await tester.pumpAndSettle();
    final row = tester.getRect(find.byKey(const Key('row')));
    final panel = _panel(tester);
    final arrow = _arrow(tester);
    expect(panel.left, greaterThan(row.right), reason: 'to its right');
    expect(panel.top, lessThanOrEqualTo(row.top));
    expect(row.center.dy - panel.top, lessThan(40), reason: 'near its top');
    expect(arrow.right, closeTo(panel.left + 1, 0.01));
    expect(arrow.center.dy, row.center.dy);
    expect(arrow.width, lessThan(arrow.height), reason: 'pointing left');
    await _draw(tester, 'popover_beside_13in');
    await tester.tap(find.text('Move up'));
    await tester.pumpAndSettle();
    expect(await chosen, 'up');
  });

  testWidgets('a row near the bottom: kept inside the window, the arrow '
      'still at the row', (tester) async {
    asIPad();
    const size = Size(1194, 834);
    final ask = await _window(tester, size, top: 790);
    final chosen = ask();
    await tester.pumpAndSettle();
    final row = tester.getRect(find.byKey(const Key('row')));
    final panel = _panel(tester);
    expect(panel.left, greaterThan(row.right));
    expect(panel.bottom, lessThanOrEqualTo(size.height - 8));
    expect(panel.top, lessThan(row.top));
    // As near the row's middle as the popover's corner lets it.
    final arrow = _arrow(tester);
    expect(arrow.center.dy, greaterThan(row.top));
    expect(arrow.center.dy, lessThanOrEqualTo(row.center.dy));
    await tester.tapAt(const Offset(1100, 20));
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
  });

  testWidgets('without a popover\'s width to its right: under it, as '
      'before', (tester) async {
    asIPad();
    // Two thirds of a 11" in Split View, a wide row.
    final ask = await _window(tester, const Size(694, 834), rowWidth: 400);
    final chosen = ask();
    await tester.pumpAndSettle();
    final row = tester.getRect(find.byKey(const Key('row')));
    final panel = _panel(tester);
    expect(panel.top, greaterThan(row.bottom), reason: 'under it');
    expect(_arrow(tester).width, greaterThan(_arrow(tester).height));
    await tester.tapAt(const Offset(600, 20));
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
  });

  testWidgets('right to left: under it, not mirrored yet', (tester) async {
    asIPad();
    final ask = await _window(
      tester,
      const Size(1366, 1024),
      direction: TextDirection.rtl,
    );
    final chosen = ask();
    await tester.pumpAndSettle();
    final row = tester.getRect(find.byKey(const Key('row')));
    expect(_panel(tester).top, greaterThan(row.bottom));
    await tester.tapAt(const Offset(1300, 20));
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
  });

  testWidgets('beside left out: under it, as every other caller has it', (
    tester,
  ) async {
    asIPad();
    final ask = await _window(tester, const Size(1366, 1024), beside: false);
    final chosen = ask();
    await tester.pumpAndSettle();
    final row = tester.getRect(find.byKey(const Key('row')));
    expect(_panel(tester).top, greaterThan(row.bottom));
    await tester.tapAt(const Offset(1300, 20));
    await tester.pumpAndSettle();
    expect(await chosen, isNull);
  });
}
