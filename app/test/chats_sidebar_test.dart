// Where chats live (#213): "Chats" under the projects in the Mac's sidebar
// and the phone's drawer, the newest 20 then Show all, each a session (a
// bubble and its title, no character, no color); a tap opens it in place of
// a conversation, a right click or a touch and hold offers Rename and
// Delete, and "+" starts a new one.
import 'package:factory_app/chats.dart' show ChatView;
import 'package:factory_app/characters/character.dart' show AgentCharacter;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _phone = Size(393, 852);

Chat _chat(int n, {String title = '', bool unseen = false, int waiting = 0}) =>
    Chat(
      id: 'chat-$n',
      title: title.isEmpty ? 'Chat number $n' : title,
      created: Int64(1000 + n),
      updated: Int64(1000 + n),
      modelAvailable: true,
      unseen: unseen,
      waitingSeqs: [for (var i = 0; i < waiting; i++) Int64(i + 1)],
    );

Workspace _workspace({List<Chat> chats = const [], String selected = ''}) =>
    Workspace(
      projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
      agents: [
        Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
        Agent(
          id: 'mike',
          projectId: 'p1',
          name: 'Mike',
          role: 'Coordinator',
          modelAvailable: true,
        ),
      ],
      selectedAgentId: 'mike',
      chats: chats,
      selectedChatId: selected,
    );

/// The chat rows in the sidebar.
Finder get _chatRows =>
    find.byWidgetPredicate((widget) => widget.key is ValueKey<String> &&
        (widget.key! as ValueKey<String>).value.startsWith('chat-row/'));

void _iPhone() {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
}

Future<void> _openDrawer(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('Menu').last);
  await pumpFrames(tester);
  expect(find.byType(Sidebar), findsOneWidget);
}

/// Scrolls the drawer or sidebar until [target] is in view.
Future<void> _reveal(WidgetTester tester, Finder target) async {
  await tester.scrollUntilVisible(
    target,
    200,
    scrollable: find
        .descendant(
          of: find.byType(Sidebar),
          matching: find.byType(Scrollable),
        )
        .first,
  );
  await pumpFrames(tester);
}

void main() {
  testWidgets('on a Mac, Chats sits under the projects: sessions, newest '
      'first, a tap opens one in place of the conversation', (tester) async {
    final harness = Harness(
      _workspace(
        chats: [
          _chat(1, title: 'History of fried chicken'),
          _chat(2, title: 'Tide tables', unseen: true),
          _chat(3, title: 'A proposal', waiting: 1),
        ],
      ),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Chats'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Chats')).dy,
      greaterThan(tester.getTopLeft(find.text('Claktory')).dy),
    );
    // Newest first.
    final titles = [
      for (final row in _chatRows.evaluate())
        (row.widget.key! as ValueKey<String>).value,
    ];
    expect(titles, ['chat-row/chat-3', 'chat-row/chat-2', 'chat-row/chat-1']);
    // Sessions, not someone: no character in them.
    expect(
      find.descendant(of: _chatRows, matching: find.byType(AgentCharacter)),
      findsNothing,
    );
    await tester.tap(find.text('History of fried chicken'));
    await pumpFrames(tester);
    expect(harness.backend.sent.last.selectChat.chatId, 'chat-1');

    // Selected, it shows in place of the conversation, and Mike's row is
    // no longer the one selected.
    await harness.apply(
      tester,
      _workspace(
        chats: [_chat(1, title: 'History of fried chicken')],
        selected: 'chat-1',
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(ChatView), findsOneWidget);
  });

  testWidgets('with no chats, a quiet New chat; "+" starts one too', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Chats'), findsOneWidget);
    await tester.tap(find.text('New chat'));
    await pumpFrames(tester);
    expect(harness.backend.sent.last.hasCreateChat(), isTrue);
    expect(harness.backend.sent.last.createChat.text, isEmpty);
    await tester.tap(find.bySemanticsLabel('New chat').first);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((sent) => sent.hasCreateChat()),
      hasLength(2),
    );
  });

  testWidgets('the 20 newest, then Show all', (tester) async {
    final harness = Harness(
      _workspace(chats: [for (var n = 1; n <= 25; n++) _chat(n)]),
    );
    await harness.show(tester, size: const Size(1200, 2400));
    await pumpFrames(tester);
    expect(_chatRows, findsNWidgets(20));
    expect(find.text('Chat number 25'), findsOneWidget);
    expect(find.text('Chat number 5'), findsNothing);
    expect(find.text('Show all'), findsOneWidget);
  });

  testWidgets('on a phone, Chats is in the drawer: a tap opens one as the '
      'conversation, a touch and hold offers Rename and Delete', (
    tester,
  ) async {
    _iPhone();
    final harness = Harness(
      _workspace(chats: [_chat(1, title: 'History of fried chicken')]),
    );
    await harness.show(tester, size: _phone);
    await pumpFrames(tester);
    await _openDrawer(tester);
    final row = find.text('History of fried chicken');
    await _reveal(tester, row);
    await tester.longPress(row);
    await pumpFrames(tester);
    expect(find.text('Rename'), findsOneWidget);
    expect(find.text('Delete'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);

    await tester.tap(row);
    await pumpFrames(tester);
    expect(harness.backend.sent.last.selectChat.chatId, 'chat-1');
    expect(find.byType(Sidebar), findsNothing, reason: 'the drawer closes');
    await harness.apply(
      tester,
      _workspace(
        chats: [_chat(1, title: 'History of fried chicken')],
        selected: 'chat-1',
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(ChatView), findsOneWidget);
  });

  testWidgets('on a phone, a chat row is the drawer\'s size and fits the '
      'largest text', (tester) async {
    _iPhone();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(
      _workspace(
        chats: [
          _chat(1, title: 'A rather long title about fried chicken history'),
          _chat(2, title: 'Waiting', waiting: 2),
        ],
      ),
    );
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    await _openDrawer(tester);
    await _reveal(tester, find.text('Waiting'));
    expect(tester.takeException(), isNull);
  });
}
