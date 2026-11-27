// An iPad's permissions:
// - P7: the camera's first time on the pairing screen keeps its words and
//   Continue together in the window's middle at every width, Slide Over's
//   320 included, never at a phone's foot; refused, the same.
// - P7: while iPadOS keeps the camera from the app (Split View, Stage
//   Manager, another app using it), the pairing screen's Paste code
//   instead is the way on, a primary button, until the camera's back.
// - P3 (and every permission): a dialog in the window's middle at every
//   width, never a phone's sheet from the bottom.
import 'dart:async';

import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/ipad/pair_step.dart' show IPadPairStep;
import 'package:factory_app/mobile/permission.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scan_code.dart' show scanCode;
import 'package:factory_app/scanner.dart' show CameraAccess, CameraAvailability;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../pair_start.dart';
import 'as_ipad.dart';

const _sizes = [
  ('full screen', Size(1366, 1024)),
  ('portrait', Size(1024, 1366)),
  ('Split View', Size(504, 1024)),
  ('Slide Over', Size(320, 1024)),
];

/// The pairing screen of an iPad with no Claktory yet, past its first
/// screen, on the camera, which answers [access].
Future<Harness> _pairing(
  WidgetTester tester,
  Size size,
  CameraAccess access, {
  double text = 1,
}) async {
  asIPad();
  tester.platformDispatcher.textScaleFactorTestValue = text;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(Workspace(), localBackend: false);
  harness.camera
    ..state = access
    ..granting = false;
  // A wide window's beside the rail (Shell's step), a narrow one's the
  // phone's.
  await harness.show(
    tester,
    size: size,
    home: size.width >= 600 ? const IPadPairStep() : null,
  );
  await pumpFrames(tester);
  await pastPairStart(tester);
  await pumpFrames(tester);
  return harness;
}

/// Notifications' words, asked for the first time.
class _Notifications extends Permission {
  @override
  PermissionWords words(BuildContext context) => const PermissionWords(
    glyph: 'bell',
    title: 'Know when an agent needs you',
    body: 'Your iPad asks next.',
    offTitle: 'Notifications are off',
    offBody: "You'll see what needs you only when you open Claktory.",
  );

  @override
  Future<PermissionAccess> access() async => PermissionAccess.undetermined;

  @override
  Future<PermissionAccess> request() async => PermissionAccess.denied;

  @override
  Future<void> openSettings() async {}
}

void main() {
  for (final (name, size) in _sizes) {
    testWidgets('$name: the camera\'s first time, its words and Continue '
        'together in the middle', (tester) async {
      await _pairing(tester, size, CameraAccess.undetermined);
      expect(tester.takeException(), isNull);
      final title = tester.getRect(find.text('Scan the code'));
      final proceed = tester.getRect(find.text('Continue'));
      // Under the words, not pinned at the window's foot.
      expect(proceed.top, greaterThan(title.bottom));
      expect(proceed.top - title.bottom, lessThan(260), reason: '$size');
      final middle = (title.top + proceed.bottom) / 2;
      expect((middle - size.height / 2).abs(), lessThan(120), reason: '$size');
      expect(proceed.bottom, lessThan(size.height * 0.75));

      // Refused: the same place, Open Settings the way on.
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final off = tester.getRect(find.text('Open Settings'));
      expect(off.bottom, lessThan(size.height * 0.75), reason: '$size');
      expect(find.text('Paste code instead'), findsOneWidget);
    });
  }

  testWidgets('Slide Over at the largest text: the words scroll, Continue '
      'stays in sight', (tester) async {
    await _pairing(
      tester,
      const Size(320, 1024),
      CameraAccess.undetermined,
      text: 3.1,
    );
    expect(tester.takeException(), isNull);
    final proceed = tester.getRect(find.text('Continue'));
    expect(proceed.bottom, lessThanOrEqualTo(1024));
    expect(proceed.top, greaterThan(0));
  });

  for (final (name, size, text) in [
    ('full screen', _sizes.first.$2, 1.0),
    ('Split View', _sizes[2].$2, 1.0),
    ('Split View at large text', _sizes[2].$2, 2.35),
    ('Slide Over at large text', _sizes[3].$2, 2.35),
    ('Slide Over at the largest text', _sizes[3].$2, 3.1),
  ]) {
    testWidgets('$name: the camera kept from the app, a band with why, and '
        'Paste code instead the way on, in sight, until it\'s back (Lucy '
        '48961)', (tester) async {
      await _pairing(tester, size, CameraAccess.granted, text: text);
      Finder paste({required bool primary}) => find.byWidgetPredicate(
        (w) =>
            w is Button &&
            w.label == 'Paste code instead' &&
            w.primary == primary,
      );
      expect(find.text('Paste code instead'), findsOneWidget);
      expect(paste(primary: true), findsNothing);
      final square = tester.getSize(find.byKey(const ValueKey('camera')));
      final camera = tester.element(find.byKey(const ValueKey('camera')));

      const words =
          "The camera isn't available in this window. Make Claktory full "
          'screen, or paste the code instead.';
      const CameraAvailability(words).dispatch(camera);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(paste(primary: true), findsOneWidget);
      // The way on in the window, not scrolled past its foot.
      final way = tester.getRect(paste(primary: true));
      expect(way.bottom, lessThanOrEqualTo(size.height), reason: '$size');
      expect(way.top, greaterThan(0));
      // The picture's a band, as tall as why needs, at least 120.
      final band = tester.getSize(find.byKey(const ValueKey('camera')));
      expect(band.height, greaterThanOrEqualTo(120 - 2));
      if (text == 1) expect(band.height, lessThan(square.height));
      expect(find.text(words), findsOneWidget);
      // Why, in sight with it, above it.
      final why = tester.getRect(
        find.byKey(const ValueKey('camera-kept-words')),
      );
      expect(why.top, greaterThanOrEqualTo(0));
      expect(why.top, lessThan(way.top));
      // The same picture, not a new one: the camera can say it's back.
      expect(tester.element(find.byKey(const ValueKey('camera'))), camera);

      const CameraAvailability(null).dispatch(camera);
      await pumpFrames(tester);
      expect(paste(primary: true), findsNothing);
      expect(find.text('Paste code instead'), findsOneWidget);
      expect(tester.element(find.byKey(const ValueKey('camera'))), camera);
      expect(tester.getSize(find.byKey(const ValueKey('camera'))), square);

      // Taken while it's kept: the code's field.
      const CameraAvailability('kept').dispatch(camera);
      await pumpFrames(tester);
      await tester.tap(paste(primary: true));
      await pumpFrames(tester);
      expect(find.text('Paste code'), findsOneWidget);
    });
  }

  for (final (name, size) in _sizes) {
    testWidgets('$name: a permission asked is a dialog in the middle, not '
        'a sheet', (tester) async {
      asIPad();
      final harness = Harness(
        Workspace(
          agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
          selectedAgentId: 'june',
        ),
      );
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      unawaited(
        askPermission(tester.element(find.byType(Text).last), _Notifications()),
      );
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(tester.takeException(), isNull);
      expect(find.byType(PhoneSheet), findsNothing);
      final title = tester.getRect(find.text('Know when an agent needs you'));
      final proceed = tester.getRect(find.text('Continue'));
      expect(
        ((title.top + proceed.bottom) / 2 - size.height / 2).abs(),
        lessThan(120),
        reason: '$size',
      );
      expect((title.center.dx - size.width / 2).abs(), lessThan(40));
    });
  }

  testWidgets('Slide Over at the largest text: the dialog fits, its words '
      'scroll and Continue stays in sight (Polnareff)', (tester) async {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
        selectedAgentId: 'june',
      ),
    );
    await harness.show(tester, size: const Size(320, 1024));
    await pumpFrames(tester);
    unawaited(
      askPermission(tester.element(find.byType(Text).last), _Notifications()),
    );
    await pumpFrames(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(tester.takeException(), isNull);
    expect(find.byType(PhoneSheet), findsNothing);
    final proceed = tester.getRect(find.text('Continue'));
    expect(proceed.left, greaterThanOrEqualTo(0));
    expect(proceed.right, lessThanOrEqualTo(320));
    expect(proceed.bottom, lessThanOrEqualTo(1024));
  });

  for (final (name, size, text) in [
    ('full screen', _sizes.first.$2, 1.0),
    ('Slide Over at large text', _sizes[3].$2, 2.35),
  ]) {
    testWidgets('$name: the scanner kept from the camera, the same band and '
        'Paste code instead in sight', (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = text;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(
        Workspace(
          agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
          selectedAgentId: 'june',
        ),
      );
      harness.camera.state = CameraAccess.granted;
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      var pasted = 0;
      unawaited(
        scanCode(
          tester.element(find.byType(AgentView).first),
          title: 'Scan a code',
          check: (_) => null,
          onPasteInstead: () => pasted++,
        ),
      );
      await pumpFrames(tester);
      final camera = tester.element(find.byKey(const ValueKey('camera')));
      const CameraAvailability(
        'Another app is using the camera. Close it, or paste the code '
        'instead.',
      ).dispatch(camera);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final way = find.byWidgetPredicate(
        (w) => w is Button && w.label == 'Paste code instead' && w.primary,
      );
      expect(way, findsOneWidget);
      expect(tester.getRect(way).bottom, lessThanOrEqualTo(size.height));
      expect(tester.element(find.byKey(const ValueKey('camera'))), camera);
      expect(
        tester.getSize(find.byKey(const ValueKey('camera'))).height,
        lessThan(size.height / 2),
      );
      await tester.tap(way);
      await pumpFrames(tester);
      expect(pasted, 1);
    });
  }
}
