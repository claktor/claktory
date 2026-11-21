// A phone's sheet takes focus as it rises, even from the message box: keys
// and what's read out are the sheet's then, not a field's behind it
// (autofocus wouldn't: the field already has focus), and the field has it
// again once the sheet goes. A field in the sheet that asks for focus
// still gets it. And on Android, the drawer's dense rows
// are at least its 48dp target, while the iPhone keeps its own sizes.
import 'package:factory_app/mobile/metrics.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late FocusNode field;
  late TextEditingController text;
  late BuildContext behind;

  Future<void> window(WidgetTester tester) async {
    tester.view
      ..physicalSize = const Size(393, 852)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    field = FocusNode();
    text = TextEditingController();
    addTearDown(field.dispose);
    addTearDown(text.dispose);
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(size: Size(393, 852)),
        child: Directionality(
          textDirection: TextDirection.ltr,
          child: Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) {
                  behind = context;
                  return Align(
                    alignment: Alignment.bottomCenter,
                    child: SizedBox(
                      height: 44,
                      child: EditableText(
                        controller: text,
                        focusNode: field,
                        autofocus: true,
                        style: const TextStyle(fontSize: 17),
                        cursorColor: const Color(0xFF000000),
                        backgroundCursorColor: const Color(0xFF000000),
                      ),
                    ),
                  );
                },
              ),
            ],
          ),
        ),
      ),
    );
    await tester.pump();
    expect(field.hasFocus, isTrue);
  }

  testWidgets('a sheet takes focus from the message box behind it, and '
      'gives it back as it goes', (tester) async {
    await window(tester);
    final closed = presentSheet<void>(
      behind,
      builder: (context, close) => const Text('Sheet'),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Sheet'), findsOneWidget);
    expect(field.hasFocus, isFalse);

    // Closed, what was typed into has focus again.
    await tester.tapAt(const Offset(200, 10));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    await closed;
    expect(find.text('Sheet'), findsNothing);
    expect(field.hasFocus, isTrue);
  });

  testWidgets("a field in the sheet that asks for focus gets it", (
    tester,
  ) async {
    await window(tester);
    final inner = FocusNode();
    final innerText = TextEditingController();
    addTearDown(inner.dispose);
    addTearDown(innerText.dispose);
    final closed = presentSheet<void>(
      behind,
      builder: (context, close) => SizedBox(
        height: 44,
        child: EditableText(
          controller: innerText,
          focusNode: inner,
          autofocus: true,
          style: const TextStyle(fontSize: 17),
          cursorColor: const Color(0xFF000000),
          backgroundCursorColor: const Color(0xFF000000),
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(inner.hasFocus, isTrue);
    expect(field.hasFocus, isFalse);
    await tester.tapAt(const Offset(200, 10));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    await closed;
  });

  testWidgets("Android's rows are its 48dp at least; the iPhone's keep "
      'their sizes', (tester) async {
    late BuildContext context;
    await tester.pumpWidget(
      Builder(
        builder: (c) {
          context = c;
          return const SizedBox();
        },
      ),
    );
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
    });
    expect(PhonePlatformPolicy.least(context, Touch.denseLine), 36);
    expect(PhonePlatformPolicy.least(context, Touch.denseRow), 44);

    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    expect(PhonePlatformPolicy.least(context, Touch.denseLine), 48);
    expect(PhonePlatformPolicy.least(context, Touch.denseRow), 48);
    expect(PhonePlatformPolicy.least(context, 52), 52);

    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    expect(PhonePlatformPolicy.least(context, 28), 28);
  });
}
