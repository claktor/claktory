// Framework semantics and linear text-scale stress, not live TalkBack proof.
import 'dart:io';
import 'dart:ui' show Tristate;

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/chats.dart' show ChatView;
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:factory_app/forms.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/mobile/needs_you_button.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../pair_start.dart';
import '../phone_test.dart' show workspace;
import 'support/android_harness.dart';
import 'support/channel_fakes.dart';

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

void _action(WidgetTester tester, String label) {
  final finder = find.semantics.byLabel(label);
  expect(finder, findsOne);
  final node = finder.evaluate().single;
  expect(node.isInvisible, isFalse, reason: label);
  expect(
    node.getSemanticsData().flagsCollection.isHidden,
    isFalse,
    reason: label,
  );
  expect(
    node.getSemanticsData().hasAction(SemanticsAction.tap),
    isTrue,
    reason: '$label: $node',
  );
  expect(node.rect.width, greaterThanOrEqualTo(48), reason: '$label width');
  expect(node.rect.height, greaterThanOrEqualTo(48), reason: '$label height');
  tester.semantics.performAction(finder, SemanticsAction.tap);
}

Future<void> _back(WidgetTester tester) async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
  await pumpFrames(tester);
}

Future<void> _settings(WidgetTester tester) async {
  _action(tester, 'Menu');
  await pumpFrames(tester);
  final target = find.descendant(
    of: find.byType(Sidebar),
    matching: find.text('Settings'),
  );
  await tester.scrollUntilVisible(
    target,
    160,
    scrollable: find
        .descendant(of: find.byType(Sidebar), matching: find.byType(Scrollable))
        .first,
  );
  // Its whole row in the middle, not at the edge a step of the scroll
  // stopped at.
  await Scrollable.ensureVisible(tester.element(target), alignment: 0.5);
  await pumpFrames(tester);
  _action(tester, 'Settings');
  await pumpFrames(tester);
  expect(find.byType(SettingsView), findsOneWidget);
}

Future<Harness> _approval(
  WidgetTester tester, {
  double? scale,
  Size size = const Size(393, 852),
}) async {
  final snapshot = workspace()..selectedAgentId = 'mike';
  snapshot.agents.last.waitingSeqs.add(Int64(7));
  final harness = await showAndroid(
    tester,
    snapshot: snapshot,
    textScale: scale,
    size: size,
  );
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
  await tester.tap(find.byType(PhoneNeedsYouButton));
  await pumpFrames(tester);
  expect(find.byType(NeedsYouList), findsOneWidget);
  await tester.tap(find.text('Deploy staging?'));
  await pumpFrames(tester);
  expect(find.byType(AskPersonCard), findsOneWidget);
  return harness;
}

// The header's title reads its name, role and model; the model by its id,
// the test's Workspace listing no models.
const _model = 'anthropic/claude-opus-5-5';
const _mike = 'Mike, Coordinator, $_model';

void main() {
  setUpAll(_fonts);
  setUp(() => PhonePlatformPolicy.debugAndroidPolicy = true);
  tearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
  final android = TargetPlatformVariant.only(TargetPlatform.android);

  testWidgets(
    'conversation has ordered independently actionable labeled controls',
    (tester) async {
      final harness = await showAndroid(tester);
      final field = find
          .descendant(
            of: find.byType(AgentView),
            matching: find.byType(EditableText),
          )
          .last;
      await tester.enterText(field, 'An accessible message');
      await tester.pump();
      final labels = tester.semantics
          .simulatedAccessibilityTraversal()
          .map((node) => node.getSemanticsData().label)
          .toList();
      expect(
        labels,
        containsAllInOrder([
          'Menu',
          _mike,
          'More options for Mike',
          'Attach files',
          'Message Mike',
          'Send',
        ]),
      );
      final title = find.semantics
          .byLabel(_mike)
          .evaluate()
          .single
          .getSemanticsData();
      expect(title.hint, 'Opens agent settings');
      expect(title.hasAction(SemanticsAction.tap), isTrue);
      await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      _action(tester, 'Send');
      await pumpFrames(tester);
      final sends = harness.backend.sent
          .where((m) => m.hasSendMessage())
          .toList();
      expect(sends, hasLength(1));
      expect(sends.single.sendMessage.text, 'An accessible message');
    },
    variant: android,
  );

  testWidgets('drawer semantics exclude conversation then restore it', (
    tester,
  ) async {
    await showAndroid(tester);
    _action(tester, 'Menu');
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    expect(find.semantics.byLabel('More options for Mike'), findsNothing);
    expect(find.semantics.byLabel(_mike), findsNothing);
    await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
    await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
    _action(tester, 'Search');
    await pumpFrames(tester);
    tester.semantics.performAction(
      find.semantics.byLabel('Search'),
      SemanticsAction.setText,
      args: 'Mike',
    );
    await pumpFrames(tester);
    expect(find.byType(SidebarSearch), findsOneWidget);
    expect(
      find.semantics
          .byLabel('Search')
          .evaluate()
          .single
          .getSemanticsData()
          .value,
      'Mike',
    );
    _action(tester, 'Clear search');
    await pumpFrames(tester);
    expect(find.byType(SidebarSearch), findsNothing);
    expect(
      find.semantics
          .byLabel('Search')
          .evaluate()
          .single
          .getSemanticsData()
          .value,
      isEmpty,
    );
    await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
    _action(tester, 'Close menu');
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);
    expect(find.semantics.byLabel('More options for Mike'), findsOne);
  }, variant: android);

  testWidgets(
    'sheet route blocks underlying semantics and restores editable focus',
    (tester) async {
      final harness = await showAndroid(tester);
      final field = find
          .descendant(
            of: find.byType(AgentView),
            matching: find.byType(EditableText),
          )
          .last;
      await tester.enterText(field, 'Retain focus and draft');
      final editable = tester.widget<EditableText>(field);
      expect(editable.focusNode.hasFocus, isTrue);
      _action(tester, 'More options for Mike');
      await pumpFrames(tester);
      expect(find.text("Mike's extensions"), findsOneWidget);
      expect(find.semantics.byLabel(_mike), findsNothing);
      expect(editable.focusNode.hasFocus, isFalse);
      final route = find.semantics.byLabel("Mike's extensions").evaluate();
      expect(route, isNotEmpty);
      expect(
        route.any((node) => node.getSemanticsData().flagsCollection.namesRoute),
        isTrue,
      );
      await _back(tester);
      expect(find.semantics.byLabel(_mike), findsOne);
      expect(editable.focusNode.hasFocus, isTrue);
      expect(harness.drafts.of('mike'), 'Retain focus and draft');
    },
    variant: android,
  );

  testWidgets('selection exposes Copy action without sending the draft', (
    tester,
  ) async {
    var clipboard = '';
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
    final harness = await showAndroid(tester);
    final field = find
        .descendant(
          of: find.byType(AgentView),
          matching: find.byType(EditableText),
        )
        .last;
    await tester.enterText(field, 'Selected words');
    final state = tester.state<EditableTextState>(field);
    state.widget.controller.selection = const TextSelection(
      baseOffset: 0,
      extentOffset: 8,
    );
    state.showToolbar();
    await pumpFrames(tester);
    tester.semantics.performAction(
      find.semantics.byLabel('Copy'),
      SemanticsAction.tap,
    );
    await pumpFrames(tester);
    expect(clipboard, 'Selected');
    expect(harness.drafts.of('mike'), 'Selected words');
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
  }, variant: android);

  testWidgets('approval requires explicit semantic action and answers once', (
    tester,
  ) async {
    final harness = await _approval(tester);
    await tester.ensureVisible(find.text('Approve'));
    await tester.pump();
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    expect(
      find.semantics
          .byLabel('Approve')
          .evaluate()
          .single
          .getSemanticsData()
          .flagsCollection
          .isButton,
      isTrue,
    );
    _action(tester, 'Approve');
    await pumpFrames(tester);
    final answers = harness.backend.sent
        .where((m) => m.hasAnswerAsk())
        .toList();
    expect(answers, hasLength(1));
    expect(answers.single.answerAsk.seq, Int64(7));
    expect(answers.single.answerAsk.dismiss, isFalse);
  }, variant: android);

  testWidgets(
    'large text header keeps full name and role in one actionable label',
    (tester) async {
      const name = 'Bartholomew-Alexander Montgomery';
      const role = 'Principal infrastructure and release engineer';
      final snapshot = workspace()..selectedAgentId = 'mike';
      snapshot.agents.last
        ..name = name
        ..role = role;
      await showAndroid(
        tester,
        snapshot: snapshot,
        textScale: 3,
        size: const Size(360, 640),
      );
      final data = find.semantics
          .byLabel('$name, $role, $_model')
          .evaluate()
          .single
          .getSemanticsData();
      expect(data.hint, 'Opens agent settings');
      expect(data.flagsCollection.isButton, isTrue);
      _action(tester, '$name, $role, $_model');
      await pumpFrames(tester);
      expect(find.byType(FormFrame), findsOneWidget);
      expect(find.text('Agent settings'), findsWidgets);
      expect(tester.takeException(), isNull);
    },
    variant: android,
  );

  testWidgets(
    'composer retains one named editable node before and after typing',
    (tester) async {
      final snapshot = workspace()..selectedAgentId = 'mike';
      snapshot.agents.last.placeholder =
          'What should I build?\nAsk me anything';
      final harness = await showAndroid(tester, snapshot: snapshot);
      expect(find.semantics.byLabel('What should I build?'), findsNothing);
      final field = find
          .descendant(
            of: find.byType(AgentView),
            matching: find.byType(EditableText),
          )
          .last;
      final focus = tester.widget<EditableText>(field).focusNode;
      final named = find.semantics.byLabel('Message Mike');
      expect(named, findsOne);
      expect(named.evaluate().single.getSemanticsData().value, isEmpty);
      tester.semantics.performAction(named, SemanticsAction.tap);
      await pumpFrames(tester);
      expect(focus.hasFocus, isTrue);
      for (final value in ['', 'A draft with a persistent label']) {
        if (value.isNotEmpty) {
          tester.semantics.performAction(
            named,
            SemanticsAction.setText,
            args: value,
          );
          await tester.pump();
        }
        expect(named, findsOne);
        final data = named.evaluate().single.getSemanticsData();
        expect(data.flagsCollection.isTextField, isTrue);
        expect(data.flagsCollection.isFocused, Tristate.isTrue);
        expect(data.value, value);
        expect(data.hasAction(SemanticsAction.setText), isTrue);
        expect(data.hasAction(SemanticsAction.setSelection), isTrue);
      }
      tester.semantics.performAction(
        named,
        SemanticsAction.setSelection,
        args: {'base': 0, 'extent': 7},
      );
      await tester.pump();
      expect(
        named.evaluate().single.getSemanticsData().textSelection,
        const TextSelection(baseOffset: 0, extentOffset: 7),
      );
      expect(harness.drafts.of('mike'), 'A draft with a persistent label');
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
    },
    variant: android,
  );

  testWidgets('manual Connect announces button role and disabled state', (
    tester,
  ) async {
    final harness = await showAndroid(
      tester,
      harness: Harness(Workspace(), localBackend: false),
    );
    await pastPairStart(tester, paste: true);
    var data = find.semantics
        .byLabel('Connect')
        .evaluate()
        .single
        .getSemanticsData();
    expect(data.flagsCollection.isButton, isTrue);
    expect(data.flagsCollection.isEnabled, Tristate.isFalse);
    expect(data.hasAction(SemanticsAction.tap), isFalse);
    expect(harness.connections.current, isNull);
    final code = 'factory://?t=${'a' * 64}&e=${'c' * 64}&name=Fixture';
    await tester.enterText(find.byType(EditableText), code);
    await tester.pump();
    data = find.semantics
        .byLabel('Connect')
        .evaluate()
        .single
        .getSemanticsData();
    expect(data.flagsCollection.isEnabled, Tristate.isTrue);
    _action(tester, 'Connect');
    await pumpFrames(tester);
    // A code asks first (confirmPairing): its Connect, announced alone.
    _action(tester, 'Connect');
    await pumpFrames(tester);
    expect(harness.connections.current?.code, code);
  }, variant: android);

  testWidgets('read-only selection exposes semantic Copy without mutation', (
    tester,
  ) async {
    var clipboard = '';
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          clipboard = (call.arguments as Map)['text'] as String;
        }
        if (call.method == 'Clipboard.hasStrings') {
          return {'value': clipboard.isNotEmpty};
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
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
    await showAndroid(tester, harness: harness);
    final regions = find.descendant(
      of: find.byType(TranscriptView),
      matching: find.byType(SelectableRegion),
    );
    tester
        .state<SelectableRegionState>(regions.first)
        .selectAll(SelectionChangedCause.toolbar);
    await pumpFrames(tester);
    tester.semantics.performAction(
      find.semantics.byLabel('Copy'),
      SemanticsAction.tap,
    );
    await pumpFrames(tester);
    expect(clipboard, 'A reply to select');
    expect(
      harness.backend.transcript[0].entry.assistant.text,
      'A reply to select',
    );
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
  }, variant: android);

  for (final scale in [2.0, 3.0]) {
    for (final size in [const Size(360, 640), const Size(640, 360)]) {
      final profile =
          '${size.width.toInt()}x${size.height.toInt()} at ${scale}x';
      testWidgets('large text settings and form remain reachable: $profile', (
        tester,
      ) async {
        await showAndroid(tester, textScale: scale, size: size);
        expect(
          MediaQuery.textScalerOf(tester.element(find.byType(AgentView)))
              .scale(16),
          16 * scale,
        );
        await _settings(tester);
        expect(find.semantics.byLabel('More options for Mike'), findsNothing);
        _action(tester, 'Back');
        await pumpFrames(tester);
        _action(tester, _mike);
        await pumpFrames(tester);
        expect(find.byType(FormFrame), findsOneWidget);
        final field = find
            .descendant(
              of: find.byType(FormFrame),
              matching: find.byType(EditableText),
            )
            .first;
        await tester.ensureVisible(field);
        await tester.enterText(field, 'Accessible Mike');
        await tester.pump();
        await _back(tester);
        expect(find.text('Keep editing'), findsOneWidget);
        _action(tester, 'Keep editing');
        await pumpFrames(tester);
        expect(
          tester.widget<EditableText>(field).controller.text,
          'Accessible Mike',
        );
        expect(tester.takeException(), isNull);
      }, variant: android);

      testWidgets(
        'large text chat composer clears keyboard and sends once: $profile',
        (tester) async {
          final snapshot = workspace()..selectedChatId = 'chat1';
          snapshot.chats.add(
            Chat(id: 'chat1', title: 'Planning', modelAvailable: true),
          );
          final harness = await showAndroid(
            tester,
            snapshot: snapshot,
            textScale: scale,
            size: size,
          );
          expect(find.byType(ChatView), findsOneWidget);
          tester.view.viewInsets = FakeViewPadding(
            bottom: size.height < 400 ? 120 : 240,
          );
          final field = find
              .descendant(
                of: find.byType(ChatView),
                matching: find.byType(EditableText),
              )
              .last;
          await tester.enterText(field, 'A large-text chat message');
          await pumpFrames(tester);
          final bottom = size.height - tester.view.viewInsets.bottom;
          expect(tester.getBottomRight(field).dy, lessThanOrEqualTo(bottom));
          expect(
            tester.getBottomRight(find.byType(SendButton)).dy,
            lessThanOrEqualTo(bottom),
          );
          final input = find.semantics
              .byLabel('Ask anything')
              .evaluate()
              .single
              .getSemanticsData();
          expect(input.flagsCollection.isTextField, isTrue);
          expect(input.value, 'A large-text chat message');
          _action(tester, 'Send');
          await pumpFrames(tester);
          final sends = harness.backend.sent
              .where((m) => m.hasSendChatMessage())
              .toList();
          expect(sends, hasLength(1));
          expect(sends.single.sendChatMessage.chatId, 'chat1');
          expect(
            sends.single.sendChatMessage.text,
            'A large-text chat message',
          );
          expect(tester.takeException(), isNull);
        },
        variant: android,
      );

      testWidgets(
        'large text approval remains explicitly actionable: $profile',
        (tester) async {
          final harness = await _approval(tester, scale: scale, size: size);
          await tester.ensureVisible(find.text('Approve'));
          await tester.pump();
          _action(tester, 'Approve');
          await pumpFrames(tester);
          expect(
            harness.backend.sent.where((m) => m.hasAnswerAsk()),
            hasLength(1),
          );
          expect(tester.takeException(), isNull);
        },
        variant: android,
      );

      testWidgets('large text manual connect remains reachable: $profile', (
        tester,
      ) async {
        final harness = await showAndroid(
          tester,
          harness: Harness(Workspace(), localBackend: false),
          textScale: scale,
          size: size,
        );
        expect(find.byType(PairScreen), findsOneWidget);
        await pastPairStart(tester, paste: true);
        final field = find.byType(EditableText);
        await tester.ensureVisible(field);
        final code = 'factory://?t=${'a' * 64}&e=${'c' * 64}&name=Fixture';
        await tester.enterText(field, code);
        await tester.pump();
        await tester.ensureVisible(find.text('Connect'));
        await pumpFrames(tester);
        _action(tester, 'Connect');
        await pumpFrames(tester);
        // A code asks first (confirmPairing): its Connect, announced alone.
        _action(tester, 'Connect');
        await pumpFrames(tester);
        expect(harness.connections.current?.code, code);
        expect(tester.takeException(), isNull);
      }, variant: android);
    }
  }
}
