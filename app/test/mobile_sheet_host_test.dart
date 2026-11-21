// PhoneSheetHost: a phone sheet driven by state (an extension's panel, open
// on the backend) rather than presented.
import 'package:factory_app/mobile/presentation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<void> host(
    WidgetTester tester, {
    required Widget? child,
    required VoidCallback onDismiss,
  }) {
    tester.view
      ..physicalSize = const Size(393, 852)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    return tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(size: Size(393, 852)),
        child: Directionality(
          textDirection: TextDirection.ltr,
          child: Stack(
            children: [
              const Positioned.fill(child: Text('Behind')),
              Positioned.fill(
                child: PhoneSheetHost(onDismiss: onDismiss, child: child),
              ),
            ],
          ),
        ),
      ),
    );
  }

  testWidgets('it rises while there is a child, goes down when there is '
      'none, and counts as a sheet meanwhile', (tester) async {
    var dismissed = 0;
    await host(tester, child: null, onDismiss: () => dismissed++);
    expect(find.byType(PhoneSheet), findsNothing);
    expect(sheetsPresented.value, 0);

    await host(tester, child: const Text('Panel'), onDismiss: () {});
    await tester.pumpAndSettle();
    expect(find.text('Panel'), findsOneWidget);
    expect(sheetsPresented.value, 1);

    // The dimmed screen asks it to close; it stays until the child goes.
    await host(
      tester,
      child: const Text('Panel'),
      onDismiss: () => dismissed++,
    );
    await tester.tapAt(const Offset(200, 3));
    await tester.pump();
    expect(dismissed, 1);
    expect(find.text('Panel'), findsOneWidget);

    await host(tester, child: null, onDismiss: () {});
    await tester.pump();
    expect(find.text('Panel'), findsOneWidget, reason: 'going down');
    await tester.pumpAndSettle();
    expect(find.byType(PhoneSheet), findsNothing);
    expect(sheetsPresented.value, 0);
  });

  testWidgets('one opened as the last goes down rises again', (tester) async {
    await host(tester, child: const Text('First'), onDismiss: () {});
    await tester.pumpAndSettle();
    await host(tester, child: null, onDismiss: () {});
    await tester.pump(const Duration(milliseconds: 50));
    await host(tester, child: const Text('Second'), onDismiss: () {});
    await tester.pumpAndSettle();
    expect(find.text('Second'), findsOneWidget);
    final sheet = tester.getRect(find.byType(PhoneSheet));
    expect(sheet.bottom, lessThanOrEqualTo(852), reason: 'all the way up');
    expect(sheet.top, lessThan(852 / 2));
    expect(sheetsPresented.value, 1);
    await host(tester, child: null, onDismiss: () {});
    await tester.pumpAndSettle();
    expect(sheetsPresented.value, 0);
  });

  testWidgets('a presented sheet torn down with its app is counted out, '
      'so the next one starts from none', (tester) async {
    tester.view
      ..physicalSize = const Size(393, 852)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(size: Size(393, 852)),
        child: Directionality(
          textDirection: TextDirection.ltr,
          child: Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) => GestureDetector(
                  onTap: () => presentSheet<void>(
                    context,
                    builder: (context, close) => const Text('Sheet'),
                  ),
                  child: const Text('Open'),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('Sheet'), findsOneWidget);
    expect(sheetsPresented.value, 1);

    // Gone with what presented it, never closed.
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
    expect(sheetsPresented.value, 0);
  });
}
