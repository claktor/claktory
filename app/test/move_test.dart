import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/controls.dart' show LiveDot;
import 'package:factory_app/move.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _code =
    'claktory-move://?e=${'a' * 64}&s=${'b' * 64}&m=cloud-box&v=v0.1.15'
    '&o=linux&h=x86_64&r=https%3A%2F%2Frelay.example.com';

/// A move that does what it's asked, as the tests say, and records it.
class _FakeMove implements MoveSource {
  @override
  final ValueNotifier<MoveStatus> status = ValueNotifier(MoveStatus.none);
  final asked = <String>[];
  String? refuse;

  Future<void> _ask(String what) async {
    asked.add(what);
    if (refuse case final reason?) throw RequestFailure(reason);
  }

  @override
  Future<void> start(String code) => _ask('start $code');
  @override
  Future<void> confirm() => _ask('confirm');
  @override
  Future<void> cancel() => _ask('cancel');
  @override
  Future<void> freeze() => _ask('freeze');

  @override
  final ValueNotifier<bool> connected = ValueNotifier(true);
  @override
  String name = 'Home';

  @override
  final ValueNotifier<pb.MovedAway?> movedAway = ValueNotifier(null);
  @override
  bool canDeleteMovedCopy = true;
  @override
  Future<void> deleteMovedCopy(String typedWord) =>
      _ask('delete copy $typedWord');

  void set(MoveStatus move) => status.value = move;
}

MoveStatus _at(
  MovePhase phase, {
  int have = 0,
  int total = 0,
  bool canCancel = true,
  List<String> problems = const [],
  String error = '',
  String step = '',
  String startedBy = '',
}) => MoveStatus(
  phase: phase,
  targetMachine: 'cloud-box',
  targetOs: 'linux',
  targetArch: 'x86_64',
  digits: '123 456',
  planBytes: 4800000000,
  missingTools: const ['flutter'],
  map: const [(from: '/Users/me', to: '/home/you')],
  bytesHave: have,
  bytesTotal: total,
  round: 2,
  canCancel: canCancel,
  problems: problems,
  error: error,
  stepText: step,
  startedByAgentId: startedBy,
);

Future<_FakeMove> _open(
  WidgetTester tester, {
  CameraAccess camera = CameraAccess.unavailable,
}) async {
  final move = _FakeMove();
  final harness = Harness(pb.Workspace())..camera.state = camera;
  _harness = harness;
  await harness.show(
    tester,
    home: Center(child: MoveSettingRow(source: move)),
  );
  await tester.tap(find.text('Move to another computer…'));
  await tester.pump();
  return move;
}

late Harness _harness;

void main() {
  test('a receive code says what it is from; anything else is none', () {
    final code = parseReceiveCode('  $_code ')!;
    expect(code.code, _code);
    expect(code.machine, 'cloud-box');
    expect(code.system, 'Linux x86_64');
    expect(code.version, 'v0.1.15');
    expect(parseReceiveCode('claktory-move://?e=x&s=short'), isNull);
    expect(parseReceiveCode('factory://?t=${'a' * 64}&e=${'b' * 64}'), isNull);
    expect(parseReceiveCode('hello'), isNull);
  });

  testWidgets('pasting the code shows the computer, and Connect starts', (
    tester,
  ) async {
    final move = await _open(tester);
    expect(find.text('Connect'), findsOneWidget);
    await tester.enterText(find.byType(EditableText), 'nonsense');
    await tester.pump();
    expect(
      find.text("This isn't the code the other computer shows."),
      findsOneWidget,
    );
    await tester.tap(find.text('Connect'));
    expect(move.asked, isEmpty, reason: 'not a code');

    await tester.enterText(find.byType(EditableText), _code);
    await tester.pump();
    expect(
      find.text('cloud-box · Linux x86_64 · Claktory v0.1.15'),
      findsOneWidget,
    );
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(move.asked, ['start $_code']);
  });

  testWidgets('where there is a camera, Scan code reads the QR code the other '
      'computer shows into the field; Connect is still the person\'s (friend '
      'bug #2)', (tester) async {
    final move = await _open(tester, camera: CameraAccess.granted);
    await pumpFrames(tester);
    expect(
      find.textContaining('Paste that code here, or scan its QR code.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    expect(find.text("Scan the other computer's code"), findsOneWidget);
    _harness.camera.scan('hello');
    await pumpFrames(tester);
    expect(
      find.text("This isn't the code the other computer shows."),
      findsOneWidget,
    );
    _harness.camera.scan(_code);
    await pumpFrames(tester);
    expect(
      find.text('cloud-box · Linux x86_64 · Claktory v0.1.15'),
      findsOneWidget,
    );
    expect(move.asked, isEmpty);
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(move.asked, ['start $_code']);
  });

  testWidgets('without a camera, no Scan code', (tester) async {
    await _open(tester);
    await pumpFrames(tester);
    expect(find.text('Scan code'), findsNothing);
    expect(find.textContaining('Paste that code here.'), findsOneWidget);
  });

  testWidgets('a refused start says why, in the form', (tester) async {
    final move = await _open(tester);
    move.refuse = 'cloud-box runs an older Claktory (v0.1.9).';
    await tester.enterText(find.byType(EditableText), _code);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(
      find.text('cloud-box runs an older Claktory (v0.1.9).'),
      findsOneWidget,
    );
  });

  testWidgets('the form follows the move: digits, checking, copying', (
    tester,
  ) async {
    final move = await _open(tester);
    move.set(_at(MovePhase.connecting));
    await tester.pump();
    expect(find.text('Connecting to cloud-box…'), findsOneWidget);

    move.set(_at(MovePhase.confirm));
    await tester.pump();
    expect(find.text('123 456'), findsOneWidget);
    await tester.tap(find.text('They match'));
    await tester.pump();
    expect(move.asked.last, 'confirm');

    move.set(_at(MovePhase.checking));
    await tester.pump();
    expect(find.text('Checking with cloud-box…'), findsOneWidget);

    move.set(_at(MovePhase.copying, have: 1200000000, total: 4800000000));
    await tester.pump();
    expect(find.text('Copying to cloud-box'), findsOneWidget);
    expect(find.textContaining(' GB · copying what changed'), findsOneWidget);
    expect(find.textContaining('flutter'), findsOneWidget);
    expect(find.text('/Users/me → /home/you'), findsOneWidget);
    await tester.tap(find.text('Move now'));
    await tester.pump();
    expect(move.asked.last, 'freeze');
    await tester.tap(find.text('Cancel'));
    await tester.pump();
    expect(move.asked.last, 'cancel');
  });

  testWidgets('past the switch it can no longer be cancelled', (tester) async {
    final move = await _open(tester);
    move.set(_at(MovePhase.committing, canCancel: false));
    await tester.pump();
    expect(find.text('Moving to cloud-box…'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pump();
    expect(move.asked, isEmpty);
  });

  testWidgets('failed: why, what was in the way, and that it carries on', (
    tester,
  ) async {
    final move = await _open(tester);
    move.set(
      _at(
        MovePhase.failed,
        canCancel: false,
        error: "cloud-box doesn't have room for it.",
        problems: const ['It needs 4.8 GB and has 2.1 GB free.'],
      ),
    );
    await tester.pump();
    expect(find.text("Couldn't move to cloud-box"), findsOneWidget);
    expect(find.text("cloud-box doesn't have room for it."), findsOneWidget);
    expect(find.text('It needs 4.8 GB and has 2.1 GB free.'), findsOneWidget);
    expect(find.text('Claktory keeps running here as before.'), findsOneWidget);
    // Close clears it.
    await tester.tap(find.text('Close'));
    await tester.pump();
    expect(move.asked, ['cancel']);
  });

  testWidgets('the moving screen: how far, Cancel while it can, then moved', (
    tester,
  ) async {
    final move = _FakeMove();
    final harness = Harness(pb.Workspace());
    move.set(
      _at(MovePhase.importing, step: 'Putting the conversations in place…'),
    );
    await harness.show(
      tester,
      home: ValueListenableBuilder<MoveStatus>(
        valueListenable: move.status,
        builder: (context, status, _) =>
            MovingScreen(move: status, actions: move),
      ),
    );
    expect(find.text('Moving to cloud-box…'), findsOneWidget);
    expect(find.text('Putting the conversations in place…'), findsOneWidget);
    await tester.tap(find.text('Cancel move'));
    await tester.pump();
    expect(move.asked, ['cancel']);

    move.set(_at(MovePhase.done, canCancel: false));
    await tester.pump();
    expect(find.text('Moved to cloud-box'), findsOneWidget);
    expect(find.text('Cancel move'), findsNothing);
  });

  group('a move an agent started', () {
    final rivet = pb.Workspace(
      agents: [pb.Agent(id: 'rivet', name: 'Rivet', role: 'Admin')],
    );

    testWidgets('the moving screen says who started it, and when it fails', (
      tester,
    ) async {
      final move = _FakeMove();
      move.set(_at(MovePhase.frozen, startedBy: 'rivet'));
      await Harness(rivet).show(
        tester,
        home: ValueListenableBuilder<MoveStatus>(
          valueListenable: move.status,
          builder: (context, status, _) =>
              MovingScreen(move: status, actions: move),
        ),
      );
      expect(
        find.text(
          'Rivet started this move. Your agents are paused until Claktory '
          'starts on cloud-box.',
        ),
        findsOneWidget,
      );

      move.set(_at(MovePhase.failed, startedBy: 'rivet', error: 'Lost it.'));
      await tester.pump();
      expect(
        find.text("Rivet's move to cloud-box didn't finish."),
        findsOneWidget,
      );
      expect(find.text('Lost it.'), findsOneWidget);

      // The person's own move: as before.
      move.set(_at(MovePhase.frozen));
      await tester.pump();
      expect(
        find.text('Your agents are paused until Claktory starts on cloud-box.'),
        findsOneWidget,
      );
    });

    testWidgets("Settings' move row says who is moving it", (tester) async {
      final move = _FakeMove();
      await Harness(rivet).show(
        tester,
        home: Center(child: MoveSettingRow(source: move)),
      );
      final copying = _at(
        MovePhase.copying,
        have: 1200000000,
        total: 3400000000,
        startedBy: 'rivet',
      );
      move.set(copying);
      await tester.pump();
      expect(
        find.text(
          'Rivet is copying Claktory to cloud-box: ${moveBytes(copying)}.',
        ),
        findsOneWidget,
      );
      move.set(_at(MovePhase.frozen, startedBy: 'rivet'));
      await tester.pump();
      expect(
        find.text('Rivet is moving Claktory to cloud-box.'),
        findsOneWidget,
      );
      move.set(_at(MovePhase.failed, startedBy: 'rivet'));
      await tester.pump();
      expect(
        find.text("Rivet couldn't move Claktory to cloud-box."),
        findsOneWidget,
      );
      // An agent no longer listed.
      move.set(_at(MovePhase.frozen, startedBy: 'gone'));
      await tester.pump();
      expect(
        find.text('An agent is moving Claktory to cloud-box.'),
        findsOneWidget,
      );
      // The person's own.
      move.set(_at(MovePhase.frozen));
      await tester.pump();
      expect(find.text('Moving to cloud-box.'), findsOneWidget);
    });
  });

  test('the backend\'s move as the app shows it; unset is none', () {
    expect(moveStatusOf(null).phase, MovePhase.none);
    final status = moveStatusOf(
      pb.Move(
        phase: pb.MovePhase.MOVE_PHASE_COPYING,
        targetMachine: 'cloud-box',
        targetOs: 'linux',
        targetArch: 'x86_64',
        digits: '123 456',
        problems: ['No room.'],
        missingTools: ['flutter'],
        map: [pb.MovePath(from: '/Users/me', to: '/home/you')],
        planBytes: Int64(4800),
        bytesHave: Int64(1200),
        bytesTotal: Int64(4800),
        round: 2,
        canCancel: true,
      ),
    );
    expect(status.phase, MovePhase.copying);
    expect(status.targetSystem, 'Linux x86_64');
    expect(status.map.single, (from: '/Users/me', to: '/home/you'));
    expect(moveProgress(status), 0.25);
    expect(status.problems, ['No room.']);
    expect(status.canCancel, isTrue);
    for (final (phase, frozen) in [
      (pb.MovePhase.MOVE_PHASE_CHECKING, false),
      (pb.MovePhase.MOVE_PHASE_FROZEN, true),
      (pb.MovePhase.MOVE_PHASE_IMPORTING, true),
      (pb.MovePhase.MOVE_PHASE_COMMITTING, true),
      (pb.MovePhase.MOVE_PHASE_DONE, true),
      (pb.MovePhase.MOVE_PHASE_FAILED, false),
    ]) {
      expect(
        moveStatusOf(pb.Move(phase: phase)).frozen,
        frozen,
        reason: '$phase',
      );
    }
  });

  testWidgets('the window gives way to the moving screen while the agents '
      'are paused, and a failure then stays until closed', (tester) async {
    final move = _FakeMove();
    final harness = Harness(pb.Workspace());
    await harness.show(
      tester,
      home: MoveGate(
        source: move,
        rail: const SizedBox(width: 60, child: Text('rail')),
        child: const Text('the window'),
      ),
    );
    expect(find.text('the window'), findsOneWidget);

    move.set(_at(MovePhase.copying, total: 10));
    await tester.pump();
    expect(find.text('the window'), findsOneWidget);

    move.set(_at(MovePhase.frozen, have: 5, total: 10));
    await tester.pump();
    expect(find.text('the window'), findsNothing);
    expect(find.text('Moving to cloud-box…'), findsOneWidget);
    expect(find.text('rail'), findsOneWidget, reason: 'the rail stays');

    move.set(_at(MovePhase.failed, error: "cloud-box didn't answer."));
    await tester.pump();
    expect(find.text("Couldn't move to cloud-box"), findsOneWidget);
    expect(find.text("cloud-box didn't answer."), findsOneWidget);
    await tester.tap(find.text('Close'));
    await tester.pump();
    expect(move.asked, ['cancel']);

    move.set(MoveStatus.none);
    await tester.pump();
    expect(find.text('the window'), findsOneWidget);

    // A failure before the pause is Settings' to tell.
    move.set(_at(MovePhase.failed, error: 'No room.'));
    await tester.pump();
    expect(find.text('the window'), findsOneWidget);
  });

  testWidgets('a device that loses the connection mid-move says so in place '
      'of the last step, without a Cancel that could do nothing', (
    tester,
  ) async {
    final move = _FakeMove();
    final harness = Harness(pb.Workspace());
    await harness.show(
      tester,
      home: MoveGate(source: move, child: const Text('the window')),
    );
    const lost =
        "Can't connect to Home right now. If the move finished, this device "
        'connects to cloud-box by itself.';
    move.set(
      _at(MovePhase.frozen, step: 'Stopping agents for the last round…'),
    );
    await tester.pump();
    expect(find.text('Stopping agents for the last round…'), findsOneWidget);
    expect(find.text('Cancel move'), findsOneWidget);
    expect(find.byType(LiveDot), findsOneWidget);

    // The source stops its tunnel at the hand-over.
    move.connected.value = false;
    await tester.pump();
    expect(find.text('Moving to cloud-box…'), findsOneWidget);
    expect(find.text(lost), findsOneWidget);
    expect(find.text('Stopping agents for the last round…'), findsNothing);
    expect(find.text('Cancel move'), findsNothing);
    expect(find.byType(LiveDot), findsNothing, reason: 'nothing live');

    // Back again, it's live as before.
    move.connected.value = true;
    await tester.pump();
    expect(find.text(lost), findsNothing);
    expect(find.text('Cancel move'), findsOneWidget);

    // Done is done, connected or not; a failure can't be closed unheard.
    move.connected.value = false;
    move.set(_at(MovePhase.done, canCancel: false));
    await tester.pump();
    expect(find.text('Moved to cloud-box'), findsOneWidget);
    expect(find.text(lost), findsNothing);
    move.set(_at(MovePhase.failed, error: "cloud-box didn't answer."));
    await tester.pump();
    expect(find.text("Couldn't move to cloud-box"), findsOneWidget);
    expect(find.text('Close'), findsNothing);
  });

  test("the backend's move follows the backend's connection", () {
    final harness = Harness(pb.Workspace());
    final move = BackendMove(harness.backend);
    expect(identical(move.connected, harness.backend.connected), isTrue);
  });

  test("the backend's move sends the backend's requests", () async {
    final harness = Harness(pb.Workspace());
    final backend = harness.backend;
    final move = BackendMove(backend);
    await move.start(_code);
    await move.confirm();
    await move.freeze();
    await move.cancel();
    expect(backend.sent.map((message) => message.whichBody()), [
      pb.ClientMessage_Body.startMove,
      pb.ClientMessage_Body.confirmMove,
      pb.ClientMessage_Body.freezeMove,
      pb.ClientMessage_Body.cancelMove,
    ]);
    expect(backend.sent.first.startMove.code, _code);
  });

  testWidgets('the code field is a link, named when filled, never corrected, '
      'suggested or autofilled', (tester) async {
    final semantics = tester.ensureSemantics();
    await _open(tester);
    final field = find.byType(EditableText);
    await tester.enterText(field, _code);
    // Faded in: read by a screen reader from then on.
    await tester.pump(const Duration(milliseconds: 300));
    final editable = tester.widget<EditableText>(field);
    expect(editable.autocorrect, isFalse);
    expect(editable.enableSuggestions, isFalse);
    expect(editable.autofillHints, isEmpty);
    expect(editable.keyboardType, TextInputType.url);
    expect(find.semantics.byLabel('Paste code'), findsOne);
    semantics.dispose();
  });

  testWidgets('on a phone, with the keyboard up and the largest text, the '
      'whole form scrolls: the field and Connect stay in reach', (
    tester,
  ) async {
    final move = _FakeMove();
    final harness = Harness(pb.Workspace());
    tester.platformDispatcher.textScaleFactorTestValue = 3;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    tester.view.viewInsets = const FakeViewPadding(bottom: 240);
    addTearDown(tester.view.resetViewInsets);
    await harness.show(
      tester,
      size: const Size(360, 640),
      // As in Settings, which scrolls.
      home: SingleChildScrollView(child: MoveSettingRow(source: move)),
    );
    await tester.tap(find.text('Move to another computer…'));
    await tester.pumpAndSettle();
    final field = find.byType(EditableText);
    await tester.enterText(field, _code);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.ensureVisible(find.text('Connect'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(move.asked, ['start $_code']);
  });

  testWidgets("the form's title is a heading; on a phone it names the sheet", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    for (final (size, phone) in [
      (const Size(1200, 900), false),
      (const Size(390, 844), true),
    ]) {
      final harness = Harness(pb.Workspace());
      await harness.show(
        tester,
        size: size,
        home: SingleChildScrollView(child: MoveSettingRow(source: _FakeMove())),
      );
      await tester.tap(find.text('Move to another computer…'));
      await tester.pumpAndSettle();
      final title = tester
          .getSemantics(find.text('Move to another computer'))
          .getSemanticsData();
      expect(title.label, 'Move to another computer');
      expect(title.flagsCollection.isHeader, isTrue);
      expect(title.flagsCollection.namesRoute, phone, reason: '$size');
    }
    semantics.dispose();
  });

  group('moved away', () {
    // A year gone, so it reads as a date whatever day the tests run.
    final at = DateTime(2020, 11, 21, 14, 20);
    pb.MovedAway away({int deleted = 0, String to = 'hetzner-box'}) =>
        pb.MovedAway(
          to: to,
          toOs: to.isEmpty ? '' : 'linux',
          atMs: Int64(at.millisecondsSinceEpoch),
          machine: 'mac-mini',
          deletedMs: Int64(deleted),
        );

    Future<_FakeMove> showGate(WidgetTester tester, {Size? size}) async {
      final move = _FakeMove();
      final harness = Harness(pb.Workspace());
      await harness.show(
        tester,
        size: size ?? const Size(1200, 900),
        home: MoveGate(
          source: move,
          rail: const SizedBox(width: 60, child: Text('rail')),
          child: const Text('the window'),
        ),
      );
      return move;
    }

    test('when it moved, in a sentence', () {
      final now = DateTime(2020, 11, 21, 18);
      expect(movedWhen(at, now: now), 'today at 14:20');
      expect(movedWhen(at, now: DateTime(2020, 11, 22)), 'on 21 Nov at 14:20');
      expect(
        movedWhen(at, now: DateTime(2021, 1, 2)),
        'on 21 Nov 2020 at 14:20',
      );
    });

    testWidgets('the window gives way to where it went, the rail beside it; '
        'Delete this copy… asks for "delete" typed, then deletes', (
      tester,
    ) async {
      final move = await showGate(tester);
      expect(find.text('the window'), findsOneWidget);

      move.movedAway.value = away();
      await tester.pump();
      expect(find.text('the window'), findsNothing);
      expect(find.text('rail'), findsOneWidget, reason: 'the rail stays');
      expect(find.text('This Claktory moved to hetzner-box'), findsOneWidget);
      expect(
        find.textContaining(
          'nothing in it runs, and everything since '
          'happens on hetzner-box',
        ),
        findsOneWidget,
      );

      await tester.tap(find.text('Delete this copy…'));
      await tester.pumpAndSettle();
      expect(find.text('Delete this copy?'), findsOneWidget);
      expect(
        find.textContaining('Your workspace and project folders stay'),
        findsOneWidget,
      );
      // Not until the word is typed.
      await tester.tap(find.text('Delete copy'));
      await tester.pump();
      expect(move.asked, isEmpty);
      await tester.enterText(find.byType(EditableText), 'delet');
      await tester.pump();
      await tester.tap(find.text('Delete copy'));
      await tester.pump();
      expect(move.asked, isEmpty);
      await tester.enterText(find.byType(EditableText), 'delete');
      await tester.pump();
      await tester.tap(find.text('Delete copy'));
      await tester.pumpAndSettle();
      expect(move.asked, ['delete copy delete']);
      expect(find.text('Delete this copy?'), findsNothing);

      // Asked again, it says when the copy went; nothing more to delete.
      move.movedAway.value = away(
        deleted: at.add(const Duration(days: 1)).millisecondsSinceEpoch,
      );
      await tester.pump();
      expect(find.textContaining('was deleted on 22 Nov 2020'), findsOneWidget);
      expect(find.text('Delete this copy…'), findsNothing);

      // Another Claktory picked: the window comes back.
      move.movedAway.value = null;
      await tester.pump();
      expect(find.text('the window'), findsOneWidget);
    });

    testWidgets('a refusal says why, in the dialog', (tester) async {
      final move = await showGate(tester);
      move.movedAway.value = away();
      move.refuse = 'Type delete to delete the copy.';
      await tester.pump();
      await tester.tap(find.text('Delete this copy…'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(EditableText), 'delete');
      await tester.pump();
      await tester.tap(find.text('Delete copy'));
      await tester.pumpAndSettle();
      expect(find.text('Type delete to delete the copy.'), findsOneWidget);
      expect(find.text('Delete this copy?'), findsOneWidget);
    });

    testWidgets('moved by file, or seen from another device: no computer to '
        'name, and no Delete', (tester) async {
      final move = await showGate(tester);
      move.canDeleteMovedCopy = false;
      move.movedAway.value = away(to: '');
      await tester.pump();
      expect(
        find.text('This Claktory moved to another computer'),
        findsOneWidget,
      );
      expect(
        find.textContaining('happens on the other computer'),
        findsOneWidget,
      );
      expect(find.textContaining('left on mac-mini is a copy'), findsOneWidget);
      expect(find.text('You can delete it only on mac-mini.'), findsOneWidget);
      expect(find.text('Delete this copy…'), findsNothing);
    });

    testWidgets('on a phone at the largest text, it fits', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final move = await showGate(tester, size: const Size(390, 844));
      move.movedAway.value = away();
      await tester.pump();
      expect(tester.takeException(), isNull);
      await tester.ensureVisible(find.text('Delete this copy…'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Delete this copy…'));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.text('Delete this copy?'), findsOneWidget);
    });
  });

  group('a move an agent started', () {
    MoveStatus confirm({
      String agent = 'rivet',
      bool here = false,
      bool there = false,
    }) => MoveStatus(
      phase: MovePhase.confirm,
      targetMachine: 'cloud-box',
      targetOs: 'linux',
      targetArch: 'x86_64',
      digits: '123 456',
      canCancel: true,
      startedByAgentId: agent,
      confirmedHere: here,
      targetConfirmed: there,
      targetAddress: '203.0.113.7:4433',
    );

    Future<_FakeMove> gate(WidgetTester tester, {Size? size}) async {
      final move = _FakeMove();
      final harness = Harness(
        pb.Workspace(
          agents: [pb.Agent(id: 'rivet', name: 'Rivet')],
        ),
      );
      await harness.show(
        tester,
        size: size ?? const Size(1200, 900),
        home: MoveGate(source: move, child: const Text('the window')),
      );
      return move;
    }

    testWidgets("Claktory's own confirm floats over the window, in its own "
        'words: the machine, its system and the digits; Confirm and Cancel', (
      tester,
    ) async {
      final move = await gate(tester);
      move.set(confirm());
      await tester.pump();
      expect(find.text('the window'), findsOneWidget, reason: 'kept under it');
      expect(
        find.text('Move this Claktory to the computer named cloud-box?'),
        findsOneWidget,
      );
      expect(find.text('Rivet started this move.'), findsOneWidget);
      expect(
        find.text(
          'It moves this whole Claktory (agents, secrets, sign-ins and your '
          'devices’ connection) to the computer named cloud-box (Linux '
          'x86_64), and it stops running here.',
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Confirm only if you asked for this move and that computer shows '
          'these digits:',
        ),
        findsOneWidget,
      );
      expect(find.text('123 456'), findsOneWidget);
      expect(
        find.text('This computer reached it at 203.0.113.7.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Confirm'));
      await tester.pump();
      expect(move.asked, ['confirm']);

      // Confirmed here: every device says it waits for the other side.
      move.set(confirm(here: true));
      await tester.pump();
      expect(find.text('Confirmed here'), findsOneWidget);
      expect(
        find.text('Waiting for cloud-box to confirm too.'),
        findsOneWidget,
      );
      expect(find.text('Confirm'), findsNothing);
      expect(find.text('123 456'), findsNothing);
      await tester.tap(find.text('Cancel move'));
      await tester.pump();
      expect(move.asked, ['confirm', 'cancel']);

      // Past the digits, it goes.
      move.set(_at(MovePhase.checking));
      await tester.pump();
      expect(find.text('Confirmed here'), findsNothing);
    });

    testWidgets('a move the person started has no such card: its form asks', (
      tester,
    ) async {
      final move = await gate(tester);
      move.set(confirm(agent: ''));
      await tester.pump();
      expect(
        find.text('Move this Claktory to the computer named cloud-box?'),
        findsNothing,
      );
    });

    testWidgets('on a phone at the largest text, it fits', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final move = await gate(tester, size: const Size(390, 844));
      move.set(confirm());
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(find.text('Confirm'), findsOneWidget);
    });

    testWidgets('on a phone it sits at the foot: the header and banners '
        'above stay in reach', (tester) async {
      final move = await gate(tester, size: const Size(390, 844));
      move.set(confirm());
      await tester.pump();
      final card = tester.getRect(find.byType(MoveConfirmCard));
      expect(card.bottom, greaterThan(844 - 40));
      // Nothing of it over the top of the screen, where the header's
      // menu and the banners are.
      expect(card.top, greaterThan(100));
    });

    test('an address without its port', () {
      expect(addressHost('203.0.113.7:4433'), '203.0.113.7');
      expect(addressHost('[2001:db8::1]:4433'), '2001:db8::1');
      expect(addressHost('2001:db8::1'), '2001:db8::1');
      expect(addressHost('203.0.113.7'), '203.0.113.7');
    });

    test('it waits for the person until they confirm here', () {
      expect(confirm().awaitsPerson, isTrue);
      expect(confirm(here: true).awaitsPerson, isFalse);
      expect(confirm(agent: '').awaitsPerson, isFalse);
    });
  });
}
