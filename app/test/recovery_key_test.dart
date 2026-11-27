// Saving the recovery key (R1): Needs you asks while the backend says the
// key isn't confirmed (VaultState.recovery_key_unconfirmed); the sheet
// shows the key only when asked, then has one group of it typed back.
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recovery_key.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool unconfirmed = true}) => Workspace(
  // Named: each Claktory has its own key, so Needs you says whose.
  settings: Settings(name: 'nixos'),
  vault: VaultState(
    exists: true,
    unlocked: true,
    recoveryKeyUnconfirmed: unconfirmed,
  ),
);

Future<Harness> _open(
  WidgetTester tester, {
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(_workspace());
  harness.backend.recoveryKey = 'ABCD-EFGH-JKLM-NPQR-STUV-WXYZ-2345-6789';
  await harness.show(tester, size: size);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await settle(tester);
  await settle(tester);
  return harness;
}

void main() {
  test('Needs you has it exactly while the key is unconfirmed', () {
    expect(needsYouItems(_workspace()).map((item) => item.kind), [
      NeedsYouKind.recoveryKey,
    ]);
    expect(needsYouCount(_workspace(unconfirmed: false)), 0);
  });

  testWidgets('saving it: the key shows only when asked, then one group of '
      'it is typed back, and it goes from every device', (tester) async {
    final harness = await _open(tester);
    final backend = harness.backend;
    expect(find.text('Save the recovery key for nixos'), findsOneWidget);
    await tester.tap(find.text('Save the recovery key for nixos'));
    await settle(tester);
    expect(find.byType(SaveRecoveryKeyForm), findsOneWidget);
    // Not asked for until the person does.
    expect(backend.sent.where((m) => m.hasShowRecoveryKey()), isEmpty);
    expect(find.text(backend.recoveryKey), findsNothing);

    await tester.tap(find.text('Show recovery key'));
    await settle(tester);
    expect(harness.last.hasShowRecoveryKey(), isTrue);
    expect(find.text(backend.recoveryKey), findsOneWidget);
    await tester.tap(find.text('Copy'));
    await settle(tester);
    expect(find.text('Copied'), findsOneWidget);

    await tester.tap(find.text("I've saved it"));
    await settle(tester);
    expect(harness.last.hasCheckRecoveryKey(), isTrue);
    // Checking what they saved, not reading it off the screen.
    expect(find.text(backend.recoveryKey), findsNothing);
    expect(
      find.textContaining('Type block 3 from the key you saved'),
      findsOneWidget,
    );

    await tester.enterText(find.byType(EditableText), 'jklm');
    await tester.tap(find.text('Confirm'));
    await settle(tester);
    final confirm = harness.last.confirmRecoveryKey;
    expect(confirm.checkId, 'check-1');
    expect(confirm.typed, 'jklm');
    expect(find.byType(SaveRecoveryKeyForm), findsNothing);

    // The backend says so everywhere: the item goes.
    await harness.apply(tester, _workspace(unconfirmed: false));
    expect(find.text('Save the recovery key for nixos'), findsNothing);
  });

  testWidgets('a wrong group says why, as the backend does, and can be '
      'typed again; Back starts over', (tester) async {
    final harness = await _open(tester);
    final backend = harness.backend;
    await tester.tap(find.text('Save the recovery key for nixos'));
    await settle(tester);
    await tester.tap(find.text("I've saved it"));
    await settle(tester);
    backend.refuse =
        "That isn't block 3 of your recovery key. Check what you saved and "
        'try again.';
    await tester.enterText(find.byType(EditableText), 'ABCD');
    await tester.tap(find.text('Confirm'));
    await settle(tester);
    expect(find.text(backend.refuse!), findsOneWidget);
    expect(find.byType(SaveRecoveryKeyForm), findsOneWidget);

    backend.refuse = null;
    await tester.tap(find.text('Back'));
    await settle(tester);
    expect(find.text('Show recovery key'), findsOneWidget);
    expect(find.text(backend.recoveryKey), findsNothing);
  });

  testWidgets('on an iPhone at the largest text, both steps fit', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    try {
      await _open(tester, size: const Size(375, 667));
      // Needs you on an iPhone lists it first: its row saves it.
      await tester.tap(find.text('Save the recovery key for nixos'));
      await settle(tester);
      await tester.ensureVisible(find.text('Show recovery key'));
      await settle(tester);
      await tester.tap(find.text('Show recovery key'));
      await settle(tester);
      expect(tester.takeException(), isNull);
      // All of it in sight, wrapped at its blocks, to copy down by hand.
      final key = tester.getRect(
        find.text('ABCD-EFGH-JKLM-NPQR-STUV-WXYZ-2345-6789'),
      );
      expect(key.right, lessThanOrEqualTo(375));
      expect(key.height, greaterThan(40), reason: 'on more than one line');
      await tester.ensureVisible(find.text("I've saved it"));
      await tester.tap(find.text("I've saved it"));
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Confirm'), findsOneWidget);
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  test("the key's file: named for its Claktory, holding whose it is, the "
      'day and the key', () {
    expect(
      recoveryKeyFileName('\u2068nix/os: lab\u2069'),
      'Claktory recovery key (nixos lab).txt',
    );
    expect(recoveryKeyFileName('\u202E'), 'Claktory recovery key.txt');
    expect(
      recoveryKeyFileText(
        '\u2068nixos\u2069',
        'ABCD-EFGH',
        DateTime(2026, 11, 27),
      ),
      'Recovery key for the Claktory nixos, saved 27 November 2026.\n\n'
      'ABCD-EFGH\n',
    );
  });

  group('Download', () {
    const device = MethodChannel('factory/device');
    late List<MethodCall> calls;
    late Future<Object?> Function(MethodCall call) answer;

    setUp(() {
      calls = [];
      answer = (_) async => '/Users/me/Downloads/key.txt';
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(device, (call) async {
            if (call.method != 'saveText' && call.method != 'shareFiles') {
              return null;
            }
            calls.add(call);
            return answer(call);
          });
    });
    tearDown(() {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(device, null);
      debugDefaultTargetPlatformOverride = null;
    });

    Future<Harness> shown(WidgetTester tester) async {
      final harness = await _open(tester);
      await tester.tap(find.text('Save the recovery key for nixos'));
      await settle(tester);
      await tester.tap(find.text('Show recovery key'));
      await settle(tester);
      return harness;
    }

    testWidgets("on a Mac it saves where the person chooses, and they still "
        'type a block back', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      try {
        final harness = await shown(tester);
        answer = (_) async => null;
        await tester.tap(find.text('Download'));
        await settle(tester);
        expect(find.text('Download'), findsOneWidget, reason: 'cancelled');

        answer = (_) async => '/Users/me/Downloads/key.txt';
        await tester.tap(find.text('Download'));
        await settle(tester);
        expect(find.text('Downloaded'), findsOneWidget);
        final args = calls.last.arguments as Map;
        expect(calls.last.method, 'saveText');
        expect(args['name'], 'Claktory recovery key (nixos).txt');
        expect(args['text'], contains(harness.backend.recoveryKey));
        expect(
          args['text'],
          startsWith('Recovery key for the Claktory nixos, '),
        );
        // Saved is no confirmation: that's still the block typed back.
        expect(
          harness.backend.sent.where((m) => m.hasConfirmRecoveryKey()),
          isEmpty,
        );
        expect(find.byType(SaveRecoveryKeyForm), findsOneWidget);

        answer = (_) async => throw PlatformException(
          code: 'failed',
          message: '/Volumes/USB can\'t be written to',
        );
        await tester.tap(find.text('Downloaded'));
        await settle(tester);
        expect(
          find.text("Couldn't save the key: /Volumes/USB can't be written to"),
          findsOneWidget,
        );
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });

    testWidgets('on an iPhone it goes to Files, never the share sheet', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      try {
        final harness = await shown(tester);
        answer = (_) async => true;
        await tester.ensureVisible(find.text('Download'));
        await tester.tap(find.text('Download'));
        await settle(tester);
        expect(calls.map((call) => call.method), ['saveText']);
        final args = calls.single.arguments as Map;
        expect(args['name'], 'Claktory recovery key (nixos).txt');
        expect(args['text'], contains(harness.backend.recoveryKey));
        expect(find.text('Downloaded'), findsOneWidget);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });

    testWidgets('on Windows the key never shows, so nor does Download', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.windows;
      try {
        await _open(tester);
        await tester.tap(find.text('Save the recovery key for nixos'));
        await settle(tester);
        expect(find.text('Show recovery key'), findsNothing);
        expect(find.text('Download'), findsNothing);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });
  });
}
