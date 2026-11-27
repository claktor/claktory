// Android parity of recently shared product surfaces. Framework/fake evidence
// only: no native picker, drag host, IME, TalkBack or move transport acceptance.
import 'dart:io';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/attachments.dart' show FileDrop;
import 'package:factory_app/chats.dart' show ChatView;
import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/forms.dart' show FormFrame;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/move.dart'
    show MoveForm, MovingScreen, MoveSettingRow;
import 'package:factory_app/phone.dart' show PhoneShell;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/secrets.dart' show SecretRequestCard;
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_test.dart' show workspace;
import 'support/android_harness.dart';
import 'support/channel_fakes.dart';

final _receiveCode =
    'claktory-move://?e=${'a' * 64}&s=${'b' * 64}'
    '&m=audit-target&o=linux&h=aarch64&v=v0.1.15';

Future<void> _fonts() async {
  for (final (name, files) in [
    (
      'Geist',
      [
        'Geist-Regular.ttf',
        'Geist-Medium.ttf',
        'Geist-SemiBold.ttf',
        'Geist-Italic.ttf',
      ],
    ),
    ('GeistMono', ['GeistMono-Regular.ttf', 'GeistMono-Medium.ttf']),
  ]) {
    final loader = FontLoader(name);
    for (final file in files) {
      loader.addFont(
        Future.value(
          ByteData.sublistView(File('fonts/$file').readAsBytesSync()),
        ),
      );
    }
    await loader.load();
  }
}

void _tap(WidgetTester tester, String label, {Pattern? match}) {
  final finder = find.semantics.byLabel(match ?? label);
  expect(finder, findsOne);
  final node = finder.evaluate().single;
  final data = node.getSemanticsData();
  expect(data.flagsCollection.isHidden, isFalse, reason: label);
  expect(data.hasAction(SemanticsAction.tap), isTrue, reason: label);
  expect(node.rect.width, greaterThanOrEqualTo(48), reason: '$label width');
  expect(node.rect.height, greaterThanOrEqualTo(48), reason: '$label height');
  tester.semantics.performAction(finder, SemanticsAction.tap);
}

Future<void> _reveal(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await pumpFrames(tester);
}

int _keyboardHides(WidgetTester tester) => tester.testTextInput.log
    .where((call) => call.method == 'TextInput.hide')
    .length;

Future<void> _back(WidgetTester tester) async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
  await pumpFrames(tester);
}

Future<Object?> _gesture(String method) => sendNativeMethod(
  SystemChannels.backGesture,
  MethodCall(
    method,
    method == 'startBackGesture'
        ? {
            'touchOffset': [1.0, 180.0],
            'progress': 0.0,
            'swipeEdge': 0,
          }
        : null,
  ),
);

Future<void> _openMove(WidgetTester tester) async {
  _tap(tester, 'Menu');
  await pumpFrames(tester);
  final settings = find.descendant(
    of: find.byType(Sidebar),
    matching: find.text('Settings'),
  );
  await tester.scrollUntilVisible(
    settings,
    160,
    scrollable: find
        .descendant(of: find.byType(Sidebar), matching: find.byType(Scrollable))
        .first,
  );
  // Clear of the footer's finger-sized row, not cut at the list's edge.
  await Scrollable.ensureVisible(tester.element(settings), alignment: 0.5);
  await pumpFrames(tester);
  _tap(tester, 'Settings');
  await pumpFrames(tester);
  expect(find.byType(SettingsView), findsOneWidget);
  await _reveal(tester, find.text('General'));
  _tap(tester, 'General', match: RegExp(r'^General(?:\n|$)'));
  await pumpFrames(tester);
  final moveButton = find.text('Move to another computer…');
  await tester.scrollUntilVisible(
    moveButton,
    180,
    scrollable: _inside(SettingsView, find.byType(Scrollable)).first,
  );
  await pumpFrames(tester);
  expect(find.byType(MoveSettingRow), findsOneWidget);
  _tap(
    tester,
    'Move to another computer…',
    match: RegExp(r'^Move to another computer…(?:\n|$)'),
  );
  await pumpFrames(tester);
  expect(find.byType(MoveForm), findsOneWidget);
}

Move _move(MovePhase phase, {bool cancel = true}) => Move(
  phase: phase,
  targetMachine: 'audit-target',
  targetOs: 'linux',
  targetArch: 'aarch64',
  digits: '123 456',
  canCancel: cancel,
  bytesHave: Int64(5),
  bytesTotal: Int64(10),
  planBytes: Int64(10),
);

// The real backend updates both the snapshot and its retained move status.
void _publishMove(Harness harness, Move move) {
  harness.backend.workspace.apply(
    harness.backend.workspace.workspace.deepCopy()..move = move,
  );
  harness.backend.move.value = move;
}

SecretRequest _secret({bool collect = false}) => SecretRequest(
  id: 'audit-request',
  agentId: 'mike',
  adminAgentId: 'rivet',
  collecting: collect,
  state: collect
      ? SecretRequestState.SECRET_REQUEST_STATE_APPROVED
      : SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  newSecret: NewSecret(
    name: 'Audit token',
    description: 'Synthetic audit fixture.',
    variables: [SecretVariable(name: 'EXAMPLE_TOKEN')],
  ),
  reason: 'Exercise the shared request card.',
  // A connected service makes it: the card's main action is Let Rivet
  // make it, the admin agent's.
  fittingSecretId: collect ? '' : 'audit-connection',
  fittingSecretName: collect ? '' : 'Audit service',
);

Future<Harness> _showSecret(
  WidgetTester tester, {
  required Size size,
  required double scale,
  bool collect = false,
}) async {
  final id = collect ? 'rivet' : 'mike';
  final snapshot = workspace()..selectedAgentId = id;
  snapshot.vault = VaultState(
    exists: true,
    unlocked: true,
    secrets: [
      SecretInfo(
        id: 'audit-connection',
        name: 'Audit service',
        connection: SecretConnection(connectorId: 'audit'),
      ),
    ],
  );
  final harness = Harness(snapshot);
  harness.backend.transcript.load(
    Transcript(
      agentId: id,
      entries: [
        Entry(
          seq: Int64(7),
          secretRequest: _secret(collect: collect),
        ),
      ],
    ),
  );
  await showAndroid(tester, harness: harness, size: size, textScale: scale);
  expect(find.byType(AgentView), findsOneWidget);
  expect(find.byType(SecretRequestCard), findsOneWidget);
  return harness;
}

Finder _inside(Type type, Finder finder) =>
    find.descendant(of: find.byType(type), matching: finder);
Finder _secretValue() => find.descendant(
  of: find.byWidgetPredicate(
    (w) => w is TextField && w.placeholder == 'Paste it here',
  ),
  matching: find.byType(EditableText),
);

void main() {
  setUpAll(_fonts);
  setUp(() => PhonePlatformPolicy.debugAndroidPolicy = true);
  tearDown(() {
    FileDrop.dragged(0);
    PhonePlatformPolicy.debugAndroidPolicy = false;
  });
  final android = TargetPlatformVariant.only(TargetPlatform.android);

  testWidgets(
    'Move form preserves code on gesture cancel and separates IME from sheet Back',
    (tester) async {
      final harness = await showAndroid(tester);
      await _openMove(tester);
      final field = _inside(MoveForm, find.byType(EditableText));
      await tester.enterText(field, _receiveCode);
      await pumpFrames(tester);
      final editable = tester.widget<EditableText>(field);
      final heading = find.semantics.byLabel('Move to another computer');
      expect(heading, findsOne);
      final title = heading.evaluate().single.getSemanticsData();
      expect(title.flagsCollection.isHeader, isTrue);
      expect(title.flagsCollection.namesRoute, isTrue);
      expect(editable.autocorrect, isFalse);
      expect(editable.enableSuggestions, isFalse);
      expect(editable.autofillHints, isEmpty);
      expect(editable.keyboardType, TextInputType.url);
      expect(find.semantics.byLabel('Paste code'), findsOne);
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('cancelBackGesture');
      await pumpFrames(tester);
      expect(editable.controller.text, _receiveCode);
      expect(find.byType(MoveForm), findsOneWidget);
      tester.view.viewInsets = const FakeViewPadding(bottom: 240);
      await pumpFrames(tester);
      final hiddenBefore = tester.testTextInput.log
          .where((m) => m.method == 'TextInput.hide')
          .length;
      await _back(tester);
      expect(find.byType(MoveForm), findsOneWidget);
      expect(editable.controller.text, _receiveCode);
      expect(
        tester.testTextInput.log.where((m) => m.method == 'TextInput.hide'),
        hasLength(hiddenBefore + 1),
      );
      tester.view.viewInsets = const FakeViewPadding();
      await pumpFrames(tester);
      await _back(tester);
      expect(find.byType(MoveForm), findsNothing);
      expect(find.byType(SettingsView), findsOneWidget);
      expect(
        harness.backend.sent.where(
          (m) => m.hasStartMove() || m.hasCancelMove(),
        ),
        isEmpty,
      );
    },
    variant: android,
  );

  for (final surface in ['Move form', 'Extensions sheet']) {
    testWidgets(
      'Freeze from an open $surface removes overlays and releases Back eligibility',
      (tester) async {
        final platform = <MethodCall>[];
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          (call) async {
            platform.add(call);
            if (call.method == 'Clipboard.hasStrings') return {'value': false};
            return null;
          },
        );
        addTearDown(
          () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            SystemChannels.platform,
            null,
          ),
        );
        final harness = await showAndroid(tester);
        if (surface == 'Move form') {
          await _openMove(tester);
        } else {
          _tap(tester, 'More options for Mike');
          await pumpFrames(tester);
          expect(find.text("Mike's extensions"), findsOneWidget);
        }
        expect(
          platform
              .where(
                (m) => m.method == 'SystemNavigator.setFrameworkHandlesBack',
              )
              .last
              .arguments,
          isTrue,
        );
        expect(await _gesture('startBackGesture'), isTrue);
        _publishMove(harness, _move(MovePhase.MOVE_PHASE_FROZEN));
        await pumpFrames(tester);
        await _gesture('cancelBackGesture');
        await pumpFrames(tester);
        expect(find.byType(MovingScreen), findsOneWidget);
        expect(
          find.byType(PhoneSheet),
          findsNothing,
          reason: 'all old-window sheets must leave with their owner',
        );
        expect(
          find.byType(MoveForm),
          findsNothing,
          reason: 'no orphan sheet above the frozen root',
        );
        expect(find.text("Mike's extensions"), findsNothing);
        expect(find.byType(PhoneShell), findsNothing);
        expect(find.semantics.byLabel('Paste code'), findsNothing);
        expect(
          platform
              .where(
                (m) => m.method == 'SystemNavigator.setFrameworkHandlesBack',
              )
              .last
              .arguments,
          isFalse,
        );
        await _back(tester);
        expect(harness.backend.sent.where((m) => m.hasCancelMove()), isEmpty);
        expect(
          platform.where((m) => m.method == 'SystemNavigator.pop'),
          hasLength(1),
        );
      },
      variant: android,
    );
  }

  for (final size in [const Size(360, 640), const Size(640, 360)]) {
    for (final scale in [2.0, 3.0]) {
      final profile =
          '${size.width.toInt()}x${size.height.toInt()} at ${scale}x';

      testWidgets('Move setting, handshake and copy actions: $profile', (
        tester,
      ) async {
        final harness = await showAndroid(tester, size: size, textScale: scale);
        await _openMove(tester);
        final field = _inside(MoveForm, find.byType(EditableText));
        tester.view.viewInsets = FakeViewPadding(
          bottom: size.height < 400 ? 120 : 240,
        );
        await pumpFrames(tester);
        await _reveal(tester, field);
        final inputBox = find
            .ancestor(of: field, matching: find.byType(MergeSemantics))
            .first;
        final body = find
            .ancestor(of: field, matching: find.byType(SingleChildScrollView))
            .first;
        expect(
          tester.getRect(inputBox).intersect(tester.getRect(body)).height,
          greaterThanOrEqualTo(48),
          reason: 'at least 48dp of the input target must actually be visible',
        );
        await tester.enterText(field, _receiveCode);
        await pumpFrames(tester);
        await _reveal(tester, _inside(MoveForm, find.text('Connect')));
        _tap(tester, 'Connect');
        await pumpFrames(tester);
        final start = harness.backend.sent
            .where((m) => m.hasStartMove())
            .toList();
        expect(start, hasLength(1));
        expect(start.single.startMove.code, _receiveCode);
        tester.view.viewInsets = const FakeViewPadding();
        await pumpFrames(tester);
        _publishMove(harness, _move(MovePhase.MOVE_PHASE_CONFIRM));
        await pumpFrames(tester);
        expect(find.text('Do the digits match?'), findsOneWidget);
        expect(find.text('123 456'), findsOneWidget);
        await _reveal(tester, find.text('They match'));
        expect(await _gesture('startBackGesture'), isTrue);
        await _gesture('cancelBackGesture');
        await pumpFrames(tester);
        expect(harness.backend.sent.where((m) => m.hasConfirmMove()), isEmpty);
        _tap(tester, 'They match');
        await pumpFrames(tester);
        expect(
          harness.backend.sent.where((m) => m.hasConfirmMove()),
          hasLength(1),
        );
        _publishMove(harness, _move(MovePhase.MOVE_PHASE_COPYING));
        await pumpFrames(tester);
        expect(find.text('Copying to audit-target'), findsOneWidget);
        await _reveal(tester, find.text('Move now'));
        _tap(tester, 'Move now');
        await pumpFrames(tester);
        expect(
          harness.backend.sent.where((m) => m.hasFreezeMove()),
          hasLength(1),
        );
        await _back(tester);
        expect(find.byType(MoveForm), findsNothing);
        expect(
          harness.backend.sent.where((m) => m.hasCancelMove()),
          isEmpty,
          reason: 'Back closes presentation, not a running move',
        );
        expect(tester.takeException(), isNull);
      }, variant: android);

      testWidgets(
        'Frozen move replaces root; Back never cancels it: $profile',
        (tester) async {
          final harness = await showAndroid(
            tester,
            size: size,
            textScale: scale,
          );
          final pops = <MethodCall>[];
          tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            SystemChannels.platform,
            (call) async {
              if (call.method == 'SystemNavigator.pop') pops.add(call);
              return null;
            },
          );
          addTearDown(
            () => tester.binding.defaultBinaryMessenger
                .setMockMethodCallHandler(SystemChannels.platform, null),
          );
          _publishMove(harness, _move(MovePhase.MOVE_PHASE_FROZEN));
          await pumpFrames(tester);
          expect(find.byType(MovingScreen), findsOneWidget);
          expect(find.byType(PhoneShell), findsNothing);
          expect(
            find.byWidgetPredicate((w) => w is AndroidBackHost),
            findsNothing,
          );
          expect(find.text('Moving to audit-target…'), findsOneWidget);
          await _reveal(tester, find.text('Cancel move'));
          await _back(tester);
          expect(harness.backend.sent.where((m) => m.hasCancelMove()), isEmpty);
          expect(find.byType(MovingScreen), findsOneWidget);
          expect(
            pops,
            hasLength(1),
            reason: 'root/system fallback, not a cancel operation',
          );
          _tap(tester, 'Cancel move');
          await pumpFrames(tester);
          expect(
            harness.backend.sent.where((m) => m.hasCancelMove()),
            hasLength(1),
          );
          _publishMove(
            harness,
            _move(MovePhase.MOVE_PHASE_DONE, cancel: false),
          );
          await pumpFrames(tester);
          expect(find.text('Moved to audit-target'), findsOneWidget);
          expect(find.semantics.byLabel('Cancel move'), findsNothing);
          expect(tester.takeException(), isNull);
        },
        variant: android,
      );

      testWidgets(
        'Secret approval needs an explicit accessible decision: $profile',
        (tester) async {
          final harness = await _showSecret(tester, size: size, scale: scale);
          await _reveal(
            tester,
            _inside(SecretRequestCard, find.text('Let Rivet make it')),
          );
          expect(
            await _gesture('startBackGesture'),
            isFalse,
            reason: 'the inline card is not a navigation target',
          );
          await _gesture('cancelBackGesture');
          await pumpFrames(tester);
          expect(
            harness.backend.sent.where((m) => m.hasDecideSecretRequest()),
            isEmpty,
          );
          await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
          _tap(tester, 'Let Rivet make it');
          await pumpFrames(tester);
          final asks = harness.backend.sent
              .where((m) => m.hasDecideSecretRequest())
              .toList();
          expect(asks, hasLength(1));
          final decision = asks.single.decideSecretRequest;
          expect(decision.agentId, 'mike');
          expect(decision.requestId, 'audit-request');
          expect(decision.approve, isTrue);
          expect(decision.approveOnly, isTrue);
          expect(decision.entries, isEmpty);
          expect(tester.takeException(), isNull);
        },
        variant: android,
      );

      testWidgets(
        'Secret collection keeps obscured input and IME state: $profile',
        (tester) async {
          final harness = await _showSecret(
            tester,
            size: size,
            scale: scale,
            collect: true,
          );
          final field = _secretValue();
          await _reveal(tester, field);
          await tester.enterText(field, 'synthetic-audit-token');
          tester.view.viewInsets = FakeViewPadding(
            bottom: size.height < 400 ? 120 : 240,
          );
          await pumpFrames(tester);
          expect(tester.widget<EditableText>(field).obscureText, isTrue);
          final secret = find.semantics.byLabel('EXAMPLE_TOKEN');
          expect(secret, findsOne);
          expect(
            secret
                .evaluate()
                .single
                .getSemanticsData()
                .flagsCollection
                .isTextField,
            isTrue,
          );
          expect(
            secret
                .evaluate()
                .single
                .getSemanticsData()
                .flagsCollection
                .isObscured,
            isTrue,
          );
          expect(
            secret.evaluate().single.getSemanticsData().value,
            isNot(contains('synthetic-audit-token')),
          );
          final hides = _keyboardHides(tester);
          await _back(tester);
          expect(_keyboardHides(tester), hides + 1);
          expect(find.byType(SecretRequestCard), findsOneWidget);
          expect(
            tester.widget<EditableText>(field).controller.text,
            'synthetic-audit-token',
          );
          expect(
            harness.backend.sent.where((m) => m.hasDecideSecretRequest()),
            isEmpty,
          );
          tester.view.viewInsets = const FakeViewPadding();
          await pumpFrames(tester);
          await _reveal(tester, find.text('Add and give to Mike'));
          _tap(tester, 'Add and give to Mike');
          await pumpFrames(tester);
          final decisions = harness.backend.sent
              .where((m) => m.hasDecideSecretRequest())
              .toList();
          expect(decisions, hasLength(1));
          expect(decisions.single.decideSecretRequest.agentId, 'mike');
          expect(
            decisions.single.decideSecretRequest.requestId,
            'audit-request',
          );
          expect(decisions.single.decideSecretRequest.approve, isTrue);
          expect(decisions.single.decideSecretRequest.approveOnly, isFalse);
          expect(
            decisions.single.decideSecretRequest.entries.single.variable,
            'EXAMPLE_TOKEN',
          );
          expect(
            decisions.single.decideSecretRequest.entries.single.value,
            'synthetic-audit-token',
          );
          expect(tester.takeException(), isNull);
        },
        variant: android,
      );

      testWidgets('Chat rename IME and nested delete confirmation: $profile', (
        tester,
      ) async {
        final snapshot = workspace()..selectedChatId = 'audit-chat';
        snapshot.chats.add(
          Chat(
            id: 'audit-chat',
            title: 'Parity planning',
            modelAvailable: true,
          ),
        );
        final harness = await showAndroid(
          tester,
          snapshot: snapshot,
          size: size,
          textScale: scale,
        );
        expect(find.byType(ChatView), findsOneWidget);
        expect(
          find.semantics.byLabel('Attach files'),
          findsNothing,
          reason: 'chats are text-only, including on a phone',
        );
        expect(find.semantics.byLabel('Record a voice message'), findsNothing);
        _tap(tester, 'Parity planning');
        await pumpFrames(tester);
        expect(find.byType(FormFrame), findsOneWidget);
        expect(find.text('Rename chat'), findsWidgets);
        final field = _inside(FormFrame, find.byType(EditableText));
        await _reveal(tester, field);
        await tester.enterText(field, 'Renamed on Android');
        tester.view.viewInsets = FakeViewPadding(
          bottom: size.height < 400 ? 120 : 240,
        );
        await pumpFrames(tester);
        final hides = _keyboardHides(tester);
        await _back(tester);
        expect(_keyboardHides(tester), hides + 1);
        expect(find.byType(FormFrame), findsOneWidget);
        expect(
          tester.widget<EditableText>(field).controller.text,
          'Renamed on Android',
        );
        tester.view.viewInsets = const FakeViewPadding();
        await pumpFrames(tester);
        expect(await _gesture('startBackGesture'), isTrue);
        await _gesture('cancelBackGesture');
        await pumpFrames(tester);
        expect(
          tester.widget<EditableText>(field).controller.text,
          'Renamed on Android',
        );
        _tap(tester, 'Save');
        await pumpFrames(tester);
        final names = harness.backend.sent
            .where((m) => m.hasRenameChat())
            .toList();
        expect(names, hasLength(1));
        expect(names.single.renameChat.chatId, 'audit-chat');
        expect(names.single.renameChat.title, 'Renamed on Android');
        _tap(tester, 'More');
        await pumpFrames(tester);
        _tap(tester, 'Delete…');
        await pumpFrames(tester);
        expect(find.text('Delete Parity planning?'), findsOneWidget);
        await _back(tester);
        expect(harness.backend.sent.where((m) => m.hasDeleteChat()), isEmpty);
        expect(find.byType(ChatView), findsOneWidget);
        _tap(tester, 'More');
        await pumpFrames(tester);
        _tap(tester, 'Delete…');
        await pumpFrames(tester);
        _tap(tester, 'Delete');
        await pumpFrames(tester);
        final deletes = harness.backend.sent
            .where((m) => m.hasDeleteChat())
            .toList();
        expect(deletes, hasLength(1));
        expect(deletes.single.deleteChat.chatId, 'audit-chat');
        expect(tester.takeException(), isNull);
      }, variant: android);
    }
  }

  for (final size in [const Size(360, 640), const Size(640, 360)]) {
    testWidgets(
      'Phone chat explains model refusal without desktop drop UI: $size',
      (tester) async {
        final snapshot = workspace()..selectedChatId = 'blocked-chat';
        snapshot.models.clear();
        snapshot.chats.add(
          Chat(
            id: 'blocked-chat',
            title: 'No model chat',
            modelAvailable: false,
          ),
        );
        final harness = await showAndroid(
          tester,
          snapshot: snapshot,
          size: size,
          textScale: 3,
          withWindowDrop: true,
        );
        expect(find.byType(ChatView), findsOneWidget);
        expect(
          find.text('Connect a model so this chat can answer.'),
          findsOneWidget,
        );
        expect(_inside(ChatView, find.byType(EditableText)), findsNothing);
        expect(find.semantics.byLabel('Attach files'), findsNothing);
        expect(find.semantics.byLabel('Send'), findsNothing);
        final action = find.semantics.byLabel(RegExp('Connect a model…'));
        expect(action, findsOne);
        expect(
          action.evaluate().single.getSemanticsData().hasAction(
            SemanticsAction.tap,
          ),
          isTrue,
        );
        expect(action.evaluate().single.rect.width, greaterThanOrEqualTo(48));
        expect(action.evaluate().single.rect.height, greaterThanOrEqualTo(48));
        FileDrop.dragged(2);
        await pumpFrames(tester);
        expect(find.textContaining('Drop to add'), findsNothing);
        expect(find.textContaining("Files can't be added"), findsNothing);
        expect(
          harness.backend.sent.where(
            (m) => m.hasSendChatMessage() || m.hasStartUpload(),
          ),
          isEmpty,
        );
        expect(tester.takeException(), isNull);
      },
      variant: android,
    );
  }

  for (final size in [const Size(393, 852), const Size(900, 700)]) {
    testWidgets(
      'Mac drop overlay and native accept are absent on Android $size',
      (tester) async {
        final calls = <MethodCall>[];
        const window = MethodChannel('factory/window');
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
          call,
        ) async {
          calls.add(call);
          throw MissingPluginException('No Mac window in Android audit');
        });
        addTearDown(
          () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            window,
            null,
          ),
        );
        final harness = await showAndroid(
          tester,
          size: size,
          expectPhone: size.shortestSide < 600,
          withWindowDrop: true,
        );
        expect(find.byType(AgentView), findsOneWidget);
        FileDrop.dragged(
          2,
        ); // Stale/synthetic desktop signal must not enable UI.
        await pumpFrames(tester);
        expect(find.textContaining('Drop to add'), findsNothing);
        expect(find.textContaining('Files dropped here'), findsNothing);
        expect(
          calls,
          isEmpty,
          reason: 'never invoke desktop APIs, even if they would throw',
        );
        expect(
          harness.backend.sent.where(
            (m) => m.hasStartUpload() || m.hasSendMessage(),
          ),
          isEmpty,
        );
      },
      variant: android,
    );
  }

  testWidgets(
    'Android attach uses device picker and reports missing native binding',
    (tester) async {
      final windows = <MethodCall>[];
      final devices = <MethodCall>[];
      const window = MethodChannel('factory/window');
      const device = MethodChannel('factory/device');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        windows.add(call);
        throw MissingPluginException('No Mac window in Android audit');
      });
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(device, (
        call,
      ) async {
        devices.add(call);
        throw MissingPluginException(
          'Android picker not installed in Dart audit',
        );
      });
      addTearDown(() {
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        );
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          device,
          null,
        );
      });
      final harness = await showAndroid(tester);
      _tap(tester, 'Attach files');
      await pumpFrames(tester);
      expect(windows, isEmpty);
      expect(devices.where((m) => m.method == 'pickAttachments'), hasLength(1));
      expect(
        find.text("You can't attach files from this phone yet."),
        findsOneWidget,
      );
      expect(harness.backend.attachments.of('mike'), isEmpty);
      expect(harness.backend.sent.where((m) => m.hasStartUpload()), isEmpty);
    },
    variant: android,
  );
}
