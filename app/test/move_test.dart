import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/move.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
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
);

Future<_FakeMove> _open(WidgetTester tester) async {
  final move = _FakeMove();
  final harness = Harness(pb.Workspace());
  await harness.show(
    tester,
    home: Center(child: MoveSettingRow(source: move)),
  );
  await tester.tap(find.text('Move to another computer…'));
  await tester.pump();
  return move;
}

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
}
