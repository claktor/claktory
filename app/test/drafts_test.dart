import 'package:factory_app/drafts.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/state.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Agent writer() => Agent(
  id: 'a2',
  projectId: 'p1',
  name: 'Writer',
  model: 'anthropic/claude-opus-5-5',
  effort: 'high',
  status: AgentStatus.AGENT_STATUS_IDLE,
  modelAvailable: true,
);

Workspace two({String selected = 'a1'}) =>
    workspace(agents: [builder(working: false), writer()], selected: selected);

String box(WidgetTester tester) =>
    tester.widget<EditableText>(find.byType(EditableText)).controller.text;

Future<void> type(WidgetTester tester, String text) async {
  await tester.enterText(find.byType(EditableText), text);
  await tester.pump();
}

void main() {
  testWidgets('a draft stays when switching agents and pages', (tester) async {
    final harness = Harness(two());
    await harness.show(tester);
    await type(tester, 'Half a thought');

    // To the other agent, whose box is its own.
    await harness.apply(tester, two(selected: 'a2'));
    await settle(tester);
    expect(box(tester), '');
    await type(tester, 'For Writer');

    await harness.apply(tester, two());
    await settle(tester);
    expect(box(tester), 'Half a thought');
    // The cursor is at its end, to carry on typing.
    final selection = tester
        .widget<EditableText>(find.byType(EditableText))
        .controller
        .selection;
    expect(selection, const TextSelection.collapsed(offset: 14));

    // Settings and the Marketplace replace the agent view for a while.
    harness.page.value = AppPage.settings;
    await settle(tester);
    harness.page.value = AppPage.marketplace;
    await settle(tester);
    harness.page.value = AppPage.agent;
    await settle(tester);
    expect(box(tester), 'Half a thought');

    await harness.apply(tester, two(selected: 'a2'));
    await settle(tester);
    expect(box(tester), 'For Writer');
  });

  testWidgets('drafts survive a restart of the app', (tester) async {
    final storage = MemoryDraftStorage();
    final first = Harness(two(), drafts: await Drafts.load(storage));
    await first.show(tester);
    await type(tester, 'Before the update');
    expect(storage.saved, {'a1': 'Before the update'});

    // A new app, reading what this device kept.
    await tester.pumpWidget(const SizedBox());
    final second = Harness(two(), drafts: await Drafts.load(storage));
    await second.show(tester);
    expect(box(tester), 'Before the update');
  });

  testWidgets('sending clears the draft', (tester) async {
    final storage = MemoryDraftStorage();
    final harness = Harness(two(), drafts: Drafts(storage));
    await harness.show(tester);
    await type(tester, 'Ship it');
    expect(storage.saved, isNotEmpty);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.sendMessage.text, 'Ship it');
    expect(storage.saved, isEmpty);
    expect(harness.drafts.of('a1'), '');
  });

  testWidgets('a slash command clears it, unless it fails', (tester) async {
    final storage = MemoryDraftStorage();
    final agent = builder(working: false)
      ..commands.add(SlashCommand(name: 'compact', description: 'Now.'));
    final harness = Harness(
      workspace(agents: [agent, writer()]),
      drafts: Drafts(storage),
    );
    await harness.show(tester);

    harness.backend.refuse = 'the agent is working';
    await type(tester, '/compact ');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.runCommand.name, 'compact');
    expect(storage.saved, {'a1': '/compact '}, reason: 'given back');

    harness.backend.refuse = null;
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(storage.saved, isEmpty);
  });

  testWidgets("drafts of agents that no longer exist are dropped", (
    tester,
  ) async {
    final storage = MemoryDraftStorage()
      ..saved = {'a1': 'Keep', 'a2': 'Writer’s', 'gone': 'Lost'};
    final drafts = await Drafts.load(storage);
    final harness = Harness(two(), drafts: drafts);
    expect(storage.saved, {'a1': 'Keep', 'a2': 'Writer’s'});

    // Deleted while the app runs.
    await harness.show(tester);
    await harness.apply(tester, workspace(agents: [builder(working: false)]));
    await tester.pump();
    expect(storage.saved, {'a1': 'Keep'});
  });

  test('nothing is dropped before the backend sends the workspace', () async {
    final storage = MemoryDraftStorage()..saved = {'a1': 'Keep'};
    final drafts = await Drafts.load(storage);
    final store = WorkspaceStore();
    drafts.watch(store);
    expect(drafts.of('a1'), 'Keep', reason: 'nothing is known yet');
    store.apply(Workspace());
    expect(drafts.of('a1'), '', reason: 'no agents at all');
  });

  test("a chat's draft stays while the chat does, and a Claktory being "
      "switched to drops nothing", () async {
    final storage = MemoryDraftStorage()
      ..saved = {'a1': 'Keep', 'chat:c1': 'Ask', 'chat:gone': 'Lost'};
    final drafts = await Drafts.load(storage);
    final store = WorkspaceStore();
    drafts.watch(store);
    store.apply(
      Workspace(
        agents: [Agent(id: 'a1')],
        chats: [Chat(id: 'c1')],
      ),
    );
    expect(storage.saved, {'a1': 'Keep', 'chat:c1': 'Ask'});
    // Another Claktory: unknown until its Workspace comes, not empty.
    store.clear();
    expect(store.received, isFalse);
    expect(drafts.of('a1'), 'Keep');
    expect(drafts.of('chat:c1'), 'Ask');
  });

  testWidgets('on the device, drafts go to the native side', (tester) async {
    const channel = MethodChannel('factory/device');
    Map<Object?, Object?> kept = {'a1': 'Saved'};
    final calls = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
      call,
    ) async {
      calls.add(call.method);
      if (call.method == 'drafts') return kept;
      if (call.method == 'setDrafts') {
        kept = call.arguments as Map<Object?, Object?>;
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    final drafts = await Drafts.load(DeviceDraftStorage());
    expect(drafts.of('a1'), 'Saved');
    drafts.set('a2', 'New');
    await tester.pump();
    expect(calls, ['drafts', 'setDrafts']);
    expect(kept, {'a1': 'Saved', 'a2': 'New'});
  });

  group('Reply', () {
    test('quotes the message, naming another agent', () {
      expect(replyQuote('Ship it?'), '> Ship it?');
      expect(
        replyQuote('  The overlay covers the window.\n\nAnd says why.\n'),
        '> The overlay covers the window.\n>\n> And says why.',
      );
      expect(replyQuote('Done.', from: 'Kenji'), '> Kenji wrote:\n> Done.');
      expect(replyQuote('   '), '');
    });

    test('a long message is quoted to about 500 characters, between words', () {
      final long = List.filled(200, 'word').join(' ');
      final quote = replyQuote(long);
      expect(quote.length, lessThanOrEqualTo(2 + replyQuoteLength + 1));
      expect(quote, endsWith('word…'));
      expect(quote, startsWith('> word word'));
    });

    test('goes above the draft, with room for the reply under it', () {
      final drafts = Drafts(MemoryDraftStorage());
      drafts.reply('a1', 'Ready to land?');
      expect(drafts.of('a1'), '> Ready to land?\n\n');
      expect(drafts.takeFocus('a1'), isTrue);
      expect(drafts.takeFocus('a1'), isFalse, reason: 'once');

      drafts.set('a1', 'Yes, after the tests.');
      drafts.reply('a1', 'Done.', from: 'Kenji');
      expect(
        drafts.of('a1'),
        '> Kenji wrote:\n> Done.\n\nYes, after the tests.',
      );
      // Nothing to quote: nothing changes.
      drafts.reply('a1', ' ');
      expect(drafts.takeFocus('a1'), isTrue);
      expect(drafts.takeFocus('a1'), isFalse);
    });

    testWidgets('the box takes it, focused, the cursor at the end', (
      tester,
    ) async {
      final harness = Harness(two());
      await harness.show(tester);
      await type(tester, 'Also: ');
      FocusManager.instance.primaryFocus?.unfocus();
      await tester.pump();
      harness.drafts.reply('a1', 'The build is green.');
      await settle(tester);
      const expected = '> The build is green.\n\nAlso:';
      expect(box(tester), expected);
      final field = tester.widget<EditableText>(find.byType(EditableText));
      expect(field.focusNode.hasFocus, isTrue);
      expect(
        field.controller.selection,
        const TextSelection.collapsed(offset: expected.length),
      );
    });
  });
}
