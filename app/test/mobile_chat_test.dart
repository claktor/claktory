// The conversation on an iPhone (research mobile.md, phase 1B; #198): the
// header's finger-sized Back or Menu, name and "…"; a message box laid out
// as iOS's Messages, Return starting a new line; 17pt reading text in a
// 16pt gutter; code that scrolls sideways without the drawer taking it;
// iOS's own text selection. The Mac stays as it was.
import 'package:cupertino_ui/cupertino_ui.dart'
    show
        cupertinoTextSelectionHandleControls,
        CupertinoAdaptiveTextSelectionToolbar;
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/inbox.dart' show NeedsYouChip, NeedsYouList;
import 'package:factory_app/mobile/composer.dart';
import 'package:factory_app/mobile/needs_you_button.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/presentation.dart'
    show PhoneSheet, sheetsPresented;
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import 'dart:ui' show Tristate;

import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool model = true}) => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'mike',
      projectId: 'p1',
      name: 'Mike',
      role: 'Coordinator',
      modelAvailable: model,
      model: 'anthropic/claude-opus-5-5',
      extensions: [AgentExtension(id: 'checklist', enabled: true)],
    ),
  ],
  models: [if (model) Model(id: 'anthropic/claude-opus-5-5', name: 'Opus 5.5')],
  extensions: [
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  ],
  selectedAgentId: 'mike',
);

Transcript _transcript() => Transcript(
  agentId: 'mike',
  entries: [
    Entry(
      seq: Int64(1),
      user: UserText(text: 'How is the build going?'),
    ),
    Entry(
      seq: Int64(2),
      assistant: AssistantText(
        text:
            'It passes now. The fix:\n\n'
            '```rust\nfn main() { println!("a line long enough to scroll '
            'sideways on any phone, and then some more"); }\n```',
      ),
    ),
  ],
);

/// The font size [text] is drawn at: its own span's, else the nearest
/// span around it that says.
double? _sizeOf(WidgetTester tester, String text) {
  double? found;
  bool visit(InlineSpan span, double? size) {
    final here = span.style?.fontSize ?? size;
    if (span is TextSpan) {
      if (span.text?.contains(text) ?? false) {
        found = here;
        return true;
      }
      for (final child in span.children ?? const <InlineSpan>[]) {
        if (visit(child, here)) return true;
      }
    }
    return false;
  }

  for (final element in find.byType(RichText).evaluate()) {
    if (visit((element.widget as RichText).text, null)) break;
  }
  return found;
}

void main() {
  /// An iPhone 16 with Mike's conversation open.
  Future<Harness> conversation(
    WidgetTester tester, {
    bool model = true,
    Transcript? transcript,
    Workspace? workspace,
    TargetPlatform platform = TargetPlatform.iOS,
  }) async {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace ?? _workspace(model: model));
    harness.backend.transcript.load(transcript ?? _transcript());
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    if (find.byType(AgentView).evaluate().isEmpty) {
      await tester.tap(find.text('Mike').first);
      await pumpFrames(tester);
    }
    expect(find.byType(AgentView), findsOneWidget);
    return harness;
  }

  Finder inView(Finder finder) =>
      find.descendant(of: find.byType(AgentView), matching: finder);

  group('the header', () {
    testWidgets('Back, the name over the role and "…", a finger each', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await conversation(tester);
      final view = tester.widget<AgentView>(find.byType(AgentView));
      final leading = view.onBack != null ? 'Back' : 'Menu';
      for (final label in [leading, 'Extensions']) {
        final size = tester.getSize(inView(find.bySemanticsLabel(label)).first);
        expect(size.width, greaterThanOrEqualTo(44), reason: label);
        expect(size.height, greaterThanOrEqualTo(44), reason: label);
      }
      expect(inView(find.text('Mike')), findsOneWidget);
      expect(inView(find.text('Coordinator')), findsOneWidget);
      expect(
        inView(find.text('Opus 5.5')),
        findsNothing,
        reason: 'the model steps aside on a phone',
      );
      // Under the status bar.
      expect(tester.getTopLeft(inView(find.text('Mike'))).dy, greaterThan(59));
      semantics.dispose();
    });

    testWidgets('every control in the conversation is a labelled finger', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await conversation(tester);
      final wrong = <String>[];
      final seen = <String>{};
      void visit(SemanticsNode node) {
        final data = node.getSemanticsData();
        final tappable =
            data.hasAction(SemanticsAction.tap) ||
            data.hasAction(SemanticsAction.longPress);
        final button = data.flagsCollection.isButton;
        if ((tappable || button) && !node.isInvisible) {
          final name = data.label.isEmpty ? '(no label)' : data.label;
          seen.add(name);
          final size = node.transform == null
              ? node.rect.size
              : MatrixUtils.transformRect(node.transform!, node.rect).size;
          if (data.label.isEmpty && data.tooltip.isEmpty) wrong.add(name);
          if (size.width < 43.5 || size.height < 43.5) {
            wrong.add('$name is ${size.width}x${size.height}');
          }
          final disabled = data.flagsCollection.isEnabled == Tristate.isFalse;
          if (button && !tappable && !disabled) {
            wrong.add('$name has no tap for VoiceOver');
          }
          if (node.childrenCount > 0 && data.hasAction(SemanticsAction.tap)) {
            wrong.add('$name holds other elements');
          }
        }
        node.visitChildren((child) {
          visit(child);
          return true;
        });
      }

      visit(tester.getSemantics(find.byType(AgentView)));
      expect(wrong, isEmpty);
      expect(seen, containsAll(['Menu', 'Attach files', 'Send', 'Extensions']));
      semantics.dispose();
    });

    testWidgets('the name opens the agent\'s settings', (tester) async {
      await conversation(tester);
      await tester.tap(inView(find.text('Mike')));
      await pumpFrames(tester);
      expect(find.text('Agent settings'), findsWidgets);
    });
  });

  group('the message box', () {
    testWidgets('+, the field and Send, as Messages has them', (tester) async {
      final harness = await conversation(tester);
      final field = inView(find.byType(EditableText)).last;
      final editable = tester.widget<EditableText>(field);
      expect(editable.style.fontSize, 17);
      expect(editable.textInputAction, TextInputAction.newline);
      expect(editable.textCapitalization, TextCapitalization.sentences);
      expect(editable.focusNode.hasFocus, isFalse, reason: 'no autofocus');
      final attach = tester.getRect(find.bySemanticsLabel('Attach files'));
      final send = tester.getRect(find.byType(SendButton));
      final box = tester.getRect(field);
      expect(attach.size, const Size(44, 44));
      expect(send.size, const Size(44, 44));
      expect(attach.right, lessThanOrEqualTo(box.left), reason: '+ first');
      expect(send.left, greaterThanOrEqualTo(box.right), reason: 'Send last');

      // Nothing to send yet.
      await tester.tap(find.byType(SendButton));
      await pumpFrames(tester);
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);

      await tester.enterText(field, 'Ship it');
      await pumpFrames(tester);
      await tester.tap(find.byType(SendButton));
      await pumpFrames(tester);
      final sent = harness.backend.sent.where((m) => m.hasSendMessage());
      expect(sent.single.sendMessage.text, 'Ship it');
      expect(sent.single.sendMessage.agentId, 'mike');
    });

    testWidgets("on Android, +, Send and the field are Android's 48", (
      tester,
    ) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      await conversation(tester, platform: TargetPlatform.android);
      final send = tester.getRect(find.byType(SendButton));
      expect(send.size, const Size(48, 48));
      final attach = tester.getRect(find.bySemanticsLabel('Attach files'));
      expect(attach.size, const Size(48, 48));
      final row = tester.getRect(find.byType(PhoneComposerRow));
      expect(row.height, greaterThanOrEqualTo(48));
      final semantics = tester.ensureSemantics();
      final field = inView(find.byType(EditableText)).last;
      expect(tester.getSemantics(field).rect.height, greaterThanOrEqualTo(48));
      semantics.dispose();
    });

    testWidgets("VoiceOver calls the field 'Message Mike', typed in or not, "
        'even with his own placeholder', (tester) async {
      final workspace = _workspace();
      workspace.agents.last.placeholder =
          'What should I build?\nAsk me anything';
      final semantics = tester.ensureSemantics();
      await conversation(tester, workspace: workspace);
      final field = inView(find.byType(EditableText)).last;
      SemanticsNode node() => tester.getSemantics(field);
      expect(node().label, 'Message Mike');
      expect(node().flagsCollection.isTextField, isTrue);
      expect(find.bySemanticsLabel('What should I build?'), findsNothing);
      // The whole box, as tall as the buttons beside it.
      expect(node().rect.height, greaterThanOrEqualTo(44));

      // VoiceOver's double tap brings the keyboard up.
      expect(node().getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
      node().owner!.performAction(node().id, SemanticsAction.tap);
      await pumpFrames(tester);
      expect(tester.widget<EditableText>(field).focusNode.hasFocus, isTrue);
      // And says so, editable, even with no keyboard rising (a hardware one).
      expect(node().flagsCollection.isFocused, Tristate.isTrue);
      expect(
        node().getSemanticsData().hasAction(SemanticsAction.setText),
        isTrue,
      );
      tester.widget<EditableText>(field).focusNode.unfocus();
      await pumpFrames(tester);

      // A tap anywhere in the box, not only on the text, focuses it.
      final box = tester.getRect(find.byType(PhoneComposerRow));
      final editable = tester.widget<EditableText>(field);
      await tester.tapAt(Offset(box.center.dx, box.top + 3));
      await pumpFrames(tester);
      expect(editable.focusNode.hasFocus, isTrue);

      await tester.enterText(field, 'Ship it');
      await pumpFrames(tester);
      expect(node().label, 'Message Mike');
      expect(node().value, 'Ship it');
      final data = node().getSemanticsData();
      expect(data.hasAction(SemanticsAction.setText), isTrue);
      expect(data.hasAction(SemanticsAction.setSelection), isTrue);
      semantics.dispose();
    });

    group('+ asks the phone for its picker, never a desktop channel', () {
      /// Taps + on [platform] with factory/device answering [answer]
      /// (none: no picker in this app); the methods factory/window was
      /// asked for.
      Future<List<String>> attach(
        WidgetTester tester,
        TargetPlatform platform, {
        Future<Object?> Function()? answer,
      }) async {
        final desktop = <String>[];
        final messenger = tester.binding.defaultBinaryMessenger;
        messenger.setMockMethodCallHandler(
          const MethodChannel('factory/window'),
          (call) async {
            desktop.add(call.method);
            throw MissingPluginException();
          },
        );
        messenger.setMockMethodCallHandler(
          const MethodChannel('factory/device'),
          (call) async => call.method != 'pickAttachments'
              ? null
              : answer == null
              ? throw MissingPluginException()
              : answer(),
        );
        addTearDown(() {
          messenger.setMockMethodCallHandler(
            const MethodChannel('factory/window'),
            null,
          );
          messenger.setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          );
        });
        if (platform == TargetPlatform.android) {
          PhonePlatformPolicy.debugAndroidPolicy = true;
          addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
        }
        await conversation(tester, platform: platform);
        await tester.tap(find.bySemanticsLabel('Attach files'));
        await pumpFrames(tester);
        return desktop;
      }

      testWidgets('the iPhone: cancelled, nothing said', (tester) async {
        var asked = 0;
        final desktop = await attach(
          tester,
          TargetPlatform.iOS,
          answer: () async {
            asked++;
            return <Object?>[];
          },
        );
        expect(asked, 1);
        expect(desktop, isNot(contains('pickFiles')));
        expect(
          find.text("Couldn't open your photos or files. Try again."),
          findsNothing,
        );
        expect(
          find.text("You can't attach files from this phone yet."),
          findsNothing,
        );
      });

      testWidgets("the iPhone's picker failing says so", (tester) async {
        await attach(
          tester,
          TargetPlatform.iOS,
          answer: () async => throw PlatformException(code: 'failed'),
        );
        expect(
          find.text("Couldn't open your photos or files. Try again."),
          findsOneWidget,
        );
      });

      testWidgets("Android, with no picker yet, says so and doesn't ask the "
          "Mac's", (tester) async {
        final desktop = await attach(tester, TargetPlatform.android);
        expect(desktop, isNot(contains('pickFiles')));
        expect(
          find.text("You can't attach files from this phone yet."),
          findsOneWidget,
        );
      });
    });

    testWidgets('Return starts a new line rather than sending', (tester) async {
      final harness = await conversation(tester);
      final field = inView(find.byType(EditableText)).last;
      await tester.showKeyboard(field);
      await tester.enterText(field, 'One line');
      await tester.testTextInput.receiveAction(TextInputAction.newline);
      await pumpFrames(tester);
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      // A hardware keyboard's Return too.
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await pumpFrames(tester);
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
      // ⌘Return sends.
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await pumpFrames(tester);
      expect(
        harness.backend.sent.where((m) => m.hasSendMessage()),
        hasLength(1),
      );
    });

    testWidgets('empty, the box is one line, growing with what is written', (
      tester,
    ) async {
      await conversation(tester);
      final row = find.byType(PhoneComposerRow);
      // One line: no taller than its 44pt buttons and a few points.
      expect(tester.getSize(row).height, lessThan(52));
      final field = inView(find.byType(EditableText)).last;
      await tester.enterText(field, 'One\nTwo\nThree');
      await pumpFrames(tester);
      final three = tester.getSize(row).height;
      expect(three, greaterThan(52 + 24));
      await tester.enterText(field, '');
      await pumpFrames(tester);
      expect(tester.getSize(row).height, lessThan(52), reason: 'back to one');
    });

    testWidgets('a tap on the conversation or scrolling it tucks the '
        'keyboard away; Send keeps it', (tester) async {
      await conversation(
        tester,
        transcript: Transcript(
          agentId: 'mike',
          entries: [
            for (var seq = 1; seq <= 30; seq++)
              Entry(
                seq: Int64(seq),
                assistant: AssistantText(text: 'Reply number $seq, ' * 6),
              ),
          ],
        ),
      );
      final field = inView(find.byType(EditableText)).last;
      bool focused() => tester.widget<EditableText>(field).focusNode.hasFocus;
      final chat = tester.getRect(find.byType(TranscriptView));

      await tester.showKeyboard(field);
      await pumpFrames(tester);
      expect(focused(), isTrue);
      await tester.tapAt(chat.center);
      await pumpFrames(tester);
      expect(focused(), isFalse, reason: 'a tap on the conversation');

      await tester.showKeyboard(field);
      await pumpFrames(tester);
      await tester.dragFrom(chat.center, const Offset(0, 200));
      await pumpFrames(tester);
      expect(focused(), isFalse, reason: 'scrolling the conversation');

      await tester.showKeyboard(field);
      await tester.enterText(field, 'Keep me');
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('Send'));
      await pumpFrames(tester);
      expect(focused(), isTrue, reason: 'Send keeps the keyboard');
    });

    testWidgets('with the keyboard up, the edge swipe still opens the '
        'drawer', (tester) async {
      // A sheet an earlier test left up when its app went counts still.
      sheetsPresented.value = 0;
      await conversation(tester);
      await tester.showKeyboard(inView(find.byType(EditableText)).last);
      await pumpFrames(tester);
      await tester.dragFrom(const Offset(6, 420), const Offset(300, 0));
      await pumpFrames(tester);
      expect(find.byType(Sidebar), findsOneWidget);
    });

    testWidgets('a long draft scrolls in a box that keeps the view', (
      tester,
    ) async {
      await conversation(tester);
      tester.view.viewInsets = const FakeViewPadding(bottom: 336);
      addTearDown(tester.view.resetViewInsets);
      await pumpFrames(tester);
      final field = inView(find.byType(EditableText)).last;
      await tester.enterText(
        field,
        List.filled(30, 'A line of a draft').join('\n'),
      );
      await pumpFrames(tester);
      final box = tester.getRect(find.byType(PhoneComposerRow));
      final view = tester.getRect(find.byType(AgentView));
      expect(box.bottom, lessThanOrEqualTo(852 - 336 + 0.5));
      expect(
        box.height,
        lessThan((view.height) * 0.45),
        reason: 'the conversation stays in view above it',
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('without a model, what to do in its place', (tester) async {
      await conversation(tester, model: false);
      expect(inView(find.byType(PhoneComposerRow)), findsNothing);
      expect(inView(find.text('Connect a model…')), findsOneWidget);
      final button = tester.getSize(
        find
            .ancestor(
              of: find.text('Connect a model…'),
              matching: find.byType(Semantics),
            )
            .first,
      );
      expect(button.height, greaterThanOrEqualTo(44));
    });

    testWidgets('without a model, what to do fits a small phone at any text '
        'size', (tester) async {
      await conversation(tester, model: false);
      for (final scale in [1.0, 2.0, 3.1]) {
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        tester.view.physicalSize = const Size(320, 568) * 3;
        tester.view.devicePixelRatio = 3;
        await pumpFrames(tester);
        expect(tester.takeException(), isNull, reason: '$scale');
        final action = tester.getRect(inView(find.text('Connect a model…')));
        expect(action.right, lessThanOrEqualTo(320), reason: '$scale');
      }
      tester.platformDispatcher.clearTextScaleFactorTestValue();
    });
  });

  group('the transcript', () {
    testWidgets('17pt reading text in a 16pt gutter', (tester) async {
      await conversation(tester);
      final reply = find.textContaining('It passes now', findRichText: true);
      expect(reply, findsWidgets);
      expect(_sizeOf(tester, 'It passes now'), 17);
      final left = tester.getTopLeft(reply.first).dx;
      expect(left, lessThan(24 + 30), reason: 'a phone gutter, not 24pt');
      // The person's bubble keeps most of the width for words.
      final asked = tester.getRect(find.text('How is the build going?'));
      expect(asked.left, lessThan(393 * 0.3));
    });

    testWidgets("code scrolls sideways and the drawer doesn't take it", (
      tester,
    ) async {
      await conversation(tester);
      final code = find.textContaining('fn main()', findRichText: true);
      expect(code, findsWidgets);
      expect(
        find.ancestor(of: code.first, matching: find.byType(NoDrawerGesture)),
        findsOneWidget,
      );
      expect(_sizeOf(tester, 'fn main()'), 15);
    });

    testWidgets('scrolled up, Latest takes you back down', (tester) async {
      final semantics = tester.ensureSemantics();
      Entry reply(int seq) => Entry(
        seq: Int64(seq),
        assistant: AssistantText(text: 'Reply number $seq, ' * 6),
      );
      final harness = await conversation(
        tester,
        transcript: Transcript(
          agentId: 'mike',
          entries: [for (var seq = 1; seq <= 40; seq++) reply(seq)],
        ),
      );
      Finder latest(String label) => find.descendant(
        of: find.byType(AgentView),
        matching: find.text(label),
      );
      expect(
        latest('Latest').hitTestable(),
        findsNothing,
        reason: 'at the end',
      );

      await tester.drag(
        inView(find.byType(CustomScrollView)),
        const Offset(0, 600),
      );
      await pumpFrames(tester);
      expect(latest('Latest').hitTestable(), findsOneWidget);
      final button = tester.getSize(
        find.bySemanticsLabel('Go to the latest message'),
      );
      expect(button.height, greaterThanOrEqualTo(44));

      // A reply meanwhile is counted, and the view stays where it is.
      harness.backend.transcript.update(
        EntryUpdate(agentId: 'mike', entry: reply(41)),
      );
      await pumpFrames(tester);
      expect(latest('1 new'), findsOneWidget);
      expect(
        find.bySemanticsLabel('1 new message, go to the latest'),
        findsOneWidget,
      );

      await tester.tap(latest('1 new'));
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 1));
      await pumpFrames(tester);
      expect(
        inView(find.textContaining('Reply number 41', findRichText: true))
            .hitTestable(),
        findsWidgets,
      );
      expect(latest('Latest').hitTestable(), findsNothing);
      expect(latest('1 new').hitTestable(), findsNothing);
      semantics.dispose();
    });

    testWidgets('a queued message is faded, still readable; Queued is not', (
      tester,
    ) async {
      final harness = await conversation(tester);
      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'mike',
          messages: [
            QueuedMessage(id: Int64(7), source: 'person', text: 'And then?'),
          ],
        ),
      );
      await pumpFrames(tester);
      await tester.pump(const Duration(seconds: 2));
      await pumpFrames(tester);
      double fade(Finder finder) {
        var opacity = 1.0;
        tester.element(finder).visitAncestorElements((ancestor) {
          final widget = ancestor.widget;
          if (widget is Opacity) opacity *= widget.opacity;
          return true;
        });
        return opacity;
      }

      final words = inView(find.text('And then?'));
      expect(words, findsOneWidget);
      expect(fade(words), 0.75);
      expect(fade(inView(find.text('Queued'))), 1);
    });

    testWidgets("iOS's own selection: handles and the system's toolbar", (
      tester,
    ) async {
      await conversation(tester);
      final region = tester.widget<SelectableRegion>(
        inView(find.byType(SelectableRegion)).first,
      );
      expect(
        region.selectionControls,
        same(cupertinoTextSelectionHandleControls),
      );
      expect(
        region.magnifierConfiguration,
        isNot(TextMagnifierConfiguration.disabled),
      );
    });
  });

  group("a message's actions", () {
    testWidgets('touch and hold for Copy and Select text', (tester) async {
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String?;
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
      await conversation(tester);
      await tester.longPress(inView(find.text('How is the build going?')));
      await pumpFrames(tester);
      expect(find.text('Select text'), findsOneWidget);
      final copy = find.text('Copy');
      expect(tester.getSize(copy.hitTestable()).height, greaterThan(0));
      await tester.tap(copy);
      await pumpFrames(tester);
      expect(copied, 'How is the build going?');
      expect(find.text('Select text'), findsNothing, reason: 'the sheet went');

      // The agent's reply, on its own to select any part of it.
      await tester.longPress(
        inView(find.textContaining('It passes now', findRichText: true)).first,
      );
      await pumpFrames(tester);
      await tester.tap(find.text('Select text'));
      await pumpFrames(tester);
      final sheet = find.byType(PhoneSheet);
      expect(sheet, findsOneWidget);
      expect(
        find.descendant(of: sheet, matching: find.byType(SelectableRegion)),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: sheet,
          matching: find.textContaining('It passes now', findRichText: true),
        ),
        findsWidgets,
      );
    });

    testWidgets("once words are selected, a hold is the selection's", (
      tester,
    ) async {
      await conversation(tester);
      final message = inView(find.text('How is the build going?'));
      final at = tester.getRect(message).centerLeft + const Offset(20, 0);
      await tester.tapAt(at);
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tapAt(at);
      await pumpFrames(tester);
      expect(
        find.byType(CupertinoAdaptiveTextSelectionToolbar),
        findsOneWidget,
        reason: 'a double tap selects',
      );
      await tester.longPressAt(at);
      await pumpFrames(tester);
      expect(find.text('Select text'), findsNothing);

      // Selection over, a hold is the message's again.
      await tester.tapAt(const Offset(200, 300));
      await pumpFrames(tester);
      await tester.longPress(message);
      await pumpFrames(tester);
      expect(find.text('Select text'), findsOneWidget);
    });

    testWidgets('VoiceOver has them without a hold', (tester) async {
      final semantics = tester.ensureSemantics();
      await conversation(tester);
      final node = tester.getSemantics(
        inView(find.text('How is the build going?')),
      );
      final ids = node.getSemanticsData().customSemanticsActionIds ?? [];
      final labels = [
        for (final id in ids) CustomSemanticsAction.getAction(id)?.label,
      ];
      expect(labels, containsAll(['Copy', 'Select text']));
      semantics.dispose();
    });
  });

  testWidgets('Needs you: a button beside "…" in the header, clear of the '
      'conversation, opens the Needs you page', (tester) async {
    sheetsPresented.value = 0;
    Workspace waiting(List<int> seqs, {String selected = 'mike'}) =>
        _workspace()
          ..agents.first.waitingSeqs.addAll([for (final s in seqs) Int64(s)])
          ..selectedAgentId = selected;
    final harness = await conversation(tester, workspace: waiting([5]));
    harness.backend.entriesById['rivet/5'] = Entry(
      seq: Int64(5),
      timestampMs: Int64(500),
      proposal: Proposal(id: 'prop5', text: 'Add a designer agent'),
    );
    // No chip floating over the conversation (#321).
    expect(find.byType(NeedsYouChip), findsNothing);
    final chip = find.byType(PhoneNeedsYouButton);
    // The count on its glyph, and the chip's words for VoiceOver.
    expect(find.descendant(of: chip, matching: find.text('1')), findsOneWidget);
    expect(
      find.bySemanticsLabel('1 thing needs your answer. Show it.'),
      findsOneWidget,
    );
    final rect = tester.getRect(chip);
    final more = tester.getRect(inView(find.bySemanticsLabel('Extensions')));
    expect(rect.size, const Size(44, 44));
    // In the header, just before "…".
    expect(rect.top, more.top);
    expect(rect.right, lessThanOrEqualTo(more.left));
    expect(
      rect.bottom,
      lessThanOrEqualTo(tester.getRect(find.byType(TranscriptView)).top),
    );

    await tester.tap(chip);
    await pumpFrames(tester);
    await pumpFrames(tester);
    // The Needs you page, as the drawer opens it: what a card opens
    // (Review) comes over it, and Back returns to it.
    expect(find.byType(PhoneSheet), findsNothing);
    expect(harness.page.value, AppPage.inbox);
    expect(find.byType(NeedsYouList), findsOneWidget);
    expect(
      find.text('Proposes to Rivet: Add a designer agent'),
      findsOneWidget,
    );

    // Answered elsewhere: the page stays, saying so, for the person to close.
    await harness.apply(tester, waiting([]));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
    expect(find.text('Nothing is waiting for your answer.'), findsOneWidget);

    // Back: the conversation, whose header has no such button now.
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    expect(
      find.descendant(
        of: find.byType(PhoneNeedsYouButton),
        matching: find.byType(Semantics),
      ),
      findsNothing,
    );
    expect(sheetsPresented.value, 0);
  });

  testWidgets('a Mac keeps its header and box', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(_transcript());
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Opus 5.5'), findsOneWidget);
    expect(find.byType(PhoneComposerRow), findsNothing);
    expect(
      find.byWidgetPredicate(
        (widget) => widget is GlyphButton && widget.label == 'Attach files',
      ),
      findsOneWidget,
    );
    final editable = tester.widget<EditableText>(
      find.byType(EditableText).last,
    );
    expect(editable.style.fontSize, 13);
    // A touch and hold is the pointer's to select with.
    await tester.longPress(find.text('How is the build going?'));
    await pumpFrames(tester);
    expect(find.text('Select text'), findsNothing);
  });
}
