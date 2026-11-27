import 'dart:io';

import 'package:factory_app/attachments.dart' show FileDrop;
import 'package:factory_app/card_files.dart' show cardFilesKey;
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart' show Button, GlyphButton;
import 'package:factory_app/recovery_key.dart' show SaveRecoveryKeyForm;
import 'package:factory_app/dictation.dart' show DictationHold;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart' show FactoryApp;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/rail.dart' show DeviceDrafts;
import 'package:factory_app/review_answer.dart'
    show ReviewAnswering, forgetReviewChoices, reviewChosen;
import 'package:factory_app/card_answer.dart';
import 'package:factory_app/identity.dart' show Palette, agentColors;
import 'package:factory_app/inbox.dart'
    show InboxView, NeedsYouCard, NeedsYouChip, NeedsYouList;
import 'package:factory_app/transcript.dart' show AgentTints;
import 'package:factory_app/needs_you_entries.dart';
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';

import 'dart:async';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'attachments_test.dart' show fromWindow;
import 'harness.dart' hide settle;
import 'harness.dart' as harness show settle;

/// The harness's settle, then a frame more: a card publishes its answer
/// into the stage's dock after the frame it changed in (AnswerDock.publish),
/// and the bar draws it in the next.
Future<void> settle(WidgetTester tester) async {
  await harness.settle(tester);
  await tester.pump();
}

/// June and Mista, each waiting on cards at [june] and [mista] (seq: when
/// it came, in minutes ago), and [devices] waiting to be allowed.
Workspace _waiting({
  List<int> june = const [],
  List<int> mista = const [],
  List<String> devices = const [],
}) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      color: 'violet',
      waitingSeqs: [for (final seq in june) Int64(seq)],
    ),
    Agent(
      id: 'mista',
      name: 'Mista',
      color: 'green',
      waitingSeqs: [for (final seq in mista) Int64(seq)],
    ),
  ],
  remote: RemoteAccess(
    pendingDevices: [
      for (final id in devices)
        PendingDevice(deviceId: id, name: 'iPad', requestedMs: Int64(1)),
    ],
  ),
);

final _base = DateTime(2026, 11, 26, 9);

/// A question at [seq] that came [minutesAgo] before 9:00.
Entry _ask(int seq, int minutesAgo, {int revision = 0, bool text = false}) =>
    Entry(
      seq: Int64(seq),
      timestampMs: Int64(
        _base.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
      ),
      askPerson: AskPerson(
        id: 'q$seq',
        title: 'Question $seq',
        revision: revision,
        wantsText: text,
      ),
    );

void main() {
  late RecordingBackend backend;
  late DateTime now;

  Future<FocusSession> start(Workspace workspace) async {
    backend.workspace.apply(workspace);
    final session = FocusSession(
      backend,
      entries: NeedsYouEntries.of(backend),
      now: () => now,
    );
    addTearDown(session.dispose);
    await pumpEventQueue();
    return session;
  }

  List<String> keys(FocusSession session) => [
    for (final item in session.items) item.key,
  ];

  setUp(() {
    backend = RecordingBackend();
    now = _base;
    backend.entriesById
      ..['june/1'] = _ask(1, 30)
      ..['june/2'] = _ask(2, 60)
      ..['mista/7'] = _ask(7, 5);
  });

  test('what stops agents first, then the newest', () async {
    final session = await start(
      _waiting(june: [1, 2], mista: [7], devices: ['d1']),
    );
    // Mista's 5 min ago, June's 30 and 60.
    expect(keys(session), [
      'device:d1',
      'agent:mista/7',
      'agent:june/1',
      'agent:june/2',
    ]);
    expect(session.current?.key, 'device:d1');
    expect(session.position, (1, 4));
  });

  test('J and K go through what waits, round', () async {
    final session = await start(_waiting(june: [1, 2], mista: [7]));
    expect(session.current?.key, 'agent:mista/7');
    session.next();
    expect(session.current?.key, 'agent:june/1');
    session.previous();
    session.previous();
    expect(session.current?.key, 'agent:june/2');
  });

  test('answered here: how it ended, a receipt, and on to the next', () async {
    final session = await start(_waiting(june: [1, 2], mista: [7]));
    session.sending('agent:mista/7');
    expect(session.outcomeOf('agent:mista/7')?.words, 'Sending…');
    backend.workspace.apply(_waiting(june: [1, 2]));
    await pumpEventQueue();
    expect(session.outcomeOf('agent:mista/7')?.words, 'Approved');
    expect(session.receipt, 'Approved · Mista');
    expect(session.current?.key, 'agent:june/1');
    expect(session.answered, 1);
    // It keeps its place, answered, and J skips it.
    expect(keys(session).first, 'agent:mista/7');
    session.next();
    session.next();
    expect(session.current?.key, 'agent:june/1');
  });

  test(
    'dismissed here with a reason: the receipt says Dismissed, not '
    'Approved (Zucchero row 650); a no refused, then a yes, says Approved',
    () async {
      final session = await start(_waiting(june: [1, 2], mista: [7]));
      expect(session.current?.key, 'agent:mista/7');
      var refuse = true;
      session.answer.value = CardAnswer(
        key: 'agent:mista/7',
        docked: true,
        yes: 'Approve',
        no: 'Dismiss',
        onYes: () async {},
        onNo: (_) async {
          if (refuse) throw const AnswerNotSent();
        },
      );
      await session.no(reason: 'Unclear', askWhy: false);
      expect(session.outcomeOf('agent:mista/7'), isNull, reason: 'refused');
      await session.yes();
      backend.workspace.apply(_waiting(june: [1, 2]));
      await pumpEventQueue();
      expect(session.receipt, 'Approved · Mista');

      refuse = false;
      session.answer.value = CardAnswer(
        key: 'agent:june/1',
        docked: true,
        yes: 'Approve',
        no: 'Dismiss',
        onYes: () async {},
        onNo: (_) async {},
      );
      expect(session.current?.key, 'agent:june/1');
      await session.no(reason: 'Unclear', askWhy: false);
      backend.workspace.apply(_waiting(june: [2]));
      await pumpEventQueue();
      expect(session.outcomeOf('agent:june/1')?.words, 'Dismissed');
      expect(session.receipt, 'Dismissed · June');
    },
  );

  test(
    "an errand's or a secret's card ends in its own words: Paid, "
    "Couldn't pay, Gave Hetzner, a login's no Dismissed, Allowed once",
    () async {
      Entry payment(int seq, int minutesAgo) => _ask(seq, minutesAgo)
        ..askPerson.handoff = ErrandHandoff(
          kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_PAYMENT,
          domain: 'example.com',
        );
      Entry secret(int seq, SecretRequest request) => Entry(
        seq: Int64(seq),
        timestampMs: Int64(_base.millisecondsSinceEpoch),
        secretRequest: request,
      );
      backend.entriesById
        ..['june/1'] = payment(1, 30)
        ..['june/2'] = payment(2, 60)
        ..['mista/7'] = secret(
          7,
          SecretRequest(
            id: 'r7',
            state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
            secretName: 'Hetzner',
          ),
        )
        ..['mista/8'] = secret(
          8,
          SecretRequest(
            id: 'r8',
            state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
            login: true,
            loginSite: 'x.com',
          ),
        )
        ..['mista/9'] = Entry(
          seq: Int64(9),
          timestampMs: Int64(_base.millisecondsSinceEpoch),
          secretApproval: SecretApproval(
            id: 'a9',
            state: SecretApprovalState.SECRET_APPROVAL_STATE_ALLOWED,
          ),
        );
      final session = await start(_waiting(june: [1, 2], mista: [7, 8, 9]));
      Future<void> no(String key) async {
        while (session.current?.key != key) {
          session.next();
        }
        session.answer.value = CardAnswer(
          key: key,
          docked: true,
          yes: 'Yes',
          no: 'No',
          onYes: () async {},
          onNo: (_) async {},
        );
        await session.no(reason: 'Not now', askWhy: false);
      }

      session.sending('agent:june/1');
      session.sending('agent:mista/7');
      session.sending('agent:mista/9');
      await no('agent:june/2');
      await no('agent:mista/8');
      backend.workspace.apply(_waiting());
      await pumpEventQueue();
      expect(session.outcomeOf('agent:june/1')?.words, 'Paid');
      expect(session.outcomeOf('agent:june/2')?.words, "Couldn't pay");
      expect(session.outcomeOf('agent:mista/7')?.words, 'Gave Hetzner');
      expect(session.outcomeOf('agent:mista/8')?.words, 'Dismissed');
      expect(session.outcomeOf('agent:mista/9')?.words, 'Allowed once');
    },
  );

  test('answered elsewhere: it stays, saying so', () async {
    final session = await start(_waiting(june: [1, 2], mista: [7]));
    backend.workspace.apply(_waiting(june: [1, 2]));
    await pumpEventQueue();
    expect(session.current?.key, 'agent:mista/7');
    expect(
      session.outcomeOf('agent:mista/7')?.words,
      'Answered on another device',
    );
    expect(session.answered, 0);
    // The card says which device, once it comes again (§9.2).
    backend.entriesById['mista/7'] = _ask(7, 5)
      ..answeredOn = AnsweredOn(deviceId: 'd9', deviceName: "Matt's iPhone");
    NeedsYouEntries.of(backend).fetch(session.current!);
    await pumpEventQueue();
    expect(
      session.outcomeOf('agent:mista/7')?.words,
      "Answered on Matt's iPhone",
    );
  });

  test('on or off is the backend\'s, by device (§9.1)', () async {
    backend.workspace.apply(_waiting(june: [1]));
    expect(focusModeOf(backend).value, isFalse);
    setFocusMode(backend, true);
    expect(focusModeOf(backend).value, isTrue, reason: 'at once');
    expect(backend.sent.last.setFocusMode.on, isTrue);
    // Said back for this computer's app; another device's is its own.
    backend.workspace.apply(
      _waiting(june: [1])..settings = Settings(focusMode: ['local']),
    );
    expect(focusModeOf(backend).value, isTrue);
    backend.workspace.apply(
      _waiting(june: [1])..settings = Settings(focusMode: ['d9']),
    );
    expect(focusModeOf(backend).value, isFalse);
  });

  test('every run starts out of focus mode (#599, Squalo\'s F1): a device '
      'still listed when the app first hears from its Claktory (quit in '
      'focus mode) is taken out, and never shown in it', () async {
    expect(focusModeOf(backend).value, isFalse);
    backend.workspace.apply(
      _waiting(june: [1])..settings = Settings(focusMode: ['local']),
    );
    expect(focusModeOf(backend).value, isFalse);
    expect(backend.sent.last.setFocusMode.on, isFalse);
    // Its echo, then on again in this run: kept, through what comes after.
    backend.workspace.apply(_waiting(june: [1]));
    setFocusMode(backend, true);
    backend.workspace.apply(
      _waiting(june: [1])..settings = Settings(focusMode: ['local']),
    );
    expect(focusModeOf(backend).value, isTrue);
    expect(
      backend.sent
          .where((m) => m.hasSetFocusMode())
          .map((m) => m.setFocusMode.on),
      [false, true],
    );
  });

  test('another Claktory opened in focus mode from the queue before this '
      'run heard from it stays in it (Polnareff 48256)', () async {
    setFocusMode(backend, true);
    backend.workspace.apply(
      _waiting(june: [1])..settings = Settings(focusMode: ['local']),
    );
    expect(focusModeOf(backend).value, isTrue);
    expect(
      backend.sent
          .where((m) => m.hasSetFocusMode())
          .map((m) => m.setFocusMode.on),
      [true],
    );
  });

  test('a card asked for (a notification tapped) comes up once it has '
      'come', () async {
    backend.needsYouCard.value = 'agent:june/2';
    final session = await start(_waiting(june: [1]));
    expect(session.current?.key, 'agent:june/1');
    expect(backend.needsYouCard.value, 'agent:june/2', reason: 'kept');
    backend.workspace.apply(_waiting(june: [1, 2]));
    await pumpEventQueue();
    expect(session.current?.key, 'agent:june/2');
    expect(backend.needsYouCard.value, isNull);
    // The order still settles around it: Mista's older card comes in
    // first, and June's stays showing.
    backend.workspace.apply(_waiting(june: [1, 2], mista: [7]));
    await pumpEventQueue();
    expect(session.current?.key, 'agent:june/2');
    expect(keys(session).first, 'agent:mista/7');
    expect(session.position, (3, 3));
  });

  test('a new card goes in at the top of its kind, as the newest; it never '
      'takes the stage, nor while the order still settles', () async {
    final session = await start(_waiting(june: [1], mista: [7]));
    expect(session.current?.key, 'agent:mista/7');
    backend.entriesById['june/3'] = _ask(3, 1);
    backend.workspace.apply(_waiting(june: [1, 3], mista: [7]));
    await pumpEventQueue();
    expect(keys(session), ['agent:june/3', 'agent:mista/7', 'agent:june/1']);
    expect(session.current?.key, 'agent:mista/7', reason: 'not settled yet');
    session.next();
    expect(session.current?.key, 'agent:june/1');
    backend.workspace.apply(
      _waiting(june: [1, 3], mista: [7], devices: ['d1']),
    );
    await pumpEventQueue();
    expect(keys(session), [
      'device:d1',
      'agent:june/3',
      'agent:mista/7',
      'agent:june/1',
    ]);
    // The card showing stays.
    expect(session.current?.key, 'agent:june/1');
  });

  test('answered, the next is the row below; past the last, the top again '
      'if anything newer waits', () async {
    final session = await start(_waiting(june: [1, 2], mista: [7]));
    session.select('agent:june/1');
    session.sending('agent:june/1');
    backend.workspace.apply(_waiting(june: [2], mista: [7]));
    await pumpEventQueue();
    expect(session.current?.key, 'agent:june/2', reason: 'the older one');
    session.sending('agent:june/2');
    backend.workspace.apply(_waiting(mista: [7]));
    await pumpEventQueue();
    expect(session.current?.key, 'agent:mista/7', reason: 'round to the top');
  });

  test('changed by its agent: no yes or no for a second', () async {
    final session = await start(_waiting(mista: [7]));
    expect(session.guarded, isFalse);
    backend.entriesById['mista/7'] = _ask(7, 5, revision: 2);
    // Fetched again as the workspace comes (a patch from the change).
    NeedsYouEntries.of(backend).fetch(session.current!);
    await pumpEventQueue();
    expect(session.guarded, isTrue);
    now = now.add(const Duration(milliseconds: 1100));
    expect(session.guarded, isFalse);
  });

  test('an alert marked read here says Read, as History does; read '
      'elsewhere, on another device', () async {
    Workspace withAlert(String id) => _waiting(june: [1])
      ..alerts.add(
        Alert(
          id: id,
          agentId: 'june',
          title: 'The build broke',
          sentMs: Int64(_base.millisecondsSinceEpoch),
        ),
      );
    final session = await start(withAlert('a1'));
    final here = keys(session).firstWhere((key) => key.contains('a1'));
    session.sending(here);
    backend.workspace.apply(_waiting(june: [1]));
    await pumpEventQueue();
    expect(session.outcomeOf(here)?.words, 'Read');
    backend.workspace.apply(withAlert('a2'));
    await pumpEventQueue();
    final elsewhere = keys(session).firstWhere((key) => key.contains('a2'));
    backend.workspace.apply(_waiting(june: [1]));
    await pumpEventQueue();
    expect(session.outcomeOf(elsewhere)?.words, 'Read on another device');
  });

  test('replied on and waiting again: it waits where it was', () async {
    final session = await start(_waiting(june: [1, 2], mista: [7]));
    session.sending('agent:mista/7');
    backend.workspace.apply(_waiting(june: [1, 2]));
    await pumpEventQueue();
    backend.workspace.apply(_waiting(june: [1, 2], mista: [7]));
    await pumpEventQueue();
    expect(session.outcomeOf('agent:mista/7'), isNull);
    expect(keys(session).first, 'agent:mista/7');
    expect(session.position, (2, 3));
  });

  group('on a Mac', () {
    Future<Harness> show(WidgetTester tester) async {
      tester.view.physicalSize = const Size(1440, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final harness = Harness(
        _waiting(june: [1, 2], mista: [7])
          ..projects.add(Project(id: 'p1', name: 'Factory'))
          ..agents.first.projectId = 'p1',
      );
      harness.backend.entriesById
        ..['june/1'] = _ask(1, 30)
        ..['june/2'] = _ask(2, 60)
        ..['mista/7'] = _ask(7, 5);
      await harness.show(tester);
      await settle(tester);
      return harness;
    }

    Future<void> focusKey(WidgetTester tester) async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyN);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await settle(tester);
    }

    // The card on the stage (the queue's rows say what they ask too).
    Finder onStage(String text) =>
        find.descendant(of: find.byType(FocusStage), matching: find.text(text));

    testWidgets('⌘⇧N: the queue and the oldest card, and back', (tester) async {
      await show(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
      await focusKey(tester);
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      expect(find.byType(FocusQueue), findsOneWidget);
      // The newest first: Mista's.
      expect(onStage('Question 7'), findsOneWidget);
      expect(onStage('Question 1'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
      await settle(tester);
      expect(onStage('Question 1'), findsOneWidget);
      await tester.tap(find.text('Done'));
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
    });

    testWidgets("the sidebar's Needs you opens focus mode at once, newest "
        'first; Done at the queue\'s top says its key on hover and leaves, '
        'the one way out', (tester) async {
      await show(tester);
      await tester.tap(find.text('Needs you'));
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      expect(find.byType(InboxView), findsNothing);
      expect(onStage('Question 7'), findsOneWidget);
      // One Done, in the queue, read as what it does.
      final done = find.byKey(const ValueKey('focus-done'));
      expect(
        find.descendant(of: find.byType(FocusQueue), matching: done),
        findsOneWidget,
      );
      expect(find.text('Done'), findsOneWidget);
      expect(find.bySemanticsLabel('Leave focus mode'), findsOneWidget);
      // Seen before the pointer finds it: filled at rest, as by touch.
      Color fill() =>
          ((tester
                      .widget<Container>(
                        find
                            .descendant(
                              of: done,
                              matching: find.byType(Container),
                            )
                            .first,
                      )
                      .decoration!
                  as BoxDecoration)
              .color!);
      final palette = Palette.of(tester.element(done));
      expect(fill(), palette.hover);
      // A pointer's hover shows, as with a mouse.
      FocusManager.instance.highlightStrategy =
          FocusHighlightStrategy.alwaysTraditional;
      addTearDown(
        () => FocusManager.instance.highlightStrategy =
            FocusHighlightStrategy.automatic,
      );
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      addTearDown(mouse.removePointer);
      await mouse.moveTo(tester.getCenter(done));
      await tester.pump(const Duration(milliseconds: 400));
      await tester.pump(const Duration(milliseconds: 400));
      expect(fill(), palette.selected);
      expect(find.text('Leave focus mode · Esc'), findsOneWidget);
      await mouse.moveTo(Offset.zero);
      await tester.pump();
      await tester.tap(done);
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
      expect(find.byType(InboxView), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("an alert's row says when it came, as a request's does", (
      tester,
    ) async {
      final harness = await show(tester);
      await harness.apply(
        tester,
        harness.backend.workspace.workspace.deepCopy()
          ..alerts.add(
            Alert(
              id: 'a1',
              agentId: 'june',
              title: 'The account service is down',
              urgent: true,
              sentMs: Int64(
                DateTime.now()
                    .subtract(const Duration(minutes: 12))
                    .millisecondsSinceEpoch,
              ),
            ),
          ),
      );
      await focusKey(tester);
      expect(
        find.descendant(
          of: find.byType(FocusQueue),
          matching: find.text('12 min ago'),
        ),
        findsOneWidget,
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('saving the recovery key: the stage says what waits and '
        'opens the form, with no conversation to go to', (tester) async {
      final harness = await show(tester);
      await harness.apply(
        tester,
        harness.backend.workspace.workspace.deepCopy()
          ..settings = Settings(name: 'Studio')
          ..vault = VaultState(
            exists: true,
            unlocked: true,
            recoveryKeyUnconfirmed: true,
          ),
      );
      await focusKey(tester);
      await tester.tap(
        find.descendant(
          of: find.byType(FocusQueue),
          matching: find.textContaining('Save the recovery key for'),
        ),
      );
      await settle(tester);
      expect(
        find.descendant(
          of: find.byType(FocusStage),
          matching: find.textContaining('Your way back into your secrets'),
        ),
        findsOneWidget,
      );
      expect(find.text('Open conversation'), findsNothing);
      await tester.tap(find.widgetWithText(Button, 'Save recovery key…'));
      await settle(tester);
      expect(find.byType(SaveRecoveryKeyForm), findsOneWidget);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("an agent's alert goes to its conversation; a device waiting "
        'has none to go to', (tester) async {
      final harness = await show(tester);
      await harness.apply(
        tester,
        (_waiting(june: [1], devices: ['d1'])
            ..remote.pendingDevices.first.requestedMs = Int64(
              DateTime.now().millisecondsSinceEpoch,
            ))
          ..alerts.add(
            Alert(
              id: 'a1',
              agentId: 'june',
              title: 'The build broke',
              sentMs: Int64(DateTime.now().millisecondsSinceEpoch),
            ),
          ),
      );
      await focusKey(tester);
      Future<void> choose(String text) async {
        await tester.tap(
          find
              .descendant(
                of: find.byType(FocusQueue),
                matching: find.textContaining(text),
              )
              .first,
        );
        await settle(tester);
      }

      await choose('Allow a new device');
      expect(find.text('Open conversation'), findsNothing);
      await choose('The build broke');
      await tester.tap(find.text('Open conversation'));
      await settle(tester);
      expect(harness.backend.sent.last.selectAgent.agentId, 'june');
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("the conversation's chip opens focus mode at its own "
        'request, with no drop-down', (tester) async {
      final harness = await show(tester);
      await harness.apply(
        tester,
        harness.backend.workspace.workspace.deepCopy()
          ..selectedAgentId = 'june',
      );
      await settle(tester);
      // Nothing drops down: no chevron.
      expect(
        find.descendant(
          of: find.byType(NeedsYouChip),
          matching: find.byType(AnimatedRotation),
        ),
        findsNothing,
      );
      await tester.tap(find.text('3 need you'));
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      // June's latest request, not the queue's top (Mista's).
      expect(onStage('Question 2'), findsOneWidget);
      expect(find.byType(NeedsYouList), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("the View menu's Focus on Needs You goes in and out", (
      tester,
    ) async {
      await show(tester);
      Future<void> menu() async {
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          'factory/menu',
          const StandardMethodCodec().encodeMethodCall(
            const MethodCall('focusMode'),
          ),
          (_) {},
        );
        await settle(tester);
      }

      await menu();
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      await menu();
      expect(find.byType(NeedsYouFocus), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('Waiting · History in the queue: a History row shows its card '
        'on the stage as it ended, read only; Waiting is back to what '
        'waits', (tester) async {
      final harness = await show(tester);
      harness.backend.needsYouHistory = [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
          agentId: 'june',
          agentName: 'June',
          seq: Int64(4),
          title: 'Question 4',
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED,
          decidedMs: Int64(_base.millisecondsSinceEpoch),
        ),
      ];
      harness.backend.entriesById['june/4'] = _ask(4, 90);
      await focusKey(tester);
      final queue = find.byType(FocusQueue);
      await tester.tap(
        find.descendant(of: queue, matching: find.text('History')),
      );
      await settle(tester);
      expect(
        find.descendant(of: queue, matching: find.text('Question 1')),
        findsNothing,
      );
      await tester.tap(
        find.descendant(of: queue, matching: find.text('Question 4')),
      );
      await settle(tester);
      expect(onStage('Question 4'), findsOneWidget);
      expect(find.byKey(const ValueKey('focus-history-ended')), findsOneWidget);
      // Read only: nothing answers it, ⌘↩ included.
      expect(
        tester.widget<FocusStage>(find.byType(FocusStage)).session.answerable,
        isFalse,
      );
      await tester.tap(
        find.descendant(of: queue, matching: find.text('Waiting')),
      );
      await settle(tester);
      expect(onStage('Question 7'), findsOneWidget);
    });

    testWidgets('⌘F finds in the queue; Esc clears it, then leaves', (
      tester,
    ) async {
      await show(tester);
      await focusKey(tester);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await settle(tester);
      final field = find.byKey(const ValueKey('focus-find'));
      expect(field, findsOneWidget);
      await tester.enterText(
        find.descendant(of: field, matching: find.byType(EditableText)),
        'mista',
      );
      await settle(tester);
      final queue = find.byType(FocusQueue);
      expect(
        find.descendant(of: queue, matching: find.text('Question 7')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: queue, matching: find.text('Question 1')),
        findsNothing,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      expect(field, findsNothing);
      expect(
        find.descendant(of: queue, matching: find.text('Question 1')),
        findsOneWidget,
      );
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('at its top, the queue stays put as requests come in under '
        'an urgent alert: nothing goes out of view', (tester) async {
      final harness = await show(tester);
      Workspace waitingOn(List<int> mista) =>
          (_waiting(june: [1, 2], mista: mista)
            ..alerts.add(
              Alert(
                id: 'a1',
                agentId: 'june',
                title: 'The account service is down',
                urgent: true,
                sentMs: Int64(DateTime.now().millisecondsSinceEpoch),
              ),
            ));
      for (final seq in [8, 9, 10]) {
        harness.backend.entriesById['mista/$seq'] = _ask(seq, 1);
      }
      await harness.apply(tester, waitingOn([7]));
      await focusKey(tester);
      final queue = find.byType(FocusQueue);
      Finder row(String text) =>
          find.descendant(of: queue, matching: find.textContaining(text));
      expect(row('The account service is down'), findsOneWidget);
      final top = tester.getTopLeft(row('The account service is down')).dy;
      await harness.apply(tester, waitingOn([7, 8, 9, 10]));
      await settle(tester);
      expect(row('Question 10'), findsOneWidget);
      expect(tester.getTopLeft(row('The account service is down')).dy, top);
      final pill = find.ancestor(
        of: find.byKey(const ValueKey('needs-you-new-above')),
        matching: find.byType(AnimatedOpacity),
      );
      expect(tester.widget<AnimatedOpacity>(pill.first).opacity, 0);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('a find cleared leaves the queue scrolled as it was before, '
        'not held at the row found', (tester) async {
      final harness = await show(tester);
      for (var seq = 10; seq < 40; seq++) {
        harness.backend.entriesById['june/$seq'] = _ask(seq, seq);
      }
      await harness.apply(
        tester,
        harness.backend.workspace.workspace.deepCopy()
          ..agents.first.waitingSeqs.addAll([
            for (var seq = 10; seq < 40; seq++) Int64(seq),
          ]),
      );
      await focusKey(tester);
      final queue = find.byType(FocusQueue);
      ScrollPosition position() => tester
          .state<ScrollableState>(
            find.descendant(of: queue, matching: find.byType(Scrollable)).first,
          )
          .position;
      expect(position().pixels, 0);
      expect(position().maxScrollExtent, greaterThan(0));
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await settle(tester);
      await tester.enterText(
        find.descendant(
          of: find.byKey(const ValueKey('focus-find')),
          matching: find.byType(EditableText),
        ),
        'Question 39',
      );
      await settle(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      expect(position().pixels, 0);
      expect(
        find.descendant(of: queue, matching: find.text('Question 7')),
        findsOneWidget,
      );
      expect(
        tester
            .getTopLeft(
              find.descendant(of: queue, matching: find.text('Question 7')),
            )
            .dy,
        lessThan(tester.getTopLeft(find.byType(FocusQueue)).dy + 200),
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('nothing left: Done is the stage\'s primary, and it leaves', (
      tester,
    ) async {
      final harness = await show(tester);
      harness.backend.workspace.apply(_waiting());
      await settle(tester);
      await focusKey(tester);
      final stage = find.byType(FocusStage);
      expect(
        find.descendant(of: stage, matching: find.text('Nothing needs you.')),
        findsOneWidget,
      );
      await tester.tap(find.descendant(of: stage, matching: find.text('Done')));
      await settle(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
    });

    testWidgets("on Android Needs you has no Focus mode", (tester) async {
      await show(tester);
      await tester.tap(find.text('Needs you'));
      await settle(tester);
      expect(find.text('Waiting'), findsOneWidget);
      expect(find.text('Focus mode'), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.android));

    testWidgets("the stage's answer bar takes files, as the card does: the "
        'paperclip, and what is dropped while its field has the keys '
        "(Zucchero's row 666)", (tester) async {
      final folder = Directory.systemTemp.createTempSync('focus-files');
      addTearDown(() => folder.deleteSync(recursive: true));
      final logo = '${folder.path}/logo.png';
      final notes = '${folder.path}/notes.txt';
      File(logo).writeAsBytesSync(List.filled(3000, 7));
      File(notes).writeAsStringSync('x' * 2048);
      final window =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      window.setMockMethodCallHandler(const MethodChannel('factory/window'), (
        call,
      ) async {
        if (call.method == 'pickFiles') return [logo];
        return null;
      });
      addTearDown(
        () => window.setMockMethodCallHandler(
          const MethodChannel('factory/window'),
          null,
        ),
      );
      Future<void> io() async {
        for (var i = 0; i < 12; i++) {
          await tester.pump();
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 10)),
          );
        }
        await settle(tester);
      }

      final harness = await show(tester);
      await focusKey(tester);
      expect(onStage('Question 7'), findsOneWidget);
      final waiting = harness.backend.attachments;
      final key = cardFilesKey('mista', 'q7');
      final clip = find.descendant(
        of: find.byType(FocusStage),
        matching: find.byWidgetPredicate(
          (widget) => widget is GlyphButton && widget.label == 'Attach files',
        ),
      );
      expect(clip, findsOneWidget);
      await tester.tap(clip);
      await io();
      expect(waiting.of(key).map((file) => file.name), ['logo.png']);
      // On the card, under its words.
      expect(onStage('logo.png'), findsOneWidget);
      // Into the field: what's dropped on the window comes here.
      await tester.sendKeyEvent(LogicalKeyboardKey.keyR);
      await settle(tester);
      expect(FileDrop.divertedTo.value, 'your answer to Mista');
      FileDrop.deliver([notes]);
      await io();
      expect(waiting.of(key).map((file) => file.name), [
        'logo.png',
        'notes.txt',
      ]);
      expect(waiting.of('mista'), isEmpty);
      // Dropped on the request's words, the keys out of its field: its
      // answer's all the same.
      waiting.remove(key, waiting.of(key).last);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      expect(FileDrop.divertedTo.value, isNull);
      final title = tester.getCenter(onStage('Question 7'));
      await fromWindow(tester, 'filesDropped', {
        'paths': [notes],
        'x': title.dx,
        'y': title.dy,
      });
      await io();
      expect(waiting.of(key).map((file) => file.name), [
        'logo.png',
        'notes.txt',
      ]);
      expect(waiting.of('mista'), isEmpty);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('answered with its own button, the next card comes up', (
      tester,
    ) async {
      final harness = await show(tester);
      await focusKey(tester);
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(harness.last.answerAsk.seq.toInt(), 7);
      harness.backend.workspace.apply(
        _waiting(june: [1, 2])
          ..projects.add(Project(id: 'p1', name: 'Factory'))
          ..agents.first.projectId = 'p1',
      );
      await tester.pump();
      await tester.pump();
      expect(find.text('Approved · Mista'), findsOneWidget);
      expect(onStage('Question 1'), findsOneWidget);
      await tester.pump(focusReceiptFor);
      await settle(tester);
      expect(find.text('Approved · Mista'), findsNothing);
    });

    // What the card on the stage publishes (Illuso's CardAnswer): here a
    // stand-in with its own field, recording what the keys did. The real
    // card publishes its own after each frame it builds in; the stand-in
    // takes its place again each time, as the card's.
    // Returns how to publish the stand-in anew (its words typed, say).
    void Function(CardAnswer) stand(WidgetTester tester, CardAnswer answer) {
      final dock = tester
          .element(find.byType(NeedsYouCard))
          .getInheritedWidgetOfExactType<AnswerDock>()!;
      var mine = answer.withKey(dock.itemKey ?? answer.key);
      final notifier = dock.answer;
      void keep() {
        final now = notifier.value;
        if (now != null && !identical(now, mine) && now.key == mine.key) {
          notifier.value = mine;
        }
      }

      notifier
        ..value = mine
        ..addListener(keep);
      addTearDown(() {
        try {
          notifier.removeListener(keep);
        } on Object {
          // Gone with the stage.
        }
      });
      return (next) => notifier.value = mine = next.withKey(mine.key);
    }

    Future<void> cmd(WidgetTester tester, LogicalKeyboardKey key) async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
    }

    Future<(List<String>, TextEditingController)> publish(
      WidgetTester tester, {
      Object? refuse,
      Future<void>? accepted,
      bool wantsText = false,
    }) async {
      final did = <String>[];
      final field = TextEditingController();
      addTearDown(field.dispose);
      // As ask_card.dart publishes it: a card that asks for words has
      // them as its yes (Answer), never typed, and no yes while empty.
      CardAnswer make() {
        final words = field.text.trim().isNotEmpty;
        return CardAnswer(
          docked: true,
          field: field,
          wantsText: wantsText,
          yes: wantsText ? 'Answer' : 'Approve',
          no: 'Dismiss',
          typed: words && !wantsText,
          onYes: wantsText && !words
              ? null
              : () async {
                  did.add(wantsText ? 'yes ${field.text}' : 'yes');
                  if (refuse != null) throw refuse;
                  await accepted;
                },
          onSubmit: () async => did.add('submit ${field.text}'),
          onNo: (reason) async => did.add('no $reason'),
        );
      }

      final update = stand(tester, make());
      // The card says words are typed as they are.
      field.addListener(() => update(make()));
      await tester.pump();
      return (did, field);
    }

    testWidgets("⌘↩ is the card's yes: sent, it moves on once accepted", (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = await show(tester);
      await focusKey(tester);
      final ack = Completer<void>();
      final (did, _) = await publish(tester, accepted: ack.future);
      // The bar under the card, with its keys.
      expect(find.text('⌘↩'), findsOneWidget);
      expect(find.text('⌘⌫'), findsOneWidget);
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(did, ['yes']);
      expect(find.text('Sending…'), findsOneWidget);
      ack.complete();
      harness.backend.workspace.apply(
        _waiting(june: [1, 2])
          ..projects.add(Project(id: 'p1', name: 'Factory'))
          ..agents.first.projectId = 'p1',
      );
      await tester.pump();
      await tester.pump();
      expect(onStage('Question 1'), findsOneWidget);
      await tester.pump(focusReceiptFor);
      await settle(tester);
    });

    testWidgets('refused, the card stays and nothing moves on', (tester) async {
      await show(tester);
      await focusKey(tester);
      final (did, _) = await publish(
        tester,
        refuse: const RequestFailure('The card changed while you answered.'),
      );
      await cmd(tester, LogicalKeyboardKey.enter);
      await tester.pump();
      expect(did, ['yes']);
      expect(find.text('Sending…'), findsNothing);
      expect(onStage('Question 7'), findsOneWidget);
    });

    testWidgets('nothing sent (a confirmation cancelled): it stays, with '
        'no error', (tester) async {
      await show(tester);
      await focusKey(tester);
      final (did, _) = await publish(tester, refuse: const AnswerNotSent());
      await cmd(tester, LogicalKeyboardKey.enter);
      await tester.pump();
      expect(did, ['yes']);
      expect(find.text('Sending…'), findsNothing);
      expect(onStage('Question 7'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('answered on a long-named device, its row says so whole on '
        'its second line, in a narrow queue', (tester) async {
      final harness = Harness(_waiting(june: [1], mista: [7]));
      tester.view.physicalSize = const Size(900, 700);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      harness.backend.entriesById
        ..['june/1'] = _ask(1, 30)
        ..['mista/7'] = _ask(7, 5);
      final session = FocusSession(harness.backend);
      addTearDown(session.dispose);
      await harness.show(
        tester,
        home: Align(
          alignment: Alignment.topLeft,
          child: FocusQueue(session: session, width: 200),
        ),
      );
      await settle(tester);
      harness.backend.entriesById['mista/7'] = _ask(7, 5)
        ..answeredOn = AnsweredOn(
          deviceId: 'd9',
          deviceName: "Matt's iPad Pro (13-inch, M4)",
        );
      harness.backend.workspace.apply(_waiting(june: [1]));
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(
        find.text("Answered on Matt's iPad Pro (13-inch, M4)"),
        findsOneWidget,
      );
    });

    testWidgets("1–9 choose among a card's choices, sending nothing", (
      tester,
    ) async {
      await show(tester);
      await focusKey(tester);
      final chose = <int>[];
      stand(
        tester,
        CardAnswer(
          docked: false,
          yes: 'Give',
          no: '',
          onYes: () async => chose.add(-1),
          choices: [
            for (var i = 0; i < 3; i++)
              CardChoice('AWS $i', chosen: false, choose: () => chose.add(i)),
          ],
        ),
      );
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.digit2);
      await tester.sendKeyEvent(LogicalKeyboardKey.digit9);
      expect(chose, [1]);
    });

    testWidgets('a card with something to show once answered stays on the '
        'stage until the person moves on', (tester) async {
      final harness = await show(tester);
      await focusKey(tester);
      stand(
        tester,
        CardAnswer(
          docked: false,
          yes: 'Make invite',
          no: '',
          keep: true,
          onYes: () async {},
        ),
      );
      await tester.pump();
      await cmd(tester, LogicalKeyboardKey.enter);
      harness.backend.workspace.apply(
        _waiting(june: [1, 2])
          ..projects.add(Project(id: 'p1', name: 'Factory'))
          ..agents.first.projectId = 'p1',
      );
      await settle(tester);
      expect(onStage('Question 7'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
      await settle(tester);
      expect(onStage('Question 1'), findsOneWidget);
    });

    testWidgets('a card that runs something takes ⌘↩ twice; a click runs it '
        'at once', (tester) async {
      await show(tester);
      await focusKey(tester);
      final ran = <String>[];
      final field = TextEditingController();
      addTearDown(field.dispose);
      stand(
        tester,
        CardAnswer(
          docked: true,
          field: field,
          yes: 'Execute',
          no: 'Decline',
          pressTwice: true,
          onYes: () async => ran.add('run'),
          onNo: (reason) async {},
        ),
      );
      await tester.pump();
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(ran, isEmpty);
      expect(find.text('Press ⌘↩ again to run'), findsOneWidget);
      await tester.pump(focusPressAgainWithin);
      expect(find.text('Press ⌘↩ again to run'), findsNothing);
      await cmd(tester, LogicalKeyboardKey.enter);
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(ran, ['run']);
      await tester.tap(find.text('Execute'));
      await tester.pump();
      expect(ran, ['run', 'run']);
    });

    testWidgets("moving on drops the card before's answer", (tester) async {
      await show(tester);
      await focusKey(tester);
      await publish(tester);
      final session = tester
          .widget<FocusStage>(find.byType(FocusStage))
          .session;
      final before = session.answer.value!.key;
      expect(before, 'agent:mista/7');
      await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
      // Gone as the stage moves, before the next card publishes its own.
      expect(session.answer.value, isNull);
      await settle(tester);
      expect(session.answer.value?.key, isNot(before));
    });

    testWidgets('⌘⌫ with nothing typed offers the reasons: a number sends '
        'one, ⌘⌫ again none, and they go by themselves', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await show(tester);
      await focusKey(tester);
      final (did, _) = await publish(tester);
      await cmd(tester, LogicalKeyboardKey.backspace);
      expect(did, isEmpty);
      expect(find.text('Not now'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.digit2);
      await tester.pump();
      expect(did, ['no Not now']);
      await cmd(tester, LogicalKeyboardKey.backspace);
      await cmd(tester, LogicalKeyboardKey.backspace);
      expect(did, ['no Not now', 'no null']);
      await cmd(tester, LogicalKeyboardKey.backspace);
      expect(find.text('Not now'), findsOneWidget);
      await tester.pump(focusReasonsFor);
      expect(find.text('Not now'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.digit1);
      expect(did, hasLength(2));
    });

    testWidgets('typed: ⌘↩ and Return submit the words; Return on an empty '
        'field does nothing', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await show(tester);
      await focusKey(tester);
      final (did, field) = await publish(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyR);
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(did, isEmpty);
      await tester.enterText(find.byType(EditableText), 'Use the 2nd');
      await tester.pump();
      expect(find.text('Submit'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(did, ['submit Use the 2nd']);
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(did, ['submit Use the 2nd', 'submit Use the 2nd']);
      // ⌘⌫ in the field is the field's.
      await cmd(tester, LogicalKeyboardKey.backspace);
      expect(did, hasLength(2));
      expect(field.text, isNot('Use the 2nd'));
    });

    testWidgets('typed: the text input\'s Send submits the words (an iPad\'s '
        'text view can keep Return from the keys: row 730); empty sends '
        'nothing; Shift-Return types a new line', (tester) async {
      await show(tester);
      await focusKey(tester);
      final (did, field) = await publish(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyR);
      await tester.pump();
      final text = tester.widget<EditableText>(find.byType(EditableText));
      expect(text.textInputAction, TextInputAction.send);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pump();
      expect(did, isEmpty);
      await tester.enterText(find.byType(EditableText), 'One');
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pump();
      expect(did, isEmpty);
      expect(field.text, 'One\n');
      await tester.enterText(find.byType(EditableText), 'One\nTwo');
      await tester.pump();
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pump();
      expect(did, ['submit One\nTwo']);
    });

    // A yes moves the card on once it's sent: each way on a card of its own.
    for (final (how, send) in [
      (
        'the text input\'s Send',
        (WidgetTester tester) async {
          await tester.testTextInput.receiveAction(TextInputAction.send);
        },
      ),
      (
        'Return',
        (WidgetTester tester) async {
          await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        },
      ),
    ]) {
      testWidgets('a card that asks for words: $how answers with them; '
          'empty, nothing', (tester) async {
        await show(tester);
        await focusKey(tester);
        final (did, _) = await publish(tester, wantsText: true);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyR);
        await tester.pump();
        await send(tester);
        await tester.pump();
        expect(did, isEmpty);
        await tester.enterText(find.byType(EditableText), 'Blue');
        await tester.pump();
        expect(find.text('Answer'), findsOneWidget);
        await send(tester);
        await tester.pump();
        expect(did, ['yes Blue']);
      });
    }

    testWidgets('held, ⌘↩ records into the field instead, and sends nothing', (
      tester,
    ) async {
      await show(tester);
      await focusKey(tester);
      final (did, _) = await publish(tester);
      final session = tester
          .widget<FocusStage>(find.byType(FocusStage))
          .session;
      // The stage's field takes what's said.
      expect(session.say, isNotNull);
      final say = _Say();
      session.say = say;
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
      await tester.pump(focusHoldToSay ~/ 2);
      expect(say.did, isEmpty);
      await tester.pump(focusHoldToSay);
      expect(say.did, ['start']);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
      expect(say.did, ['start', 'end']);
      expect(did, isEmpty);
    });

    testWidgets("the queue says what waits on the other Claktories; a tap "
        'carries on in focus mode there', (tester) async {
      tester.view.physicalSize = const Size(1440, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final connections = Connections(MemoryConnectionStorage());
      final nixos = parsePairingCode(
        'factory://?t=${'a' * 64}&name=nixos&e=${'c' * 64}',
      );
      final backends = <String, RecordingBackend>{};
      connections.use(nixos);
      connections.use(null);
      await tester.pumpWidget(
        FactoryApp(
          drafts: DeviceDrafts(MemoryDraftStorage()),
          connections: connections,
          localBackend: true,
          open: (target) {
            final backend = RecordingBackend()..connectTo(target);
            backend.workspace.apply(
              (target == null ? _waiting(june: [1]) : _waiting(mista: [7, 8]))
                // Named: no first-launch step over the window.
                ..settings = Settings(userName: 'Matt'),
            );
            return backends[target?.code ?? ''] = backend;
          },
        ),
      );
      await pumpFrames(tester);
      await focusKey(tester);
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      await tester.tap(find.text('nixos · 2'));
      await pumpFrames(tester);
      expect(connections.current?.code, nixos!.code);
      expect(find.byType(NeedsYouFocus), findsOneWidget);
      expect(focusModeOf(backends[nixos.code]!).value, isTrue);
      expect(find.text('nixos · 2'), findsNothing);
    });

    testWidgets('only other Claktories waiting: the stage says so, with '
        "theirs under it, and Done", (tester) async {
      tester.view.physicalSize = const Size(1440, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final connections = Connections(MemoryConnectionStorage());
      final nixos = parsePairingCode(
        'factory://?t=${'a' * 64}&name=nixos&e=${'c' * 64}',
      );
      final backends = <String, RecordingBackend>{};
      connections.use(nixos);
      connections.use(null);
      await tester.pumpWidget(
        FactoryApp(
          drafts: DeviceDrafts(MemoryDraftStorage()),
          connections: connections,
          localBackend: true,
          open: (target) {
            final backend = RecordingBackend()..connectTo(target);
            backend.workspace.apply(
              (target == null ? _waiting() : _waiting(mista: [7, 8]))
                ..settings = Settings(userName: 'Matt'),
            );
            return backends[target?.code ?? ''] = backend;
          },
        ),
      );
      await pumpFrames(tester);
      await focusKey(tester);
      final stage = find.byType(FocusStage);
      expect(
        find.descendant(
          of: stage,
          matching: find.textContaining('Nothing needs you on '),
        ),
        findsOneWidget,
      );
      expect(
        find.descendant(of: stage, matching: find.text('nixos · 2')),
        findsOneWidget,
      );
      // Its own Done, the queue's way out too.
      await tester.tap(find.descendant(of: stage, matching: find.text('Done')));
      await pumpFrames(tester);
      expect(find.byType(NeedsYouFocus), findsNothing);
    });

    testWidgets("Context follows the card's conversation without selecting "
        'it, with the card as a line and the box at its foot', (tester) async {
      final harness = await show(tester);
      await focusKey(tester);
      Iterable<WatchTranscript> watched() => harness.backend.sent
          .where((m) => m.hasWatchTranscript())
          .map((m) => m.watchTranscript);
      expect(watched().last, WatchTranscript(agentId: 'mista'));
      expect(find.text('Loading the conversation…'), findsOneWidget);
      harness.backend.watchedTranscript.load(
        Transcript(
          agentId: 'mista',
          entries: [
            Entry(
              seq: Int64(6),
              timestampMs: Int64(
                _base
                    .subtract(const Duration(minutes: 61))
                    .millisecondsSinceEpoch,
              ),
              user: UserText(text: 'Ship the migration'),
            ),
            _ask(7, 5),
          ],
        ),
      );
      await settle(tester);
      final context = find.byType(FocusContext);
      Finder inContext(Finder finder) =>
          find.descendant(of: context, matching: finder);
      expect(inContext(find.text('Ship the migration')), findsOneWidget);
      // In Mista's colors, as its own conversation has them.
      final tints = AgentTints.of(
        tester.element(inContext(find.text('Ship the migration'))),
      );
      expect(tints?.own, agentColors['green']);
      expect(tints?.byId['june'], agentColors['violet']);
      expect(inContext(find.text('▸ This request')), findsOneWidget);
      expect(inContext(find.text('Question 7')), findsNothing);
      expect(inContext(find.byType(MessageBox)), findsOneWidget);
      // Nothing selected: the other devices' view stays.
      expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
      // The keys stay with the card.
      await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
      await settle(tester);
      expect(watched().last, WatchTranscript(agentId: 'june'));
      expect(onStage('Question 1'), findsOneWidget);
      // C hides it; leaving stops watching.
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await settle(tester);
      expect(context, findsNothing);
      expect(watched().last, WatchTranscript());
    });

    testWidgets('in a window too narrow for Context beside the stage, it '
        "starts closed, so the stage's answers stay in reach; its button "
        'opens it over the stage; widened, it is back beside (row 1608)', (
      tester,
    ) async {
      await show(tester);
      tester.view.physicalSize = const Size(1123, 900);
      await settle(tester);
      await focusKey(tester);
      final context = find.byType(FocusContext);
      expect(context, findsNothing);
      expect(onStage('Question 7'), findsOneWidget);
      await tester.tap(
        find.descendant(
          of: find.byType(FocusStage),
          matching: find.text('Context'),
        ),
      );
      await settle(tester);
      expect(context, findsOneWidget);
      tester.view.physicalSize = const Size(1440, 900);
      await settle(tester);
      expect(context, findsOneWidget);
      tester.view.physicalSize = const Size(1123, 900);
      await settle(tester);
      expect(context, findsNothing);
    });

    testWidgets("the header's actions turn to glyphs when the words don't fit "
        'beside who asks', (tester) async {
      await show(tester);
      await focusKey(tester);
      expect(find.text('Open conversation'), findsOneWidget);
      // A narrow window at a large text size: glyphs, named to VoiceOver.
      tester.view.physicalSize = const Size(820, 900);
      tester.platformDispatcher.textScaleFactorTestValue = 1.6;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Open conversation'), findsNothing);
      expect(find.bySemanticsLabel('Open conversation'), findsOneWidget);
      expect(find.bySemanticsLabel('Context'), findsOneWidget);
      expect(find.text('Done'), findsOneWidget);
    });

    testWidgets('a file it asks to have reviewed is read on the stage, above '
        'the card', (tester) async {
      final harness = Harness(
        _waiting(june: [1], mista: [7])
          ..projects.add(Project(id: 'p1', name: 'Factory'))
          ..agents.first.projectId = 'p1',
      );
      tester.view.physicalSize = const Size(1440, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      addTearDown(forgetReviewChoices);
      harness.backend.entriesById
        ..['june/1'] = _ask(1, 30)
        ..['mista/7'] = (_ask(7, 5)
          ..askPerson.reviewPath = '/w/plan.png'
          ..askPerson.thread.add(AskThreadMessage(text: 'See the top')));
      await harness.show(tester);
      await settle(tester);
      await focusKey(tester);
      final stage = find.byType(FocusStage);
      expect(
        find.descendant(of: stage, matching: find.byType(ReviewAnswering)),
        findsOneWidget,
      );
      // As the card's Review does: the talk it showed, for the answer.
      expect(reviewChosen('mista'), 7);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
      await settle(tester);
      expect(
        find.descendant(of: stage, matching: find.byType(ReviewAnswering)),
        findsNothing,
      );
    });

    testWidgets("a platform's review that answers the card takes the stage: "
        "no card, no second bar, and ⌘↩ is its answer", (tester) async {
      final harness = Harness(_waiting(mista: [7]));
      tester.view.physicalSize = const Size(1200, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      addTearDown(forgetReviewChoices);
      harness.backend.entriesById['mista/7'] = _ask(7, 5)
        ..askPerson.reviewPath = '/w/plan.pdf';
      final session = FocusSession(harness.backend);
      addTearDown(session.dispose);
      final did = <String>[];
      await harness.show(
        tester,
        home: FocusKeys(
          session: session,
          leave: () {},
          child: FocusStage(
            session: session,
            leave: () {},
            reviewBody: (context, item, card) => FocusReviewBody(
              Builder(
                builder: (context) {
                  // Its own bar publishes the card's answer.
                  AnswerDock.of(context)!.answer.value = CardAnswer(
                    docked: false,
                    yes: 'Approve',
                    no: 'Dismiss',
                    onYes: () async => did.add('yes with drawings'),
                    onNo: (reason) async => did.add('no $reason'),
                  );
                  return const Text('Document host');
                },
              ),
              answers: true,
            ),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('Document host'), findsOneWidget);
      expect(find.byType(NeedsYouCard), findsNothing);
      expect(find.text('⌘↩'), findsNothing, reason: 'the stage draws no bar');
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(did, ['yes with drawings']);
    });

    testWidgets('a card changed by its agent takes no ⌘↩ for a moment, and '
        'its yes says Updated', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = await show(tester);
      await focusKey(tester);
      final (did, _) = await publish(tester);
      final session = tester
          .widget<FocusStage>(find.byType(FocusStage))
          .session;
      harness.backend.entriesById['mista/7'] = _ask(7, 5, revision: 1);
      NeedsYouEntries.of(harness.backend).fetch(session.current!);
      await tester.pump();
      await tester.pump();
      expect(find.text('Updated'), findsOneWidget);
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(did, isEmpty);
      await tester.pump(focusChangeGuard);
      await tester.pump();
      expect(find.text('Updated'), findsNothing);
      await cmd(tester, LogicalKeyboardKey.enter);
      expect(did, ['yes']);
    });
  });
}

class _Say implements DictationHold {
  final did = <String>[];

  @override
  Future<void> holdStart() async => did.add('start');

  @override
  void holdEnd() => did.add('end');
}
