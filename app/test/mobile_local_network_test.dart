// Local Network on an iPhone or iPad (mobile/local_network.dart): before
// the first connection, why Claktory asks, then iOS's prompt; declined,
// what that means and how to turn it on, going on by itself once it is.
// Never a dead end, and never asked again once a Claktory is there.
import 'dart:async';

import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/mobile/local_network.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'pair_start.dart';

const _phone = Size(390, 844);

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay.example.com';

/// iOS's permission as the test answers it: each check waits for [answer].
class _FakeLocalNetwork implements LocalNetwork {
  var checks = 0;
  var settingsOpened = 0;
  Completer<LocalNetworkAccess>? _pending;

  bool get asking => _pending != null;

  void answer(LocalNetworkAccess access) {
    final pending = _pending!;
    _pending = null;
    pending.complete(access);
  }

  @override
  Future<LocalNetworkAccess> check() {
    checks++;
    return (_pending = Completer()).future;
  }

  @override
  Future<void> openSettings() async => settingsOpened++;
}

const _explain = 'Find your Claktory on this network';
const _off = 'Local Network is off';

void main() {
  late _FakeLocalNetwork network;

  setUp(() {
    network = _FakeLocalNetwork();
    localNetwork = network;
  });
  tearDown(() => localNetwork = null);

  /// An iPhone's, for the test (reset before its invariants are checked).
  void onIphone() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }

  /// A phone with no Claktory, its code pasted and Connect said twice:
  /// up to where the sheet comes.
  Future<Harness> pairing(WidgetTester tester) async {
    onIphone();
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: _phone);
    await pumpFrames(tester);
    await pastPairStart(tester, paste: true);
    await tester.enterText(find.byType(EditableText), _code);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    return harness;
  }

  Finder button(String label) => find.widgetWithText(Button, label);

  testWidgets('the first connection says why first; Continue asks iOS, and '
      'allowed, it connects', (tester) async {
    final harness = await pairing(tester);
    expect(find.text(_explain), findsOneWidget);
    expect(
      find.textContaining('Your iPhone asks for permission next'),
      findsOneWidget,
    );
    expect(network.checks, 0, reason: 'iOS asks only after Continue');
    expect(harness.connections.current, isNull, reason: 'not yet');

    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    expect(network.asking, isTrue);
    expect(
      tester.widget<Button>(button('Continue')).onPressed,
      isNull,
      reason: 'once, while iOS asks',
    );
    network.answer(LocalNetworkAccess.granted);
    await pumpFrames(tester);
    expect(find.text(_explain), findsNothing);
    expect(harness.connections.current?.endpointId, 'c' * 64);
    expect(find.byType(PairScreen), findsNothing);
  });

  testWidgets('declined: what it means and where to turn it on; turned on '
      'in Settings, it goes on by itself', (tester) async {
    final harness = await pairing(tester);
    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(find.text(_off), findsOneWidget);
    expect(
      find.textContaining("still connects through Claktory's servers"),
      findsOne,
    );
    expect(find.textContaining('→ Local Network'), findsOneWidget);
    expect(harness.connections.current, isNull);

    await tester.tap(button('Open Settings'));
    await pumpFrames(tester);
    expect(network.settingsOpened, 1);

    // Back from Settings with it still off: it stays, saying so.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(find.text(_off), findsOneWidget);

    // Turned on: it goes on by itself.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.granted);
    await pumpFrames(tester);
    expect(find.text(_off), findsNothing);
    expect(harness.connections.current?.endpointId, 'c' * 64);
  });

  testWidgets("why it asks can't be set aside: Continue is the way on; "
      'declined, setting it aside is Not now', (tester) async {
    final harness = await pairing(tester);
    // A tap on the dimmed screen above the sheet.
    await tester.tapAt(const Offset(195, 60));
    await pumpFrames(tester);
    expect(find.text(_explain), findsOneWidget);
    // Nor read out as closing, nor dragged away: no Close, no grabber.
    expect(find.bySemanticsLabel('Close'), findsNothing);
    await tester.drag(find.text(_explain), const Offset(0, 400));
    await pumpFrames(tester);
    expect(find.text(_explain), findsOneWidget);

    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(find.text(_off), findsOneWidget);
    expect(find.bySemanticsLabel('Close'), findsOneWidget);
    await tester.tapAt(const Offset(195, 60));
    await pumpFrames(tester);
    expect(find.text(_off), findsNothing);
    expect(harness.connections.current?.endpointId, 'c' * 64);
  });

  testWidgets("a keyboard's Return presses the way on; Not now is a "
      "finger's size", (tester) async {
    await pairing(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(network.asking, isTrue, reason: 'Return is Continue');
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(
      tester.getSize(find.bySemanticsLabel('Not now')).height,
      greaterThanOrEqualTo(44),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(network.settingsOpened, 1, reason: 'Return is Open Settings');
  });

  testWidgets("on an iPad, a dialog: Esc does nothing while it explains, "
      "and is Not now once it's off", (tester) async {
    asIPad();
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: const Size(1032, 1376));
    await pumpFrames(tester);
    final context = tester.element(find.byType(Text).last);
    var closed = false;
    unawaited(showLocalNetwork(context, network).then((_) => closed = true));
    await pumpFrames(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.text(_explain), findsOneWidget);
    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(find.text(_off), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.text(_off), findsNothing);
    expect(closed, isTrue);
  });

  testWidgets('declined, Not now connects all the same', (tester) async {
    final harness = await pairing(tester);
    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester);
    expect(find.text(_off), findsNothing);
    expect(harness.connections.current?.endpointId, 'c' * 64);
  });

  testWidgets("when it can't tell, it connects", (tester) async {
    final harness = await pairing(tester);
    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.unknown);
    await pumpFrames(tester);
    expect(find.text(_explain), findsNothing);
    expect(harness.connections.current?.endpointId, 'c' * 64);
  });

  testWidgets('with a Claktory already, never asked', (tester) async {
    onIphone();
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: _phone);
    await pumpFrames(tester);
    final context = tester.element(find.byType(PairScreen));
    harness.connections.use(parsePairingCode(_code));
    var done = false;
    unawaited(localNetworkBeforeFirst(context).then((_) => done = true));
    await tester.pump();
    expect(done, isTrue);
    await pumpFrames(tester);
    expect(find.text(_explain), findsNothing);
    expect(network.checks, 0);
  });

  testWidgets('at the largest text on a small iPhone, nothing is cut off', (
    tester,
  ) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    onIphone();
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    final context = tester.element(find.byType(PairScreen));
    unawaited(showLocalNetwork(context, network));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    await tester.tap(button('Continue'));
    await pumpFrames(tester);
    network.answer(LocalNetworkAccess.denied);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    // The buttons stay on screen; the words scroll.
    final screen = tester.view.physicalSize / tester.view.devicePixelRatio;
    expect(
      tester.getRect(find.text('Not now')).bottom,
      lessThan(screen.height),
    );
    expect(
      tester.getRect(button('Open Settings')).bottom,
      lessThan(screen.height),
    );
  });
}
