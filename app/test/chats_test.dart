import 'dart:async';
import 'dart:io';

import 'package:factory_app/chats.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show admin;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;
import 'pairing_test.dart' show until;

import 'package:factory_app/connections.dart';

Chat chat({
  String id = 'c1',
  String title = 'Trains to Lyon',
  int updated = 2,
  bool available = true,
  AgentStatus status = AgentStatus.AGENT_STATUS_IDLE,
  bool unseen = false,
  List<int> waiting = const [],
}) => Chat(
  id: id,
  title: title,
  updated: Int64(updated),
  status: status,
  unseen: unseen,
  model: 'anthropic/claude-sonnet-5',
  modelAvailable: available,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace chatting(List<Chat> chats, {String selected = 'c1'}) => Workspace(
  agents: [admin()],
  models: [Model(id: 'anthropic/claude-sonnet-5', name: 'Claude Sonnet 5')],
  chats: chats,
  selectedChatId: selected,
);

/// The selected chat's view, as the shell shows it.
class _Selected extends StatelessWidget {
  const _Selected({this.onMenu});

  final VoidCallback? onMenu;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final chat = selectedChat(store.workspace);
        if (chat == null) return const SizedBox.shrink();
        return ChatView(
          key: ValueKey('chat/${chat.id}'),
          chat: chat,
          onMenu: onMenu,
        );
      },
    );
  }
}

Future<Harness> showChat(
  WidgetTester tester, {
  Chat? with_,
  List<Entry> entries = const [],
  Size size = const Size(1200, 900),
  VoidCallback? onMenu,
}) async {
  final harness = Harness(chatting([with_ ?? chat()]));
  harness.backend.chatTranscript.load(
    Transcript(agentId: (with_ ?? chat()).id, entries: entries),
  );
  await harness.show(
    tester,
    size: size,
    home: _Selected(onMenu: onMenu),
  );
  await settle(tester);
  return harness;
}

Entry said(int seq, String text) => Entry(
  seq: Int64(seq),
  user: UserText(text: text),
);

Entry answered(int seq, String text) => Entry(
  seq: Int64(seq),
  assistant: AssistantText(text: text),
);

void main() {
  group('helpers', () {
    test('the selected chat, the newest first, and "New chat" untitled', () {
      final older = chat(id: 'c1', updated: 1);
      final newer = chat(id: 'c2', title: '', updated: 5);
      final workspace = chatting([older, newer], selected: 'c2');
      expect(selectedChat(workspace)?.id, 'c2');
      expect(selectedChat(chatting([older], selected: '')), isNull);
      expect(selectedChat(chatting([older], selected: 'gone')), isNull);
      expect(chatsNewestFirst(workspace).map((c) => c.id), ['c2', 'c1']);
      expect(chatTitle(newer), 'New chat');
      expect(chatTitle(older), 'Trains to Lyon');
      expect(chatWaiting(chat(waiting: [3, 9])), 2);
    });
  });

  testWidgets('a chat shows its title and model, and an empty one says '
      'what it is for', (tester) async {
    await showChat(tester, with_: chat(title: ''));
    expect(find.textContaining('New chat', findRichText: true), findsOneWidget);
    expect(
      find.textContaining('Claude Sonnet 5', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Ask anything'), findsOneWidget, reason: 'the box');
    expect(find.textContaining('search the web'), findsOneWidget);
    // Text only: no paperclip or microphone.
    expect(find.bySemanticsLabel('Attach files'), findsNothing);
  });

  testWidgets('Return sends to the chat, and the message shows at once', (
    tester,
  ) async {
    final harness = await showChat(tester);
    // The backend queues it while it starts, as it does.
    harness.backend.onSend = (message) {
      if (!message.hasSendChatMessage()) return;
      harness.backend.chatQueued.apply(
        QueuedMessages(
          agentId: 'c1',
          messages: [
            QueuedMessage(
              id: Int64(1),
              text: message.sendChatMessage.text,
              clientId: message.sendChatMessage.clientId,
            ),
          ],
        ),
      );
    };
    await tester.enterText(find.byType(EditableText), 'Fastest train?');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final sent = harness.backend.sent
        .where((m) => m.hasSendChatMessage())
        .single
        .sendChatMessage;
    expect(sent.chatId, 'c1');
    expect(sent.text, 'Fastest train?');
    expect(sent.clientId, isNotEmpty);
    expect(find.text('Fastest train?'), findsOneWidget);
    expect(harness.drafts.of('chat:c1'), isEmpty);
  });

  testWidgets('its conversation and what waits for it show as an agent\'s', (
    tester,
  ) async {
    final harness = await showChat(
      tester,
      entries: [said(1, 'Fastest train?'), answered(2, 'The TGV, 2h.')],
    );
    expect(find.text('Fastest train?'), findsOneWidget);
    expect(find.text('The TGV, 2h.'), findsOneWidget);
    harness.backend.chatQueued.apply(
      QueuedMessages(
        agentId: 'c1',
        messages: [QueuedMessage(id: Int64(1), text: 'And on Sunday?')],
      ),
    );
    await settle(tester);
    expect(find.text('And on Sunday?'), findsOneWidget);
  });

  testWidgets('replying, it can be stopped, by Stop or Esc', (tester) async {
    final harness = await showChat(
      tester,
      with_: chat(status: AgentStatus.AGENT_STATUS_WORKING),
      entries: [said(1, 'Fastest train?')],
    );
    await tester.tap(find.textContaining('Stop'));
    await settle(tester);
    expect(harness.last.stopChat.chatId, 'c1');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasStopChat()), hasLength(2));
  });

  testWidgets("without a model, the box says it can't answer", (tester) async {
    await showChat(tester, with_: chat(available: false));
    expect(
      find.text("No model available, so this chat can't answer."),
      findsOneWidget,
    );
    expect(find.text('Sign in…'), findsOneWidget);
    expect(
      find.textContaining('No model available', findRichText: true),
      findsWidgets,
    );
  });

  testWidgets('"…" changes its model (U17): the models to choose from, '
      'the chosen one sent', (tester) async {
    final harness = Harness(
      chatting([chat()])..models.add(Model(id: 'openrouter/x/y', name: 'Y')),
    );
    harness.backend.chatTranscript.load(Transcript(agentId: 'c1'));
    await harness.show(
      tester,
      size: const Size(1200, 900),
      home: const _Selected(),
    );
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('More'));
    await settle(tester);
    await tester.tap(find.text('Change model…'));
    await settle(tester);
    expect(find.text('Change model'), findsOneWidget);
    expect(find.text('Claude Sonnet 5'), findsWidgets);
    await tester.tap(find.text('Y'));
    await settle(tester);
    expect(harness.last.setChatModel.chatId, 'c1');
    expect(harness.last.setChatModel.model, 'openrouter/x/y');
    expect(find.text('Change model'), findsNothing, reason: 'it closed');
  });

  testWidgets('without a model, Change model… beside Sign in…', (tester) async {
    final harness = Harness(
      chatting([chat(available: false)])
        ..models.add(Model(id: 'openrouter/x/y', name: 'Y')),
    );
    harness.backend.chatTranscript.load(Transcript(agentId: 'c1'));
    await harness.show(
      tester,
      size: const Size(1200, 900),
      home: const _Selected(),
    );
    await settle(tester);
    expect(find.text('Sign in…'), findsOneWidget);
    await tester.tap(find.text('Change model…'));
    await settle(tester);
    await tester.tap(find.text('Y'));
    await settle(tester);
    expect(harness.last.setChatModel.model, 'openrouter/x/y');
  });

  testWidgets("a proposal to Rivet in a chat is decided as the chat's", (
    tester,
  ) async {
    final harness = await showChat(
      tester,
      with_: chat(waiting: [4]),
      entries: [
        Entry(
          seq: Int64(4),
          proposal: Proposal(id: 'p7', text: 'Add a travel agent.'),
        ),
      ],
    );
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final decide = harness.last.decideProposal;
    expect(decide.chatId, 'c1');
    expect(decide.agentId, isEmpty);
    expect(decide.proposalId, 'p7');
    expect(decide.approve, isTrue);
  });

  testWidgets('its title renames it; empty gives it back its own', (
    tester,
  ) async {
    final harness = await showChat(tester);
    await tester.tap(find.textContaining('Trains to Lyon', findRichText: true));
    await settle(tester);
    expect(find.text('Rename chat'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Lyon trip');
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.renameChat.chatId, 'c1');
    expect(harness.last.renameChat.title, 'Lyon trip');

    await tester.tap(find.textContaining('Trains to Lyon', findRichText: true));
    await settle(tester);
    await tester.enterText(find.byType(EditableText).last, '  ');
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.renameChat.title, isEmpty);
  });

  testWidgets('"…" deletes it, once asked', (tester) async {
    final harness = await showChat(tester);
    await tester.tap(find.bySemanticsLabel('More'));
    await settle(tester);
    await tester.tap(find.text('Delete…'));
    await settle(tester);
    expect(find.text('Delete Trains to Lyon?'), findsOneWidget);
    expect(
      find.text("Its conversation is deleted. This can't be undone."),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasDeleteChat()), isEmpty);

    await tester.tap(find.bySemanticsLabel('More'));
    await settle(tester);
    await tester.tap(find.text('Delete…'));
    await settle(tester);
    await tester.tap(find.text('Delete'));
    await settle(tester);
    expect(harness.last.deleteChat.chatId, 'c1');
  });

  testWidgets('on a phone: Menu, the title, and "…"', (tester) async {
    var menus = 0;
    await showChat(tester, size: const Size(390, 844), onMenu: () => menus++);
    expect(find.text('Trains to Lyon'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Menu'));
    expect(menus, 1);
    expect(find.bySemanticsLabel('More'), findsOneWidget);
  });

  testWidgets('New chat and opening one ask the backend', (tester) async {
    final harness = Harness(chatting([chat()], selected: ''));
    late BuildContext context;
    await harness.show(
      tester,
      home: Builder(
        builder: (inner) {
          context = inner;
          return const SizedBox.shrink();
        },
      ),
    );
    startNewChat(context);
    await tester.pump();
    expect(harness.last.createChat.text, isEmpty);
    openChat(context, chat());
    await tester.pump();
    expect(harness.last.selectChat.chatId, 'c1');
  });

  group('the history', () {
    Future<(Harness, List<Chat>)> showHistory(WidgetTester tester) async {
      final harness = Harness(
        chatting([
          chat(id: 'c1', title: 'Trains to Lyon', updated: 1),
          chat(id: 'c2', title: 'Sourdough starter', updated: 3, unseen: true),
          chat(id: 'c3', title: 'Lyon restaurants', updated: 2, waiting: [4]),
        ], selected: ''),
      );
      final opened = <Chat>[];
      await harness.show(tester, home: ChatHistory(onOpen: opened.add));
      await settle(tester);
      return (harness, opened);
    }

    testWidgets('lists every chat newest first, and opens one', (tester) async {
      final (_, opened) = await showHistory(tester);
      final top = [
        for (final title in [
          'Sourdough starter',
          'Lyon restaurants',
          'Trains to Lyon',
        ])
          tester.getTopLeft(find.text(title)).dy,
      ];
      expect(top, orderedEquals([...top]..sort()));
      expect(find.text('1'), findsOneWidget, reason: 'its waiting pill');
      await tester.tap(find.text('Lyon restaurants'));
      expect(opened.single.id, 'c3');
    });

    testWidgets('searches by title', (tester) async {
      await showHistory(tester);
      await tester.enterText(find.byType(EditableText), 'lyon');
      await settle(tester);
      expect(find.text('Sourdough starter'), findsNothing);
      expect(find.text('Trains to Lyon'), findsOneWidget);
      expect(find.text('Lyon restaurants'), findsOneWidget);
      await tester.enterText(find.byType(EditableText), 'paris');
      await settle(tester);
      expect(find.text('No chat matches “paris”.'), findsOneWidget);
    });

    testWidgets('a right click offers Rename and Delete', (tester) async {
      final (harness, _) = await showHistory(tester);
      await tester.tap(find.text('Trains to Lyon'), buttons: kSecondaryButton);
      await settle(tester);
      expect(find.text('Rename…'), findsOneWidget);
      await tester.tap(find.text('Delete…'));
      await settle(tester);
      await tester.tap(find.text('Delete'));
      await settle(tester);
      expect(harness.last.deleteChat.chatId, 'c1');
    });
  });

  test("the backend's chat messages fill the chat's stores", () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        for (final reply in [
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ),
          ServerMessage(
            chatTranscript: ChatTranscript(
              chatId: 'c1',
              entries: [said(1, 'Fastest train?'), answered(2, 'The')],
            ),
          ),
          ServerMessage(
            chatDelta: ChatEntryDelta(
              chatId: 'c1',
              seq: Int64(2),
              text: ' TGV',
            ),
          ),
          ServerMessage(
            chatEntry: ChatEntryUpdate(chatId: 'c1', entry: said(3, 'Thanks')),
          ),
          ServerMessage(
            chatQueuedMessages: ChatQueuedMessages(
              chatId: 'c1',
              messages: [QueuedMessage(id: Int64(1), text: 'And Sunday?')],
            ),
          ),
          // Another chat's, as a stray: not this one's.
          ServerMessage(
            chatEntry: ChatEntryUpdate(chatId: 'c2', entry: said(9, 'Other')),
          ),
        ]) {
          socket.add(reply.writeToBuffer());
        }
      });
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64);
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac'),
    );
    unawaited(backend.run());
    await until(() => backend.chatQueued.of('c1').isNotEmpty);
    final store = backend.chatTranscript;
    expect(store.chatId, 'c1');
    expect(store.length, 3);
    expect(store[1].entry.assistant.text, 'The TGV');
    expect(store[2].entry.user.text, 'Thanks');
    expect(backend.chatQueued.of('c1').single.text, 'And Sunday?');
    expect(backend.chatQueued.of('a1'), isEmpty);
    await backend.close();
    await server.close(force: true);
  });
}
