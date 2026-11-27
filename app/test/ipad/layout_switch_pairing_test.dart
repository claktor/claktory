// Pairing through the window crossing between the narrow and wide layouts
// (Sale's row 198; Lucy 49619: a resize or rotation never changes where you
// are), with the window built as main.dart builds it: a new Shell each time
// it crosses. The pairing screen keeps its step, and a dialog waiting for an
// answer (Connect to this Claktory?, a permission) stays and still answers.
import 'dart:async';

import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/identity.dart' show isPhoneLayout;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/links.dart'
    show appLinks, confirmPairing, pairingCheckTitle;
import 'package:factory_app/mobile/permission.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show DeviceDrafts, PhoneClaktories;
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../pair_start.dart';
import 'as_ipad.dart';

/// The window as main.dart's _buildWindow lays it out.
class _Window extends StatelessWidget {
  const _Window();

  @override
  Widget build(BuildContext context) => isPhoneLayout(context)
      ? const PhoneClaktories(shell: Shell(key: ValueKey('shown')))
      : const Shell(key: ValueKey('shown'), rail: SizedBox(width: 56));
}

Future<Harness> _show(
  WidgetTester tester,
  Size size, {
  bool paired = false,
}) async {
  asIPad();
  final harness = paired
      ? Harness(
          Workspace(
            agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
            selectedAgentId: 'june',
          ),
        )
      : Harness(Workspace(), localBackend: false);
  harness.camera
    ..state = CameraAccess.undetermined
    ..granting = false;
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(focus: focus, child: const _Window()),
  );
  await pumpFrames(tester);
  return harness;
}

/// The app as main.dart builds it, with no Claktory yet (pair_step_test's).
Future<Connections> _app(WidgetTester tester, Size size) async {
  asIPad();
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.view.display.size = size;
  tester.view.display.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  addTearDown(tester.view.display.reset);
  final connections = Connections(MemoryConnectionStorage());
  await tester.pumpWidget(
    FactoryApp(
      drafts: DeviceDrafts(MemoryDraftStorage()),
      connections: connections,
      open: (target) {
        final backend = RecordingBackend(localBackend: false)
          ..connectTo(target);
        backend.workspace.apply(
          Workspace(settings: Settings(userName: 'Robin')),
        );
        return backend;
      },
      localBackend: false,
    ),
  );
  await pumpFrames(tester);
  return connections;
}

Future<void> _resize(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.display.size = size;
  await pumpFrames(tester);
}

/// Notifications' words, asked for the first time.
class _Notifications extends Permission {
  var asked = 0;

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
  Future<PermissionAccess> request() async {
    asked++;
    return PermissionAccess.granted;
  }

  @override
  Future<void> openSettings() async {}
}

const _wide = Size(1366, 1024);
const _split = Size(504, 1024);
const _landscapeSplit = Size(809, 1024);

void main() {
  for (final (from, to) in [
    (_wide, _split),
    (_split, _wide),
    (_split, _landscapeSplit),
  ]) {
    final name = '${from.width.toInt()} to ${to.width.toInt()}';
    testWidgets('$name and back: the pairing screen keeps its step and the '
        'code being pasted', (tester) async {
      await _app(tester, from);
      await pastPairStart(tester, paste: true);
      await pumpFrames(tester);
      expect(find.text('Already set up?'), findsNothing);
      final screen = tester.state(find.byType(PairScreen));
      await tester.enterText(find.byType(EditableText).first, 'factory://x');
      await pumpFrames(tester);
      await _resize(tester, to);
      expect(find.text('Already set up?'), findsNothing);
      expect(find.text('factory://x'), findsOneWidget);
      expect(tester.state(find.byType(PairScreen)), same(screen));
      await _resize(tester, from);
      expect(find.text('factory://x'), findsOneWidget);
      expect(tester.state(find.byType(PairScreen)), same(screen));
    });

    testWidgets('$name: a pairing link\'s Connect, tapped after the switch, '
        'still connects (Sale\'s row 202)', (tester) async {
      final connections = await _app(tester, from);
      final code = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';
      appLinks.receive('claktory://add?code=${Uri.encodeComponent(code)}');
      await pumpFrames(tester);
      expect(find.text(pairingCheckTitle), findsOneWidget);
      await _resize(tester, to);
      await tester.tap(find.text('Connect').last);
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 1));
      await pumpFrames(tester);
      // On: Local Network's why, or connected already.
      expect(
        connections.saved.isNotEmpty ||
            find
                .text('Find your Claktory on this network')
                .evaluate()
                .isNotEmpty,
        isTrue,
      );
    });

    testWidgets('$name: Connect to this Claktory? stays, and Connect still '
        'connects', (tester) async {
      await _app(tester, from);
      final target = RemoteFactory(
        code: 'factory://code-aa',
        host: '192.168.1.20',
        port: 7420,
        token: 't',
        fingerprint: 'aa',
        name: 'mac-mini',
      );
      final answer = Completer<bool>();
      unawaited(
        confirmPairing(
          tester.element(find.byType(PairScreen)),
          target,
        ).then(answer.complete),
      );
      await pumpFrames(tester);
      expect(find.text(pairingCheckTitle), findsOneWidget);
      await _resize(tester, to);
      expect(find.text(pairingCheckTitle), findsOneWidget);
      await tester.tap(find.text('Connect').last);
      await pumpFrames(tester);
      expect(answer.isCompleted, isTrue);
      expect(await answer.future, isTrue);
      expect(find.text(pairingCheckTitle), findsNothing);
    });
  }

  for (final (from, to) in [(_wide, _split), (_split, _wide)]) {
    final name = '${from.width.toInt()} to ${to.width.toInt()}';
    testWidgets('$name: a permission being asked stays, and Continue still '
        'asks', (tester) async {
      await _show(tester, from, paired: true);
      final permission = _Notifications();
      final answer = Completer<PermissionAccess>();
      unawaited(
        askPermission(
          tester.element(find.byType(Shell)),
          permission,
        ).then(answer.complete),
      );
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('Know when an agent needs you'), findsOneWidget);
      await _resize(tester, to);
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('Know when an agent needs you'), findsOneWidget);
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(permission.asked, 1);
      expect(await answer.future, PermissionAccess.granted);
      expect(find.text('Know when an agent needs you'), findsNothing);
    });
  }
}
