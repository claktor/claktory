// Shared product surfaces on Android, with a recording backend/fake camera.
// Native Android/IME dispatch and permission/security acceptance remain owed.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/chats.dart' show ChatView;
import 'package:factory_app/controls.dart'
    show Button, GlyphButton, TextField, ResizeHandle;
import 'package:factory_app/identity.dart'
    show isPhoneLayout, touchFirst, deviceIdiom;
import 'package:factory_app/mobile/metrics.dart' show Metrics;
import 'package:factory_app/forms.dart' show FormFrame;
import 'package:factory_app/inbox.dart' show NeedsYouList;
import 'package:factory_app/mobile/needs_you_button.dart';
import 'package:factory_app/ipad/workbench.dart' show Workbench;
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/mobile/device.dart'
    show DeviceCapabilities, Idiom, debugDeviceCapabilities;
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/mobile/platform_policy.dart'
    show PhonePlatformPolicy;
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../pair_start.dart';
import '../phone_test.dart' show workspace;
import 'support/channel_fakes.dart';
import 'support/android_harness.dart';

const _size = Size(393, 852);

Finder _inAgent(Finder finder) =>
    find.descendant(of: find.byType(AgentView), matching: finder);
Finder _field() => _inAgent(find.byType(EditableText)).last;

Future<Harness> _show(
  WidgetTester tester, {
  Workspace? snapshot,
  Harness? harness,
  Size size = _size,
  bool expectPhone = true,
}) => showAndroid(
  tester,
  snapshot: snapshot,
  harness: harness,
  size: size,
  expectPhone: expectPhone,
);

Future<void> _button() async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
}

Future<Object?> _gesture(String method, {int edge = 0, double progress = 0}) =>
    sendNativeMethod(
      SystemChannels.backGesture,
      MethodCall(
        method,
        method == 'startBackGesture' || method == 'updateBackGestureProgress'
            ? {
                'touchOffset': [edge == 0 ? 1.0 : 392.0, 300.0],
                'progress': progress,
                'swipeEdge': edge,
              }
            : null,
      ),
    );

Future<void> _settings(WidgetTester tester) async {
  await tester.tap(_inAgent(find.bySemanticsLabel('Menu')));
  await pumpFrames(tester);
  final target = find.descendant(
    of: find.byType(Sidebar),
    matching: find.text('Settings'),
  );
  await tester.scrollUntilVisible(
    target,
    200,
    scrollable: find
        .descendant(of: find.byType(Sidebar), matching: find.byType(Scrollable))
        .first,
  );
  await tester.tap(target);
  await pumpFrames(tester);
  expect(find.byType(SettingsView), findsOneWidget);
  expect(find.byType(Sidebar), findsNothing);
}

void _noRemoteLayout(Harness harness) =>
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);

void _target48(WidgetTester tester, Finder finder, String label) {
  final size = tester.getSize(finder);
  expect(size.width, greaterThanOrEqualTo(48), reason: '$label width');
  expect(size.height, greaterThanOrEqualTo(48), reason: '$label height');
}

void main() {
  setUp(() => PhonePlatformPolicy.debugAndroidPolicy = true);
  tearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
  final android = TargetPlatformVariant.only(TargetPlatform.android);

  testWidgets('Menu opens the real drawer; Android edges never open it', (
    tester,
  ) async {
    final harness = await _show(tester);
    expect(
      find.byWidgetPredicate((widget) => widget is AndroidBackHost),
      findsOneWidget,
    );
    for (final edge in [1.0, 392.0]) {
      await tester.dragFrom(
        Offset(edge, 420),
        Offset(edge < 100 ? 300 : -300, 0),
      );
      await pumpFrames(tester);
      expect(find.byType(Sidebar), findsNothing);
    }
    await tester.tap(_inAgent(find.bySemanticsLabel('Menu')));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    expect(await _gesture('startBackGesture'), isTrue);
    await _gesture('cancelBackGesture');
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    await _button();
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);
    expect(find.byType(AgentView), findsOneWidget);
    _noRemoteLayout(harness);
  }, variant: android);

  for (final edge in [0, 1]) {
    testWidgets(
      'Settings reached through drawer survives edge $edge cancel with draft',
      (tester) async {
        final harness = await _show(tester);
        await tester.enterText(_field(), 'Unsent 日本語 👩🏽‍💻');
        await _settings(tester);
        final route = ModalRoute.of(tester.element(find.byType(SettingsView)))!;
        expect(
          PhoneRoutes.of(tester.element(find.byType(SettingsView)))!.canPop,
          isTrue,
        );
        expect(await _gesture('startBackGesture', edge: edge), isTrue);
        await _gesture('updateBackGestureProgress', edge: edge, progress: 0.6);
        await tester.pump();
        await _gesture('cancelBackGesture');
        await pumpFrames(tester);
        expect(find.byType(SettingsView), findsOneWidget);
        expect(route.isCurrent, isTrue);
        expect(route.animation!.value, 1);
        expect(harness.drafts.of('mike'), 'Unsent 日本語 👩🏽‍💻');
        await _button();
        await pumpFrames(tester);
        expect(find.byType(SettingsView), findsNothing);
        expect(
          tester.widget<EditableText>(_field()).controller.text,
          'Unsent 日本語 👩🏽‍💻',
        );
        _noRemoteLayout(harness);
      },
      variant: android,
    );
  }

  testWidgets('one IME-owned framework gesture cannot dismiss Settings too', (
    tester,
  ) async {
    final harness = await _show(tester);
    await tester.enterText(_field(), 'Retain my draft');
    await _settings(tester);
    tester.view.viewInsets = const FakeViewPadding(bottom: 300);
    await tester.pump();
    tester.testTextInput.log.clear();
    expect(await _gesture('startBackGesture'), isTrue);
    tester.view.viewInsets = const FakeViewPadding();
    await tester.pump();
    await _gesture('commitBackGesture');
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(
      tester.testTextInput.log.where((c) => c.method == 'TextInput.hide'),
      hasLength(1),
    );
    expect(harness.drafts.of('mike'), 'Retain my draft');
    await _button();
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsNothing);
    _noRemoteLayout(harness);
  }, variant: android);

  testWidgets(
    'Extensions sheet cancels intact, then Back closes just that sheet',
    (tester) async {
      final harness = await _show(tester);
      await tester.tap(_inAgent(find.bySemanticsLabel('Extensions')));
      await pumpFrames(tester);
      expect(find.text("Mike's extensions"), findsOneWidget);
      expect(find.byType(PhoneSheet), findsOneWidget);
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('cancelBackGesture');
      await pumpFrames(tester);
      expect(find.text("Mike's extensions"), findsOneWidget);
      await _button();
      await pumpFrames(tester);
      expect(find.text("Mike's extensions"), findsNothing);
      expect(find.byType(AgentView), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasOpenPanel()), isEmpty);
      _noRemoteLayout(harness);
    },
    variant: android,
  );

  testWidgets(
    'dirty Agent settings cancels intact and asks before discarding',
    (tester) async {
      final harness = await _show(tester);
      await tester.tap(_inAgent(find.text('Mike')));
      await pumpFrames(tester);
      expect(find.byType(FormFrame), findsOneWidget);
      expect(find.text('Agent settings'), findsWidgets);
      final field = find
          .descendant(
            of: find.byType(FormFrame),
            matching: find.byType(EditableText),
          )
          .first;
      await tester.enterText(field, 'Edited Mike');
      await tester.pump();
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('updateBackGestureProgress', progress: 0.5);
      await _gesture('cancelBackGesture');
      await pumpFrames(tester);
      expect(tester.widget<EditableText>(field).controller.text, 'Edited Mike');
      expect(find.text('Discard changes'), findsNothing);
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('commitBackGesture');
      await pumpFrames(tester);
      expect(find.text('Discard changes'), findsOneWidget);
      expect(find.text('Keep editing'), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasUpdateAgent()), isEmpty);
      await tester.tap(find.text('Keep editing'));
      await pumpFrames(tester);
      expect(find.byType(FormFrame), findsOneWidget);
      expect(tester.widget<EditableText>(field).controller.text, 'Edited Mike');
      expect(harness.backend.workspace.workspace.agents.last.name, 'Mike');
      _noRemoteLayout(harness);
    },
    variant: android,
  );

  testWidgets('selection action surface consumes Back before root navigation', (
    tester,
  ) async {
    final harness = await _show(tester);
    await tester.enterText(_field(), 'Selected words');
    await tester.pump();
    final state = tester.state<EditableTextState>(_field());
    state.widget.controller.selection = const TextSelection(
      baseOffset: 0,
      extentOffset: 8,
    );
    state.showToolbar();
    await pumpFrames(tester);
    expect(find.text('Copy'), findsOneWidget);
    await _button();
    await pumpFrames(tester);
    expect(find.text('Copy'), findsNothing);
    expect(state.widget.controller.text, 'Selected words');
    expect(find.byType(AgentView), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
  }, variant: android);

  testWidgets(
    'read-only transcript selection has Android actions and consumes Back',
    (tester) async {
      final harness = Harness(workspace()..selectedAgentId = 'mike');
      harness.backend.transcript.load(
        Transcript(
          agentId: 'mike',
          entries: [
            Entry(
              seq: Int64(1),
              assistant: AssistantText(text: 'A reply to select'),
            ),
          ],
        ),
      );
      await _show(tester, harness: harness);
      expect(find.byType(TranscriptView), findsOneWidget);
      final regions = find.descendant(
        of: find.byType(TranscriptView),
        matching: find.byType(SelectableRegion),
      );
      expect(regions, findsWidgets);
      tester
          .state<SelectableRegionState>(regions.first)
          .selectAll(SelectionChangedCause.toolbar);
      await pumpFrames(tester);
      expect(find.text('Copy'), findsOneWidget);
      await _button();
      await pumpFrames(tester);
      expect(find.text('Copy'), findsNothing);
      expect(find.byType(TranscriptView), findsOneWidget);
      expect(
        harness.backend.transcript[0].entry.assistant.text,
        'A reply to select',
      );
      _noRemoteLayout(harness);
    },
    variant: android,
  );

  testWidgets(
    'hardware send shortcut cannot submit an active IME composition',
    (tester) async {
      final harness = await _show(tester);
      await tester.showKeyboard(_field());
      tester.testTextInput.updateEditingValue(
        const TextEditingValue(
          text: '日本',
          selection: TextSelection.collapsed(offset: 2),
          composing: TextRange(start: 0, end: 2),
        ),
      );
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await pumpFrames(tester);
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      expect(
        tester.widget<EditableText>(_field()).controller.value.composing,
        const TextRange(start: 0, end: 2),
      );
    },
    variant: android,
  );

  testWidgets(
    'Android composer selects, copies and pastes without sending composing text',
    (tester) async {
      var clipboard = 'pasted 👩🏽‍💻';
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async => switch (call.method) {
          'Clipboard.getData' => {'text': clipboard},
          'Clipboard.hasStrings' => {'value': clipboard.isNotEmpty},
          'Clipboard.setData' =>
            clipboard = (call.arguments as Map)['text'] as String,
          _ => null,
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      final harness = await _show(tester);
      await tester.showKeyboard(_field());
      tester.testTextInput.updateEditingValue(
        const TextEditingValue(
          text: '日本',
          selection: TextSelection.collapsed(offset: 2),
          composing: TextRange(start: 0, end: 2),
        ),
      );
      await tester.pump();
      final state = tester.state<EditableTextState>(_field());
      expect(state.widget.selectionControls, isNotNull);
      expect(state.widget.contextMenuBuilder, isNotNull);
      expect(
        state.widget.controller.value.composing,
        const TextRange(start: 0, end: 2),
      );
      await tester.testTextInput.receiveAction(TextInputAction.newline);
      await tester.pump();
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      tester.testTextInput.updateEditingValue(
        const TextEditingValue(
          text: '日本\n👩🏽‍💻',
          selection: TextSelection(baseOffset: 0, extentOffset: 2),
        ),
      );
      state.showToolbar();
      await pumpFrames(tester);
      expect(find.text('Copy'), findsOneWidget);
      await tester.tap(find.text('Copy'));
      await pumpFrames(tester);
      expect(clipboard, '日本');
      clipboard = ' pasted';
      state.widget.controller.selection = TextSelection.collapsed(
        offset: state.widget.controller.text.length,
      );
      state.showToolbar();
      await pumpFrames(tester);
      await tester.tap(find.text('Paste'));
      await pumpFrames(tester);
      expect(state.widget.controller.text, '日本\n👩🏽‍💻 pasted');
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      await tester.tap(find.byType(SendButton));
      await pumpFrames(tester);
      final sends = harness.backend.sent
          .where((m) => m.hasSendMessage())
          .toList();
      expect(sends, hasLength(1));
      expect(sends.single.sendMessage.agentId, 'mike');
      expect(sends.single.sendMessage.text, '日本\n👩🏽‍💻 pasted');
    },
    variant: android,
  );

  testWidgets('core Android actions have 48dp hit regions', (tester) async {
    await _show(tester);
    for (final label in ['Menu', 'Extensions', 'Attach files']) {
      _target48(tester, _inAgent(find.bySemanticsLabel(label)).first, label);
    }
    _target48(tester, find.byType(SendButton), 'Send');
  }, variant: android);

  testWidgets(
    'Android capability facts do not select the phone composition by themselves',
    (tester) async {
      final previous = debugDeviceCapabilities;
      addTearDown(() => debugDeviceCapabilities = previous);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      for (final idiom in [Idiom.phone, Idiom.pad]) {
        debugDeviceCapabilities = DeviceCapabilities(idiom: idiom);
        for (final size in [const Size(360, 800), const Size(900, 700)]) {
          tester.view.physicalSize = size;
          late BuildContext context;
          await tester.pumpWidget(
            WidgetsApp(
              color: const Color(0xff202020),
              builder: (found, _) {
                context = found;
                return const SizedBox.expand();
              },
            ),
          );
          expect(
            deviceIdiom(context),
            idiom,
            reason: 'capabilities stay truthful',
          );
          expect(
            isPhoneLayout(context),
            size.shortestSide < 600,
            reason: 'Android layout is width-driven, not the iOS pad rule',
          );
          expect(touchFirst(context), isTrue);
          expect(Metrics.of(context).target, 48);
        }
      }
    },
    variant: android,
  );

  testWidgets(
    'truthful Android pad keeps the shared wide Shell, not the iPad workbench',
    (tester) async {
      final previous = debugDeviceCapabilities;
      debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
      addTearDown(() => debugDeviceCapabilities = previous);
      final harness = await _show(
        tester,
        size: const Size(900, 700),
        expectPhone: false,
      );
      expect(find.byType(Shell), findsOneWidget);
      expect(find.byType(AgentView), findsOneWidget);
      expect(find.byType(Sidebar), findsOneWidget);
      expect(find.byType(Workbench), findsNothing);
      expect(deviceIdiom(tester.element(find.byType(Shell))), Idiom.pad);
      _noRemoteLayout(harness);
    },
    variant: android,
  );

  for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
    testWidgets(
      'wide $platform control density is independent of phone composition',
      (tester) async {
        final controller = TextEditingController(text: 'A field');
        addTearDown(controller.dispose);
        tester.view.physicalSize = const Size(900, 700);
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        late BuildContext context;
        await tester.pumpWidget(
          WidgetsApp(
            color: const Color(0xff202020),
            localizationsDelegates: PhonePlatformPolicy.current.localizations,
            builder: (_, _) => Overlay(
              initialEntries: [
                OverlayEntry(
                  builder: (found) {
                    context = found;
                    return Center(
                      child: SizedBox(
                        width: 500,
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Button(label: 'Continue', onPressed: () {}),
                            TextField(
                              controller: controller,
                              placeholder: 'A field',
                            ),
                            GlyphButton(
                              label: 'Wide action',
                              active: false,
                              onTap: () {},
                              glyph: (_) =>
                                  const SizedBox.square(dimension: 12),
                            ),
                          ],
                        ),
                      ),
                    );
                  },
                ),
              ],
            ),
          ),
        );
        await tester.pump();
        final android = platform == TargetPlatform.android;
        expect(isPhoneLayout(context), isFalse);
        expect(touchFirst(context), android);
        expect(Metrics.of(context).target, android ? 48 : 28);
        expect(
          tester.getSize(find.bySemanticsLabel('Wide action')),
          Size.square(android ? 48 : 28),
        );
        expect(
          tester.getSize(find.byType(Button)).height,
          greaterThanOrEqualTo(android ? 48 : 28),
        );
        expect(ResizeHandle.widthOf(context), android ? 48 : 8);
        if (android) {
          expect(
            tester.getSize(find.byType(TextField)).height,
            greaterThanOrEqualTo(48),
          );
        }
      },
      variant: TargetPlatformVariant.only(platform),
    );
  }

  testWidgets('page owns cutout/IME insets exactly once through resize', (
    tester,
  ) async {
    tester.view.padding = const FakeViewPadding(
      left: 20,
      right: 12,
      top: 24,
      bottom: 24,
    );
    final harness = await _show(tester);
    await tester.enterText(_field(), 'one\ntwo\nthree\nfour\nfive\nsix');
    final controller = tester.widget<EditableText>(_field()).controller;
    for (final (size, keyboard) in [
      (const Size(360, 640), 240.0),
      (const Size(640, 360), 120.0),
      (const Size(412, 915), 300.0),
    ]) {
      tester.view.physicalSize = size;
      tester.view.viewInsets = FakeViewPadding(bottom: keyboard);
      await pumpFrames(tester);
      expect(
        tester.widget<EditableText>(_field()).controller,
        same(controller),
      );
      expect(
        tester.getBottomRight(find.byType(SendButton)).dy,
        lessThanOrEqualTo(size.height - keyboard),
      );
      expect(
        tester.getBottomRight(_field()).dy,
        lessThanOrEqualTo(size.height - keyboard),
      );
      expect(MediaQuery.viewInsetsOf(tester.element(_field())).bottom, 0);
      expect(MediaQuery.paddingOf(tester.element(_field())), EdgeInsets.zero);
      expect(harness.drafts.of('mike'), 'one\ntwo\nthree\nfour\nfive\nsix');
    }
    _noRemoteLayout(harness);
  }, variant: android);

  testWidgets(
    'Needs you reaches an approval; Back never answers it, explicit Approve does',
    (tester) async {
      final snapshot = workspace()..selectedAgentId = 'mike';
      snapshot.agents.last.waitingSeqs.add(Int64(7));
      final harness = await _show(tester, snapshot: snapshot);
      harness.backend.entriesById['mike/7'] = Entry(
        seq: Int64(7),
        timestampMs: Int64(1000),
        askPerson: AskPerson(
          id: 'ask7',
          title: 'Deploy staging?',
          wantsText: false,
          state: AskPersonState.ASK_PERSON_STATE_PENDING,
        ),
      );
      _target48(tester, find.byType(PhoneNeedsYouButton), 'Needs you');
      await tester.tap(find.byType(PhoneNeedsYouButton));
      await pumpFrames(tester);
      expect(find.byType(NeedsYouList), findsOneWidget);
      expect(find.text('Deploy staging?'), findsOneWidget);
      await tester.tap(find.text('Deploy staging?'));
      await pumpFrames(tester);
      expect(find.byType(AskPersonCard), findsOneWidget);
      expect(find.text('Mike needs your answer'), findsOneWidget);
      final approve = find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.text('Approve'),
      );
      await tester.ensureVisible(approve);
      _target48(
        tester,
        find.ancestor(of: approve, matching: find.byType(Button)).first,
        'Approve',
      );
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('cancelBackGesture');
      await pumpFrames(tester);
      expect(find.byType(AskPersonCard), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
      await tester.tap(approve);
      await pumpFrames(tester);
      final answers = harness.backend.sent
          .where((m) => m.hasAnswerAsk())
          .toList();
      expect(answers, hasLength(1));
      expect(answers.single.answerAsk.agentId, 'mike');
      expect(answers.single.answerAsk.seq.toInt(), 7);
      expect(answers.single.answerAsk.dismiss, isFalse);
      _noRemoteLayout(harness);
    },
    variant: android,
  );

  testWidgets(
    'chat is a real root; Send is pinned to its chat, not selected agent',
    (tester) async {
      final snapshot = workspace()..selectedChatId = 'chat1';
      snapshot.chats.add(
        Chat(id: 'chat1', title: 'Planning', modelAvailable: true),
      );
      final harness = await _show(tester, snapshot: snapshot);
      expect(find.byType(ChatView), findsOneWidget);
      expect(find.byType(AgentView), findsNothing);
      final field = find
          .descendant(
            of: find.byType(ChatView),
            matching: find.byType(EditableText),
          )
          .last;
      await tester.enterText(field, 'A chat message');
      await tester.pump();
      await tester.tap(find.byType(SendButton));
      await pumpFrames(tester);
      final sends = harness.backend.sent
          .where((m) => m.hasSendChatMessage())
          .toList();
      expect(sends, hasLength(1));
      expect(sends.single.sendChatMessage.chatId, 'chat1');
      expect(sends.single.sendChatMessage.text, 'A chat message');
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      expect(
        PhoneRoutes.of(tester.element(find.byType(ChatView)))!.canPop,
        isFalse,
      );
    },
    variant: android,
  );

  testWidgets(
    'fake-camera onboarding reaches Paste and connects parsed code without native scanner',
    (tester) async {
      final harness = await _show(
        tester,
        harness: Harness(Workspace(), localBackend: false),
      );
      expect(find.byType(PairScreen), findsOneWidget);
      // First, what's needed and the ways in (#363).
      await pastPairStart(tester, paste: true);
      expect(find.text('Paste code'), findsOneWidget);
      final code = 'factory://?t=${'a' * 64}&e=${'c' * 64}&name=Fixture';
      await tester.enterText(find.byType(EditableText), code);
      await tester.pump();
      await tester.tap(find.text('Connect'));
      await pumpFrames(tester);
      // A code asks first (confirmPairing).
      await tester.tap(find.text('Connect').last);
      await pumpFrames(tester);
      expect(harness.connections.current?.code, code);
      expect(harness.backend.target?.endpointId, 'c' * 64);
      expect(harness.camera.asked, 0);
    },
    variant: android,
  );
}
