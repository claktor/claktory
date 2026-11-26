// #529, S4 (Squalo's MC11; Fugo's conditions 5): a Claktory signed in to
// another Claktory account turns this device away (ERROR_CODE_OTHER_ACCOUNT)
// and says why: on a Mac and a phone, in place of the Claktory, its words,
// Try again and Remove…, never a way around it.
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/drafts.dart' show MemoryDraftStorage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart'
    show Claktories, ClaktoriesScope, DeviceDrafts;
import 'package:factory_app/your_claktories.dart' show OtherAccountView;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, signedIn;
import 'harness.dart';

Future<Harness> _show(
  WidgetTester tester,
  TargetPlatform platform,
  Size size,
) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(
    Workspace(),
    localBackend: false,
    account: signedIn(FakeAccountApi()),
  );
  harness.connections.use(
    parsePairingCode('factory://?t=${'b' * 64}&name=fred&e=${'d' * 64}'),
  );
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

/// What a Claktory on another account says when it turns this device away.
void _turnAway(Harness harness, String said) {
  harness.backend.connected.value = false;
  harness.backend.otherAccount.value = said;
  harness.backend.refused.value = said;
}

void main() {
  for (final (platform, size) in [
    (TargetPlatform.macOS, const Size(1200, 800)),
    (TargetPlatform.iOS, const Size(390, 844)),
  ]) {
    testWidgets('${platform.name}: refused at connect, it says to open a '
        'pipe, with Try again and no way around it', (tester) async {
      final harness = await _show(tester, platform, size);
      final said =
          "So it can't let this $thisDeviceKind in. To work with someone "
          "else's agents, open a pipe with them.";
      _turnAway(harness, said);
      await pumpFrames(tester);
      expect(find.byType(OtherAccountView), findsOneWidget);
      expect(find.text('fred is on another Claktory account'), findsOneWidget);
      expect(find.text(said), findsOneWidget);
      // Not the generic turned-away words, nor a sign-in.
      expect(find.textContaining('To use it again'), findsNothing);
      expect(find.text('Sign out'), findsNothing);
      expect(find.byKey(const ValueKey('device-email')), findsNothing);
      // Which account this device is on, and, in case it's the person's
      // own, how to move it to that one, in the app (V3; Fugo: no
      // commands, only the person may run them).
      expect(
        find.text('This $thisDeviceKind is signed in as m…@example.com.'),
        findsOneWidget,
      );
      expect(
        find.text(
          "If fred is yours, sign it in to this $thisDeviceKind's account "
          'instead, in Settings → Account on fred. Then try again here.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('No screen on fred?'), findsOneWidget);
      expect(find.textContaining('factoryd account'), findsNothing);
      expect(find.text('Sign in…'), findsOneWidget);
      await tester.ensureVisible(find.text('Try again'));
      await pumpFrames(tester);
      await tester.tap(find.text('Try again'));
      await pumpFrames(tester);
      expect(harness.backend.otherAccount.value, isNull);
      expect(find.byType(OtherAccountView), findsNothing);
    });

    testWidgets('${platform.name}: a device it let in before, now signed in '
        'to another account, is told to sign in with the one it uses', (
      tester,
    ) async {
      final harness = await _show(tester, platform, size);
      final said =
          'This $thisDeviceKind is now signed in to a different Claktory '
          'account. Sign in with the account fred uses to connect again.';
      _turnAway(harness, said);
      await pumpFrames(tester);
      expect(
        find.text('fred no longer lets this $thisDeviceKind in'),
        findsOneWidget,
      );
      expect(find.text(said), findsOneWidget);
      expect(
        find.text('This $thisDeviceKind is signed in as m…@example.com.'),
        findsOneWidget,
      );
      expect(find.textContaining('If fred is yours'), findsNothing);
      // Sign in…: out of this account first (the sign-out sheet, MC5, or
      // at once with nothing to lose), then in with fred's (V2).
      await tester.tap(find.text('Sign in…'));
      await pumpFrames(tester);
      expect(harness.connections.account.signedIn, isFalse);
      expect(find.text('Sign in with the account fred uses'), findsOneWidget);
    });

    testWidgets('${platform.name}: Remove… asks the rail\'s own question, '
        'Try again nearest the thumb on a phone', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(Workspace(), localBackend: false);
      final claktories = Claktories(
        connections: harness.connections,
        localBackend: false,
        open: (_) => harness.backend,
        drafts: DeviceDrafts(MemoryDraftStorage()),
      );
      addTearDown(claktories.dispose);
      harness.connections.use(
        parsePairingCode('factory://?t=${'b' * 64}&name=fred&e=${'d' * 64}'),
      );
      _turnAway(harness, 'Open a pipe with them.');
      await harness.show(
        tester,
        size: size,
        home: ClaktoriesScope(
          claktories: claktories,
          child: const OtherAccountView(),
        ),
      );
      await pumpFrames(tester);
      final remove = find.text('Remove…');
      expect(remove, findsOneWidget);
      if (platform == TargetPlatform.iOS) {
        expect(
          tester.getRect(find.text('Try again')).top,
          greaterThan(tester.getRect(remove).bottom),
        );
      }
      await tester.tap(remove);
      await pumpFrames(tester);
      expect(find.text('Remove fred?'), findsOneWidget);
      // Not connected, so both ways back, and that neither works while
      // it's on another account (V1).
      expect(
        find.textContaining(
          'or run factoryd pair in a terminal there. While fred is on another '
          "Claktory account, it won't let this $thisDeviceKind in.",
        ),
        findsOneWidget,
      );
    });
  }
}
