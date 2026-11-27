// Zucchero's row 507: the #37 texts at the largest text size on an iPhone
// SE. The drawer's "Can't connect · last seen 1 min ago" lost its time to
// an ellipsis, the connection sheet's "Can't connect to mac-mini" lost the
// name, and "Reconnecting…" broke inside the word.
import 'package:factory_app/connection.dart' show ConnectionDialog;
import 'package:factory_app/connections.dart';
import 'package:factory_app/forms.dart' show FittedHeading;
import 'package:factory_app/mobile/drawer.dart'
    show PhoneDrawerHost, PhoneDrawerHostState;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show PhoneConnectionWord;
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show workspace;
import 'rivet_test.dart' show snapshot;
import 'tunnel_test.dart' show tunnelStatus;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

/// [text] is drawn whole: no line dropped, no ellipsis.
void expectWhole(WidgetTester tester, Finder text) {
  final paragraph = tester.renderObject<RenderParagraph>(
    find.descendant(of: text, matching: find.byType(RichText)),
  );
  expect(paragraph.didExceedMaxLines, isFalse, reason: 'cut short');
}

void main() {
  const ax5 = 53 / 17;

  testWidgets('a word too wide for the line is drawn smaller, never broken; '
      'words that fit keep their size', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    const style = TextStyle(fontSize: 22, height: 1.2);
    Future<Size> drawn(String text) async {
      await tester.pumpWidget(
        MediaQuery(
          data: MediaQueryData.fromView(tester.view),
          child: Directionality(
            textDirection: TextDirection.ltr,
            child: Align(
              alignment: Alignment.topLeft,
              child: SizedBox(
                width: 288,
                child: FittedHeading(text, style: style),
              ),
            ),
          ),
        ),
      );
      return tester.getSize(find.text(text));
    }

    // One line, at most the line's width.
    final word = await drawn('Reconnecting…');
    expect(word.width, lessThanOrEqualTo(288));
    expect(word.height, lessThan(22 * ax5 * 1.2 * 1.5));
    // Short words wrap at the full size.
    final words = await drawn('Can I go?');
    expect(words.height, greaterThanOrEqualTo(22 * ax5 * 1.2 - 1));
  });

  for (final reconnecting in [false, true]) {
    testWidgets('on the smallest iPhone at the largest text, the connection '
        "sheet's title and the drawer's word are whole "
        '(${reconnecting ? 'reconnecting' : "can't connect"})', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = ax5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(snapshot(selected: 'a1'), localBackend: false);
      harness.connections.use(parsePairingCode(_code)!);
      harness.backend.workspace.apply(snapshot(selected: 'a1'));
      harness.backend.transcript.load(Transcript(agentId: 'a1'));
      // The drawer's header, as wide as it is on a 320-point iPhone.
      await harness.show(
        tester,
        size: const Size(320, 568),
        home: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: 231,
            child: PhoneConnectionWord(harness.backend),
          ),
        ),
      );
      harness.backend
        ..connected.value = false
        ..reconnecting.value = reconnecting
        ..lastConnected.value = DateTime.now().subtract(
          const Duration(minutes: 1),
        );
      harness.backend.tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'Nothing answered within 8 seconds.',
      );
      await pumpFrames(tester);
      final word = find.byType(PhoneConnectionWord);
      final said = reconnecting
          ? 'Reconnecting…'
          : "Can't connect · last seen 1 min ago";
      final inWord = find.descendant(of: word, matching: find.text(said));
      expect(inWord, findsOneWidget);
      expectWhole(tester, inWord);

      await tester.tap(word);
      await pumpFrames(tester);
      expect(find.byType(ConnectionDialog), findsOneWidget);
      final title = find.descendant(
        of: find.byType(ConnectionDialog),
        matching: find.text(
          reconnecting ? 'Reconnecting…' : "Can't connect to mac-mini",
        ),
      );
      expect(title, findsOneWidget);
      expectWhole(tester, title);
      // Within the sheet's width: no word broken across lines.
      final sheet = tester.getRect(find.byType(ConnectionDialog));
      final at = tester.getRect(title);
      expect(at.left, greaterThanOrEqualTo(sheet.left - 0.5));
      expect(at.right, lessThanOrEqualTo(sheet.right + 0.5));
      expect(tester.takeException(), isNull);
    });
  }

  // Row 510: in the drawer, the top (the name and more) scrolls at the
  // largest text, and the line went under its fold.
  testWidgets("in the drawer at the largest text, how it's connected is "
      'whole and in sight', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(snapshot(selected: 'a1'), localBackend: false);
    harness.connections.use(parsePairingCode(_code)!);
    harness.backend.workspace.apply(snapshot(selected: 'a1'));
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    harness.backend
      ..connected.value = false
      ..reconnecting.value = false
      ..lastConnected.value = DateTime.now().subtract(
        const Duration(minutes: 1),
      );
    harness.backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'Nothing answered within 8 seconds.',
    );
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    final said = find.text("Can't connect · last seen 1 min ago");
    expect(said, findsOneWidget);
    expectWhole(tester, said);
    // Outside the top's scrolling, so nothing of it is under a fold: its
    // last line is where a tap lands.
    expect(
      find.ancestor(of: said, matching: find.byType(SingleChildScrollView)),
      findsNothing,
    );
    // The drawer slides in.
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    final at = tester.getRect(said);
    expect(at.bottom, lessThanOrEqualTo(568));
    expect(
      find.byType(PhoneConnectionWord).hitTestable(at: const Alignment(0, 0.9)),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  // Row 510: Settings' "Notifications" broke as "Notification / s".
  testWidgets("a phone's Settings rows never break a word", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(
      workspace()
        ..settings = Settings(
          userName: 'Matt',
          notifications: NotificationSettings(),
        ),
    );
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    final one = tester.getSize(find.text('Profile'));
    final row = find.text('Notifications');
    await tester.scrollUntilVisible(
      row,
      200,
      // The list's own, not its search field's.
      scrollable: find.byType(Scrollable).first,
    );
    await pumpFrames(tester);
    // One line, as tall as "Profile"'s.
    expect(tester.getSize(row).height, lessThanOrEqualTo(one.height + 1));
    expect(tester.takeException(), isNull);
  });
}
