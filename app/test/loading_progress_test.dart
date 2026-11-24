// #375: a big file on its way says how much has come, with a bar, and
// while the connection is away that it carries on from there.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/loading_progress.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Future<ValueNotifier<bool>> _show(
  WidgetTester tester, {
  required int received,
  required int size,
  Size screen = const Size(393, 852),
  String? note,
}) async {
  final connected = ValueNotifier(true);
  final harness = Harness(Workspace());
  await harness.show(
    tester,
    size: screen,
    home: LoadingProgress(
      label: 'Loading video…',
      received: received,
      size: size,
      connected: connected,
      note: note,
    ),
  );
  return connected;
}

void main() {
  testWidgets("a video that can't come smaller says so under it (#379)", (
    tester,
  ) async {
    await _show(
      tester,
      received: 100,
      size: 1000,
      note: "Full size: this Claktory can't make a smaller copy.",
    );
    expect(
      find.text("Full size: this Claktory can't make a smaller copy."),
      findsOneWidget,
    );
  });

  testWidgets('before its size is known it says what is loading; then how '
      'much of how much, with the bar that far', (tester) async {
    await _show(tester, received: 0, size: 0);
    expect(find.text('Loading video…'), findsOneWidget);
    // Afresh (the window's overlay keeps what it was first given).
    await tester.pumpWidget(const SizedBox());
    await _show(tester, received: 2100000, size: 9400000);
    expect(find.text('Loading video… 2.1 MB of 9.4 MB'), findsOneWidget);
    final bar = tester.widget<FractionallySizedBox>(
      find.byType(FractionallySizedBox),
    );
    expect(bar.widthFactor, closeTo(2100000 / 9400000, 1e-9));
  });

  testWidgets('while the connection is away it says it carries on once '
      "it's back", (tester) async {
    final connected = await _show(tester, received: 100, size: 1000);
    const away =
        "Waiting for the connection. It carries on from here when it's back.";
    expect(find.text(away), findsNothing);
    connected.value = false;
    await tester.pump();
    expect(find.text(away), findsOneWidget);
    connected.value = true;
    await tester.pump();
    expect(find.text(away), findsNothing);
  });

  testWidgets('at the largest text on a small iPhone it fits', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.12;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final connected = await _show(
      tester,
      received: 2202010,
      size: 9856614,
      screen: const Size(320, 568),
    );
    connected.value = false;
    await tester.pump();
    expect(tester.takeException(), isNull);
  });
}
