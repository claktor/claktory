// Friend bug #24: a Mac's backend started by hand, without its background
// service (Workspace.backend_service BY_HAND), says so in Needs you and in
// Settings → General beside Move, with Restore (RestoreBackendService)
// where it can; "Restarting…" until the app reconnects.

import 'package:factory_app/backend_service.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

const _reason =
    "Claktory was started by hand, so it won't start again by itself if it "
    'stops or your Mac restarts, and it can\'t move to another computer.';
const _repair =
    "Claktory's background service isn't set up. Quit the app and open it "
    'again to repair it.';

Workspace _workspace(BackendService? service) {
  final snapshot = workspace();
  if (service != null) snapshot.backendService = service;
  return snapshot;
}

BackendService _byHand({bool canRestore = true, bool restoring = false}) =>
    BackendService(
      state: BackendServiceState.BACKEND_SERVICE_STATE_BY_HAND,
      reason: canRestore ? _reason : _repair,
      canRestore: canRestore,
      restoring: restoring,
    );

final _restore = find.byWidgetPredicate(
  (w) => w is Button && w.label == 'Restore',
);

Future<Harness> _needsYou(
  WidgetTester tester,
  BackendService service, {
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(_workspace(service));
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  await tester.tap(find.text(backendServiceTitle));
  await pumpFrames(tester);
  return harness;
}

void main() {
  test('only a backend started by hand needs you', () {
    Iterable<NeedsYouItem> items(BackendService? service) =>
        needsYouItems(_workspace(service))
            .where((item) => item.kind == NeedsYouKind.backendService);
    expect(items(null), isEmpty);
    expect(items(BackendService()), isEmpty);
    expect(
      items(
        BackendService(
          state: BackendServiceState.BACKEND_SERVICE_STATE_RUNNING,
        ),
      ),
      isEmpty,
    );
    expect(items(_byHand()).single.key, 'backendService');
    expect(items(_byHand(canRestore: false)), hasLength(1));
  });

  testWidgets('Needs you says why; Restore asks the Claktory, says '
      '"Restarting…" until the app is back, and the item goes', (tester) async {
    final harness = await _needsYou(tester, _byHand());
    expect(find.text(_reason), findsOneWidget);
    expect(
      find.text('Restore it so it starts again by itself.'),
      findsOneWidget,
    );
    // No conversation: the card is all there is.
    expect(find.text('Open conversation'), findsNothing);
    await tester.ensureVisible(_restore);
    await pumpFrames(tester);
    await tester.tap(_restore);
    await pumpFrames(tester);
    expect(harness.last.hasRestoreBackendService(), isTrue);
    expect(find.text(backendRestarting), findsOneWidget);
    expect(_restore, findsNothing);

    // The backend stops; the service starts it again.
    harness.backend.connected.value = false;
    await pumpFrames(tester);
    expect(find.text(backendRestarting), findsOneWidget);
    harness.backend.connected.value = true;
    await harness.apply(
      tester,
      _workspace(
        BackendService(
          state: BackendServiceState.BACKEND_SERVICE_STATE_RUNNING,
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.text(_reason), findsNothing);
    expect(find.text(backendServiceTitle), findsNothing);
  });

  testWidgets('restoring, as the Claktory says, shows "Restarting…" on any '
      'device', (tester) async {
    await _needsYou(tester, _byHand(restoring: true));
    expect(find.text(backendRestarting), findsWidgets);
    expect(_restore, findsNothing);
  });

  testWidgets("where it can't restore, the reason alone: no button", (
    tester,
  ) async {
    await _needsYou(tester, _byHand(canRestore: false));
    expect(find.text(_repair), findsOneWidget);
    expect(_restore, findsNothing);
  });

  testWidgets('a refusal says why, and Restore stays', (tester) async {
    final harness = await _needsYou(tester, _byHand());
    harness.backend.refuse = 'The service is missing.';
    await tester.ensureVisible(_restore);
    await pumpFrames(tester);
    await tester.tap(_restore);
    await pumpFrames(tester);
    expect(
      find.textContaining(
        "Couldn't restart Claktory in the background: The service is "
        'missing.',
      ),
      findsOneWidget,
    );
    expect(_restore, findsOneWidget);
  });

  testWidgets('Settings → General has it beside Move', (tester) async {
    final harness = Harness(_workspace(_byHand()));
    await harness.show(tester, size: const Size(1200, 1600));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text('Move to another computer…'));
    expect(find.text(_reason), findsOneWidget);
    expect(_restore, findsOneWidget);
    await tester.ensureVisible(_restore);
    await settle(tester);
    await tester.tap(_restore);
    await settle(tester);
    expect(harness.last.hasRestoreBackendService(), isTrue);
  });

  testWidgets('running as installed, Settings shows nothing of it', (
    tester,
  ) async {
    final harness = Harness(
      _workspace(
        BackendService(
          state: BackendServiceState.BACKEND_SERVICE_STATE_RUNNING,
        ),
      ),
    );
    await harness.show(tester, size: const Size(1200, 1600));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text('Move to another computer…'));
    expect(_restore, findsNothing);
  });

  testWidgets('a phone connected to the Mac sees it and restores it', (
    tester,
  ) async {
    final harness = await _needsYou(
      tester,
      _byHand(),
      size: const Size(390, 844),
    );
    expect(find.text(_reason), findsOneWidget);
    await tester.ensureVisible(_restore);
    await pumpFrames(tester);
    await tester.tap(_restore);
    await pumpFrames(tester);
    expect(harness.last.hasRestoreBackendService(), isTrue);
    expect(tester.takeException(), isNull);
  });
}
