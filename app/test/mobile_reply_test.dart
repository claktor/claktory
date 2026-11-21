// Reply on a phone (#228): a message's hold sheet offers Reply first where
// there's a message box to reply in, and VoiceOver finds it among the
// message's actions.
import 'package:cupertino_ui/cupertino_ui.dart'
    show DefaultCupertinoLocalizations;
import 'package:factory_app/mobile/message_actions.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart' show pumpFrames;

Future<void> _show(WidgetTester tester, {VoidCallback? onReply}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.view.physicalSize = const Size(393, 852);
  tester.view.devicePixelRatio = 1;
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      localizationsDelegates: const [
        DefaultCupertinoLocalizations.delegate,
        DefaultWidgetsLocalizations.delegate,
      ],
      builder: (context, _) => Overlay(
        initialEntries: [
          OverlayEntry(
            builder: (context) => Center(
              child: MessageActions(
                text: 'The build is green.',
                onReply: onReply,
                child: const Text('The build is green.'),
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

void main() {
  testWidgets('touch and hold: Reply first, then Copy and Select text', (
    tester,
  ) async {
    var replied = 0;
    await _show(tester, onReply: () => replied++);
    await tester.longPress(find.text('The build is green.'));
    await pumpFrames(tester);
    final reply = tester.getRect(find.text('Reply'));
    final copy = tester.getRect(find.text('Copy'));
    expect(reply.top, lessThan(copy.top));
    expect(find.text('Select text'), findsOneWidget);
    await tester.tap(find.text('Reply'));
    await pumpFrames(tester);
    expect(replied, 1);
    expect(find.text('Copy'), findsNothing, reason: 'the sheet went');
  });

  testWidgets('VoiceOver finds Reply among its actions', (tester) async {
    final semantics = tester.ensureSemantics();
    var replied = 0;
    await _show(tester, onReply: () => replied++);
    final node = tester.getSemantics(
      find
          .ancestor(
            of: find.text('The build is green.'),
            matching: find.byType(Semantics),
          )
          .first,
    );
    final actions = [
      for (final id in node.getSemanticsData().customSemanticsActionIds ?? [])
        CustomSemanticsAction.getAction(id)!.label,
    ];
    expect(actions, ['Reply', 'Copy', 'Select text']);
    semantics.dispose();
  });

  testWidgets('no box to reply in: no Reply', (tester) async {
    await _show(tester);
    await tester.longPress(find.text('The build is green.'));
    await pumpFrames(tester);
    expect(find.text('Copy'), findsOneWidget);
    expect(find.text('Reply'), findsNothing);
  });
}
