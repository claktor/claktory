import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart' show ReconnectingLine, WorkingLine;
import 'attachments.dart' show LocalFile;
import 'backend.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'find.dart' show ClearOfFindBar;
import 'forms.dart';
import 'identity.dart';
import 'inbox.dart' show NeedsYouChip;
import 'message_box.dart';
import 'mobile/header.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart';
import 'mobile/page_find.dart' show phonePageSearch;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'model_step.dart' show showConnectModel;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show modelLabel;
import 'transcript.dart';
import 'usage_wait.dart' show UsageSwitchLine, usageWaitLine;

/// A chat's color: its glyph, its bar's line and the person's bubbles in
/// it. Slate, no agent's, since a chat is no one in particular; white text
/// keeps its contrast on it.
const chatColor = Color(0xFF5A6377);

/// What a chat is called while it has no title (Chat.title is empty until
/// its first message).
const newChatTitle = 'New chat';

/// The chat the main pane shows, if any (Workspace.selected_chat_id).
Chat? selectedChat(Workspace workspace) {
  final id = workspace.selectedChatId;
  if (id.isEmpty) return null;
  for (final chat in workspace.chats) {
    if (chat.id == id) return chat;
  }
  return null;
}

/// Every chat, the one with the latest message first.
List<Chat> chatsNewestFirst(Workspace workspace) =>
    [...workspace.chats]..sort((a, b) => b.updated.compareTo(a.updated));

/// [chat]'s title as shown: "New chat" until it has one.
String chatTitle(Chat chat) => chat.title.isEmpty ? newChatTitle : chat.title;

/// How many things in [chat] wait for the person (a proposal to Rivet).
int chatWaiting(Chat chat) => chat.waitingSeqs.length;

/// Opens [chat] in the main pane, on every device.
void openChat(BuildContext context, Chat chat) {
  AppScope.closePage(context);
  fireAndLog(AppScope.of(context).selectChat(chat.id));
}

/// Starts a chat: an empty one opens at once, its box ready; left with
/// nothing said, the Claktory drops it.
void startNewChat(BuildContext context) {
  AppScope.closePage(context);
  fireAndLog(AppScope.of(context).createChat());
}

/// Names [chat], or gives it back its own title (empty).
Future<void> renameChat(BuildContext context, Chat chat) => showModal(
  context,
  (context, close) => _RenameChatForm(chat: chat, close: close),
);

/// Deletes [chat] once the person says so: it can't be undone.
Future<void> deleteChat(BuildContext context, Chat chat) async {
  final backend = AppScope.of(context);
  final sure = await showActions<bool>(
    context,
    title: 'Delete ${chatTitle(chat)}?',
    message: "Its conversation is deleted. This can't be undone.",
    actions: const [SheetAction('Delete', true, destructive: true)],
  );
  if (sure != true) return;
  fireAndLog(backend.deleteChat(chat.id));
}

/// What can be done with [chat] from its "…": on a phone, Search (its
/// find bar); Rename… and Delete….
Future<void> _chatActions(BuildContext context, Chat chat) async {
  final search = phonePageSearch(context);
  final action = await showActions<String>(
    context,
    title: chatTitle(chat),
    actions: [
      if (search != null)
        SheetAction(
          'Search',
          'search',
          glyph: (color) => ExtensionGlyph(
            icon: 'search',
            name: 'Search',
            color: color,
            size: 16,
          ),
        ),
      const SheetAction('Rename…', 'rename'),
      const SheetAction('Delete…', 'delete', destructive: true),
    ],
  );
  if (!context.mounted) return;
  switch (action) {
    case 'search':
      search?.call();
    case 'rename':
      await renameChat(context, chat);
    case 'delete':
      await deleteChat(context, chat);
  }
}

/// Which chat's conversation is around: a proposal card in it decides as
/// the chat's (DecideProposal.chat_id).
class ChatScope extends InheritedWidget {
  const ChatScope({super.key, required this.chatId, required super.child});

  final String chatId;

  /// The chat whose conversation [context] is in; null in an agent's.
  static String? chatIdOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<ChatScope>()?.chatId;

  @override
  bool updateShouldNotify(ChatScope old) => old.chatId != chatId;
}

/// A chat's glyph: a round speech bubble (Lucide message-circle).
class ChatGlyph extends StatelessWidget {
  const ChatGlyph({super.key, this.color, this.size = 16});

  /// `textMuted` when not given.
  final Color? color;
  final double size;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: Size.square(size),
    painter: _ChatPainter(color ?? Palette.of(context).textMuted),
  );
}

class _ChatPainter extends CustomPainter {
  const _ChatPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final unit = size.shortestSide / 24;
    canvas.scale(unit);
    // M7.9 20 A9 9 0 1 0 4 16.1 L2 22 Z
    double angle(double x, double y) => math.atan2(y - 12, x - 12);
    final from = angle(7.9, 20);
    final to = angle(4, 16.1);
    final path = Path()
      ..moveTo(7.9, 20)
      ..arcTo(
        Rect.fromCircle(center: const Offset(12, 12), radius: 9),
        from,
        -(2 * math.pi - (to - from)),
        false,
      )
      ..lineTo(2, 22)
      ..close();
    canvas.drawPath(
      path,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(_ChatPainter old) => old.color != color;
}

/// A chat's message box: text only, its draft kept as any agent's, sent
/// through the chat's outbox; without a model, why it can't answer.
class ChatMessageTarget extends MessageTarget {
  ChatMessageTarget(this.chat, this.backend);

  final Chat chat;
  final Backend backend;

  @override
  String get key => 'chat:${chat.id}';

  @override
  String get placeholder => 'Ask anything';

  @override
  bool get takesFiles => false;

  @override
  bool get takesVoice => false;

  @override
  MessageBlocked? blocked(BuildContext context) {
    if (chat.modelAvailable) return null;
    if (backend.workspace.workspace.models.isEmpty) {
      return MessageBlocked(
        'Connect a model so this chat can answer.',
        actions: [
          MessageAction(
            'Connect a model…',
            () => showConnectModel(context),
            primary: true,
          ),
        ],
      );
    }
    return MessageBlocked(
      "No model available, so this chat can't answer.",
      actions: [
        MessageAction(
          'Sign in…',
          () => AppScope.openProviders(context),
          primary: true,
        ),
      ],
    );
  }

  @override
  void send(String text, {List<LocalFile> files = const [], Duration? voice}) {
    if (text.isEmpty) return;
    backend.chatOutbox.send(chat.id, text);
  }
}

/// A chat in the main pane: a plain bar with its title, its conversation
/// (as an agent's, without the agent: no character, panes or extensions),
/// and a box for text. Esc stops its reply.
class ChatView extends StatefulWidget {
  const ChatView({super.key, required this.chat, this.onMenu});

  final Chat chat;

  /// On a phone, opens the drawer: Menu leads the header, and the chat is
  /// the screen.
  final VoidCallback? onMenu;

  @override
  State<ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<ChatView> {
  final _follow = ValueNotifier<int>(0);

  @override
  void dispose() {
    _follow.dispose();
    super.dispose();
  }

  bool get _working => widget.chat.status == AgentStatus.AGENT_STATUS_WORKING;

  void _stop() {
    if (_working) fireAndLog(AppScope.of(context).stopChat(widget.chat.id));
  }

  bool get _phone => widget.onMenu != null || isPhoneLayout(context);

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final chat = widget.chat;
    final transcript = backend.chatTranscript;
    final conversation = ChatScope(
      chatId: chat.id,
      child: AgentTints(
        own: chatColor,
        byId: const {},
        child: ListenableBuilder(
          listenable: Listenable.merge([
            transcript,
            backend.chatQueued,
            backend.chatOutbox,
          ]),
          builder: (context, _) {
            if (transcript.chatId != chat.id) return const SizedBox.shrink();
            if (transcript.length == 0 &&
                backend.chatQueued.of(chat.id).isEmpty &&
                backend.chatOutbox.of(chat.id).isEmpty &&
                !_working) {
              return const _EmptyChat();
            }
            return TranscriptView(
              key: ValueKey(transcript.loads),
              transcript: transcript,
              working: _working,
              follow: _follow,
              queued: backend.chatQueued,
              outbox: backend.chatOutbox,
              loadEarlier: () => backend.loadEarlierChat(chat.id),
              agentId: chat.id,
              reveal: backend.reveal,
              loadFrom: (seq) => backend.loadChatFrom(chat.id, seq),
            );
          },
        ),
      ),
    );
    final wait = chat.usageWait.isEmpty
        ? ''
        : usageWaitLine(
            Agent(
              usageWait: chat.usageWait,
              usageWaitUntilMs: chat.usageWaitUntilMs,
            ),
          );
    final body = LayoutBuilder(
      builder: (context, constraints) {
        final bottom = [
          // Back to its own provider once it has room (#341).
          UsageSwitchLine(id: chat.id),
          ReconnectingLine(
            otherwise: _working
                ? WorkingLine(onStop: _stop, wait: wait, waiting: chat.id)
                : const SizedBox.shrink(),
          ),
          MessageBox(
            target: ChatMessageTarget(chat, backend),
            onSent: () => _follow.value++,
            maxFieldHeight: constraints.maxHeight.isFinite
                ? constraints.maxHeight * 0.3
                : null,
          ),
        ];
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: Stack(
                children: [
                  Positioned.fill(child: conversation),
                  if (!_phone)
                    const Positioned(
                      top: Space.s,
                      left: Space.m,
                      right: Space.m,
                      bottom: Space.m,
                      child: Align(
                        alignment: Alignment.topRight,
                        // Aside while the chat's find bar is open there.
                        child: ClearOfFindBar(child: NeedsYouChip()),
                      ),
                    ),
                ],
              ),
            ),
            // On a phone, what's under the conversation takes at most 60%
            // of the room, scrolling within it, as in an agent's.
            if (_phone && constraints.maxHeight.isFinite)
              ConstrainedBox(
                constraints: BoxConstraints(
                  maxHeight: constraints.maxHeight * 0.6,
                ),
                child: SingleChildScrollView(
                  reverse: true,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: bottom,
                  ),
                ),
              )
            else
              ...bottom,
          ],
        );
      },
    );
    final view = CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): _stop},
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _phone
              ? _PhoneChatHeader(
                  chat: chat,
                  working: _working,
                  onMenu: widget.onMenu,
                )
              : _ChatHeader(chat: chat, working: _working),
          Expanded(child: body),
        ],
      ),
    );
    return _phone
        ? Semantics(container: true, explicitChildNodes: true, child: view)
        : view;
  }
}

/// What a chat's bar says under its title: its model (always at medium
/// effort), or that it has none.
String _chatDetails(BuildContext context, Chat chat) {
  if (!chat.modelAvailable) return 'No model available';
  final model = AppScope.of(context).workspace.model(chat.model);
  return model == null ? chat.model : modelLabel(model);
}

/// A chat's bar on a Mac: its glyph, title over its model, and "…" for
/// Rename and Delete; a click on the title renames it. Slate where an
/// agent's bar has its color, the working wave along its line while it
/// replies. It drags the window.
class _ChatHeader extends StatelessWidget {
  const _ChatHeader({required this.chat, required this.working});

  final Chat chat;
  final bool working;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final header = DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [
            chatColor.withValues(alpha: 0.14),
            chatColor.withValues(alpha: 0),
          ],
          stops: const [0, 0.7],
        ),
        border: Border(
          bottom: BorderSide(color: chatColor.withValues(alpha: 0.7), width: 2),
        ),
      ),
      child: WindowDrag(
        child: Container(
          height: titleBarHeight,
          padding: const EdgeInsets.only(left: Space.xl, right: Space.m),
          child: Row(
            children: [
              const ChatGlyph(color: chatColor, size: 22),
              const SizedBox(width: Space.s + 2),
              Expanded(
                child: Pressable(
                  onTap: () => renameChat(context, chat),
                  builder: (context, hovered) => Align(
                    alignment: Alignment.centerLeft,
                    child: Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text: chatTitle(chat),
                            style: type.label.copyWith(
                              color: palette.text,
                              fontWeight: FontWeight.w600,
                              decoration: hovered
                                  ? TextDecoration.underline
                                  : null,
                              decorationColor: palette.textMuted,
                            ),
                          ),
                          TextSpan(
                            text: '   ${_chatDetails(context, chat)}',
                            style: type.body.copyWith(color: palette.textMuted),
                          ),
                        ],
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: Space.m),
              GlyphButton(
                label: 'More',
                active: false,
                onTap: () => _chatActions(context, chat),
                glyph: (color) => ExtensionGlyph(
                  icon: 'ellipsis',
                  name: 'More',
                  color: color,
                ),
              ),
            ],
          ),
        ),
      ),
    );
    return Stack(
      children: [
        header,
        if (working)
          const Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            height: 2,
            child: WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
          ),
      ],
    );
  }
}

/// A chat's bar on a phone: Menu, its glyph and title, and "…" for Rename
/// and Delete, on a slate tint with its line (the working wave runs along
/// it while it replies). The title renames it.
class _PhoneChatHeader extends StatelessWidget {
  const _PhoneChatHeader({
    required this.chat,
    required this.working,
    this.onMenu,
  });

  final Chat chat;
  final bool working;
  final VoidCallback? onMenu;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return PhoneHeader(
      background: Color.alphaBlend(
        chatColor.withValues(alpha: 0.14),
        palette.canvas,
      ),
      hairline: false,
      leading: HeaderButton.menu(onTap: onMenu ?? () {}),
      title: Row(
        children: [
          const ExcludeSemantics(child: ChatGlyph(color: chatColor, size: 22)),
          const SizedBox(width: Space.s + 2),
          Flexible(
            child: Text(
              chatTitle(chat),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: type.emphasis.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
      titleLabel: chatTitle(chat),
      titleHint: 'Renames the chat',
      onTitleTap: () => renameChat(context, chat),
      trailing: [HeaderButton.more(onTap: () => _chatActions(context, chat))],
      bottom: SizedBox(
        height: 2,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: chatColor.withValues(alpha: 0.7)),
            if (working) const WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
          ],
        ),
      ),
    );
  }
}

/// A chat with nothing said yet: what it's for.
class _EmptyChat extends StatelessWidget {
  const _EmptyChat();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 360),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const ChatGlyph(color: chatColor, size: 40),
              const SizedBox(height: Space.m),
              Text(
                'A chat can search the web and read pages. Your agents '
                "don't see it.",
                textAlign: TextAlign.center,
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Renames a chat; an empty name gives it back its automatic title.
class _RenameChatForm extends StatefulWidget {
  const _RenameChatForm({required this.chat, required this.close});

  final Chat chat;
  final VoidCallback close;

  @override
  State<_RenameChatForm> createState() => _RenameChatFormState();
}

class _RenameChatFormState extends State<_RenameChatForm> {
  late final _title = TextEditingController(text: widget.chat.title);
  var _saving = false;
  String? _error;

  @override
  void dispose() {
    _title.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await AppScope.of(context).renameChat(widget.chat.id, _title.text.trim());
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _saving = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Rename chat',
    error: _error,
    submitLabel: 'Save',
    onSubmit: _saving ? null : _save,
    onCancel: widget.close,
    fields: [
      Field(
        label: 'Name',
        note: 'Leave it empty and Claktory names it for you.',
        child: TextField(
          controller: _title,
          autofocus: true,
          placeholder: newChatTitle,
          onSubmitted: (_) => _save(),
        ),
      ),
    ],
  );
}

/// Every chat, newest first, with a search by title: for the sidebar's
/// and drawer's "Show all". Each row opens its chat ([onOpen]); its "…",
/// a right click or a long press offers Rename and Delete. No header of
/// its own: it goes in a page or sheet that has one.
class ChatHistory extends StatefulWidget {
  const ChatHistory({super.key, required this.onOpen});

  final ValueChanged<Chat> onOpen;

  @override
  State<ChatHistory> createState() => _ChatHistoryState();
}

class _ChatHistoryState extends State<ChatHistory> {
  final _search = TextEditingController();

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final store = AppScope.of(context).workspace;
    final phone = isPhoneLayout(context);
    return ListenableBuilder(
      listenable: Listenable.merge([store, _search]),
      builder: (context, _) {
        final words = _search.text.trim().toLowerCase().split(RegExp(r'\s+'))
          ..removeWhere((word) => word.isEmpty);
        final chats = [
          for (final chat in chatsNewestFirst(store.workspace))
            if (words.every(chatTitle(chat).toLowerCase().contains)) chat,
        ];
        final selected = store.workspace.selectedChatId;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: EdgeInsets.fromLTRB(
                phone ? Space.l : Space.xl,
                Space.m,
                phone ? Space.l : Space.xl,
                Space.s,
              ),
              child: TextField(
                controller: _search,
                kind: InputKind.text,
                placeholder: 'Search chats',
              ),
            ),
            Expanded(
              child: chats.isEmpty
                  ? Padding(
                      padding: const EdgeInsets.all(Space.xl),
                      child: Text(
                        store.workspace.chats.isEmpty
                            ? 'No chats yet.'
                            : 'No chat matches “${_search.text.trim()}”.',
                        textAlign: TextAlign.center,
                        style: type.body.copyWith(color: palette.textFaint),
                      ),
                    )
                  : ListView(
                      padding: EdgeInsets.symmetric(
                        horizontal: phone ? Space.s : Space.l,
                        vertical: Space.xs,
                      ),
                      children: [
                        for (final chat in chats)
                          _ChatRow(
                            key: ValueKey(chat.id),
                            chat: chat,
                            selected: chat.id == selected,
                            onOpen: () => widget.onOpen(chat),
                          ),
                      ],
                    ),
            ),
          ],
        );
      },
    );
  }
}

/// A chat in [ChatHistory]: its glyph, title (weight 600 with news) over
/// when it last spoke, and at its end the needs-you pill, the news dot or
/// the live dot while it replies, then "…".
class _ChatRow extends StatelessWidget {
  const _ChatRow({
    super.key,
    required this.chat,
    required this.selected,
    required this.onOpen,
  });

  final Chat chat;
  final bool selected;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = isPhoneLayout(context);
    final waiting = chatWaiting(chat);
    final when = chat.updated.isZero
        ? ''
        : logTime(DateTime.fromMillisecondsSinceEpoch(chat.updated.toInt()));
    final Widget? mark = waiting > 0
        ? _Pill(waiting)
        : chat.status == AgentStatus.AGENT_STATUS_WORKING
        ? const LiveDot()
        : chat.unseen
        ? Dot(color: palette.text, size: 7)
        : null;
    return Semantics(
      button: true,
      label: [
        chatTitle(chat),
        if (when.isNotEmpty) when,
        if (waiting > 0) '$waiting waiting for your answer',
      ].join(', '),
      excludeSemantics: true,
      child: GestureDetector(
        onSecondaryTap: () => _chatActions(context, chat),
        onLongPress: () => _chatActions(context, chat),
        child: Pressable(
          onTap: onOpen,
          builder: (context, hovered) => Container(
            constraints: BoxConstraints(minHeight: phone ? Touch.target : 40),
            padding: const EdgeInsets.symmetric(
              horizontal: Space.s,
              vertical: Space.xs,
            ),
            decoration: BoxDecoration(
              color: selected
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : null,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                ChatGlyph(color: palette.textMuted),
                const SizedBox(width: Space.s),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        chatTitle(chat),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: type.body.copyWith(
                          color: palette.text,
                          fontWeight: chat.unseen || waiting > 0
                              ? FontWeight.w600
                              : null,
                        ),
                      ),
                      if (when.isNotEmpty)
                        Text(
                          when,
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                    ],
                  ),
                ),
                if (mark != null) ...[const SizedBox(width: Space.s), mark],
                const SizedBox(width: Space.xs),
                GlyphButton(
                  label: 'More',
                  active: false,
                  onTap: () => _chatActions(context, chat),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'ellipsis',
                    name: 'More',
                    color: color,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// How many things wait for the person, as the sidebar's needs-you pill.
class _Pill extends StatelessWidget {
  const _Pill(this.count);

  final int count;

  @override
  Widget build(BuildContext context) => Container(
    constraints: const BoxConstraints(minWidth: 16),
    height: 16,
    padding: const EdgeInsets.symmetric(horizontal: Space.xs),
    alignment: Alignment.center,
    decoration: BoxDecoration(
      color: Palette.of(context).signal,
      borderRadius: BorderRadius.circular(8),
    ),
    child: Text(
      count > 9 ? '9+' : '$count',
      style: TypeScale.of(context).caption.copyWith(
        color: const Color(0xFF000000),
        fontSize: 11,
        fontWeight: FontWeight.w600,
        fontFeatures: const [FontFeature.tabularFigures()],
        height: 1,
      ),
    ),
  );
}
