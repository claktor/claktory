import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../avatar.dart';
import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../message_time.dart' show dayLine;
import '../mobile/composer.dart' show PhoneComposerRow;
import '../mobile/header.dart';
import '../mobile/lines.dart' show ControlsLine;
import '../mobile/metrics.dart';
import '../mobile/navigation.dart';
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../transcript.dart' show Bubble;
import '../user_apps/session.dart';
import '../tunnel/api/dm.dart' as dm;
import 'chat.dart';
import 'e2ee.dart';
import 'my_identity.dart';
import 'problems.dart';
import 'review_view.dart';

// D3c: the chat screen: a friend's name and tag over the history, newest
// at the bottom, the person's messages on the right and theirs on the
// left, as plain text (never Markdown: it's another person's words); then
// what the person wrote here that isn't confirmed yet, each with where it
// stands; then the box to write in, whose draft is the person's on all
// their devices. An ended conversation reads the same with no box. People
// opens it: Message on a friend, Open on a past conversation. On a phone
// it's a page of its own (Prosciutto's routes), Back at its top and the
// phone's composer above the keyboard.

/// Who can read direct messages (dms-e2ee §7, Trish's words), in one
/// place: the chat's top, Add a friend and People's Friends say it.
const dmDisclosure =
    'End-to-end encrypted: only you and your friends can read these '
    'messages, on your devices and any Claktory you give a chat to. '
    "Claktory's servers see who you talk to and when, but not what you say.";

/// Stands in for the account service in tests.
ChatApi Function(BuildContext context)? chatApi;

/// How long the box rests before its draft is kept for the person's
/// other devices.
const chatDraftAfter = Duration(milliseconds: 800);

/// Opens [conversation]: a page on a phone, a sheet elsewhere.
Future<void> showChat(BuildContext context, DmConversation conversation) {
  final routes = PhoneRoutes.of(context);
  if (routes != null) {
    routes.closeDrawer();
    // One chat at a time: a second replaces the first.
    return routes.push<void>(
      PhoneRoute(
        id: 'people-chat:${conversation.id}',
        builder: (_) => ChatView(conversation: conversation, close: routes.pop),
      ),
    );
  }
  return showModal(
    context,
    (context, close) => SizedBox(
      height: 560,
      child: ChatView(conversation: conversation, close: close),
    ),
  );
}

/// Opens the chat [conversationId], found among the person's own
/// conversations (never taken from elsewhere): Needs you's way to an
/// agent's message waiting there.
Future<void> openDmChat(BuildContext context, String conversationId) async {
  final api = chatApi?.call(context) ?? ChatApi(UserAppsSession.of(context));
  final conversation = (await api.conversations())
      .where((each) => each.id == conversationId)
      .firstOrNull;
  if (conversation == null || !context.mounted) return;
  await showChat(context, conversation);
}

/// What a message written here says of where it stands; [friend] is
/// who it's to.
String outgoingLine(OutgoingMessage message, {String friend = ''}) =>
    switch (message.state) {
      OutgoingState.sending => 'Sending…',
      OutgoingState.unknown =>
        "Couldn't confirm it was sent. Try again: it won't be sent twice.",
      OutgoingState.refused => switch (message.problem) {
        'closed' => "Not sent: you're no longer friends.",
        'stale' => 'Not sent: this conversation changed. Open it again.',
        'rateLimited' =>
          'Not sent: too many messages for now. Try again later.',
        'storageFull' =>
          "Not sent: you're out of space for messages. Delete some "
              'conversations to make room.',
        'unencryptable' =>
          "Not sent: ${_named(friend)}'s security key changed. Read the note "
              'above, then send it again.',
        _ => 'Not sent.',
      },
    };

String _named(String name) => name.isEmpty ? 'Their' : name;

/// What a change in who can read a chat says, one line per leaf (Trish's
/// words): [me] is the person's account, [friend] the friend's name.
List<String> memberLines(
  dm.DmMembers members, {
  required String me,
  required String friend,
}) {
  final who = friend.isEmpty ? 'They' : friend;
  String line(dm.DmMember member, bool added) {
    final mine = member.account == me;
    if (member.kind == dm.DmLeafKind.claktory) {
      final whose = mine ? 'Your' : "$who's";
      return '$whose Claktory ${member.name} can '
          '${added ? 'now' : 'no longer'} read this chat';
    }
    return '${mine ? 'You' : who} ${added ? 'added' : 'removed'} a device: '
        '${member.name}';
  }

  return [
    for (final member in members.added) line(member, true),
    for (final member in members.removed) line(member, false),
  ];
}

class ChatView extends StatefulWidget {
  const ChatView({super.key, required this.conversation, required this.close});

  final DmConversation conversation;
  final VoidCallback close;

  @override
  State<ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<ChatView> {
  late final ChatThread _thread = ChatThread(
    chatApi?.call(context) ?? ChatApi(UserAppsSession.of(context)),
    e2eeOf(context),
    widget.conversation,
  );
  final _text = TextEditingController();

  /// The box's text as the service last had it: another device's draft
  /// replaces it only while the person hasn't typed since.
  var _synced = '';
  Timer? _draft;

  @override
  void initState() {
    super.initState();
    _thread.onDraft = (text) {
      if (_text.text != _synced) return;
      _synced = text;
      _text.text = text;
    };
    // Edit: the agent's words into the box, after what's there.
    _thread.onUse = (text) {
      final now = _text.text;
      _text.text = now.trim().isEmpty ? text : '$now\n$text';
      _text.selection = TextSelection.collapsed(offset: _text.text.length);
    };
    _text.addListener(_typed);
    unawaited(_thread.follow());
  }

  void _typed() {
    setState(() {});
    _draft?.cancel();
    if (_text.text != _synced) _draft = Timer(chatDraftAfter, _keepDraft);
  }

  void _keepDraft() {
    _draft?.cancel();
    _draft = null;
    final text = _text.text;
    if (text == _synced || !_thread.loaded) return;
    _synced = text;
    unawaited(_thread.saveDraft(text));
  }

  @override
  void dispose() {
    // What was typed and not kept yet goes now, for the other devices.
    if (_draft != null) _keepDraft();
    _thread.dispose();
    _text.dispose();
    super.dispose();
  }

  bool get _tooLong => utf8.encode(_text.text.trim()).length > messageByteLimit;

  Future<void> _send() async {
    final text = _text.text;
    if (text.trim().isEmpty || _tooLong) return;
    _text.clear();
    // Sent, the draft goes too.
    _keepDraft();
    await _thread.send(text);
  }

  Future<void> _delete() async {
    final name = _thread.conversation.person.name;
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Delete this conversation?',
        body:
            'Its messages are deleted from all your devices. '
            "${name.isEmpty ? 'They keep' : '$name keeps'} their copy.",
        fields: const [],
        error: null,
        submitLabel: 'Delete',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (sure && mounted) await _thread.deleteHistory();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _thread,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final now = DateTime.now();
    final problem = _thread.problem;
    final phone = PhoneRoutes.of(context) != null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (phone) _phoneHeader(type, palette) else ..._header(type, palette),
        Expanded(
          child: ListView(
            reverse: true,
            padding: const EdgeInsets.all(Space.l),
            children: [
              // Reversed: the newest first, at the bottom. Nearest the box,
              // what agents suggest here, then what was written here.
              if (_thread.reviewProblem case final problem?)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs),
                  child: Text(
                    reviewProblemText(problem, agent: _thread.reviewAgent),
                    style: type.caption.copyWith(color: palette.danger),
                  ),
                ),
              for (final proposal in _thread.proposals.reversed)
                ProposalCard(
                  key: ValueKey('proposal:${proposal.id}'),
                  proposal: proposal,
                  to: _thread.conversation.person,
                  thread: _thread,
                ),
              for (final draft in _thread.agentDrafts)
                AgentDraftCard(
                  key: ValueKey('agent-draft:${draft.id}'),
                  draft: draft,
                  thread: _thread,
                ),
              if (_thread.keyChanged)
                _KeyChanged(
                  name: _thread.conversation.person.name,
                  onSeen: _thread.keySeen,
                ),
              for (final message in _thread.outgoing.reversed)
                _Outgoing(
                  friend: _thread.conversation.person.name,
                  message: message,
                  onRetry: () => _thread.retry(message),
                  onDiscard: () => _thread.discard(message),
                ),
              for (final (i, message)
                  in _thread.messages.indexed.toList().reversed) ...[
                if (message.members case final members?)
                  for (final line in memberLines(
                    members,
                    me: _thread.e2ee.accountId,
                    friend: _thread.conversation.person.name,
                  ))
                    _QuietLine(line)
                else
                  _Message(
                    message: message,
                    // The newest, the person's own: it came back.
                    sent:
                        message.mine &&
                        i == _thread.messages.length - 1 &&
                        _thread.outgoing.isEmpty,
                  ),
                // A day line where the day changes, above its first.
                if (i == 0 ||
                    dayLine(_thread.messages[i - 1].acceptedMs, now) !=
                        dayLine(message.acceptedMs, now))
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: Space.s),
                    child: Center(
                      child: Text(
                        dayLine(message.acceptedMs, now),
                        style: type.caption.copyWith(color: palette.textFaint),
                      ),
                    ),
                  ),
              ],
              if (_thread.loaded && _thread.messages.isEmpty)
                Center(
                  child: Text(
                    'No messages yet.',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                ),
              if (!_thread.loaded && problem == null)
                Center(
                  child: Text(
                    'Getting your messages…',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                ),
              if (_thread.hasEarlier)
                Center(
                  child: Button(
                    label: 'Show earlier messages',
                    onPressed: _thread.loadEarlier,
                  ),
                ),
              // The conversation's top, above its first message (and above
              // No messages yet, before anything's sent): who can read it.
              if (_thread.loaded && !_thread.hasEarlier)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.m),
                  child: Text(
                    dmDisclosure,
                    textAlign: TextAlign.center,
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
                ),
            ],
          ),
        ),
        if (problem != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.l),
            child: Text(
              problemText(problem),
              style: type.caption.copyWith(color: palette.danger),
            ),
          ),
        if (!phone || !_thread.writable)
          Container(height: 1, color: palette.hairline),
        if (phone && _thread.writable)
          _phoneComposer(palette, type)
        else
          Padding(
            padding: const EdgeInsets.all(Space.m),
            child: _thread.writable
                ? _composer(palette, type)
                : _endedLine(palette, type),
          ),
      ],
    );
  }

  /// "Ana · K7QF2M": the tag always, so two of a name never pass for each
  /// other.
  Widget _name(TypeScale type, Palette palette) {
    final person = _thread.conversation.person;
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text: person.name.isEmpty ? 'No name' : person.name,
            style: TextStyle(color: palette.text),
          ),
          if (person.tag.isNotEmpty)
            TextSpan(
              text: ' · ${person.tag}',
              style: TextStyle(color: palette.textFaint),
            ),
        ],
      ),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: type.emphasis,
    );
  }

  Widget _avatar(double size) {
    final person = _thread.conversation.person;
    return PersonAvatar(
      person: peopleAvatarOf(person.name, person.avatarSeed),
      size: size,
    );
  }

  List<Widget> _header(TypeScale type, Palette palette) => [
    Padding(
      padding: const EdgeInsets.fromLTRB(Space.l, Space.l, Space.l, Space.s),
      child: Row(
        children: [
          _avatar(28),
          const SizedBox(width: Space.m),
          Expanded(child: _name(type, palette)),
          if (_thread.messages.isNotEmpty) ...[
            Button(label: 'Delete…', onPressed: _delete),
            const SizedBox(width: Space.s),
          ],
          Button(label: 'Done', onPressed: widget.close),
        ],
      ),
    ),
    Container(height: 1, color: palette.hairline),
  ];

  Widget _phoneHeader(TypeScale type, Palette palette) {
    final person = _thread.conversation.person;
    return PhoneHeader(
      leading: HeaderButton.back(onTap: widget.close),
      title: Row(
        children: [
          _avatar(28),
          const SizedBox(width: Space.s),
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  person.name.isEmpty ? 'No name' : person.name,
                  style: type.emphasis.copyWith(color: palette.text),
                ),
                if (person.tag.isNotEmpty)
                  Text(
                    person.tag,
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
              ],
            ),
          ),
        ],
      ),
      trailing: [
        if (_thread.messages.isNotEmpty)
          HeaderButton.more(
            onTap: () async {
              final chosen = await showActions<String>(
                context,
                title: person.label,
                actions: const [
                  SheetAction(
                    'Delete conversation…',
                    'delete',
                    destructive: true,
                  ),
                ],
              );
              if (chosen == 'delete') await _delete();
            },
          ),
      ],
    );
  }

  /// The phone's composer, as the agent conversation's: Return makes a new
  /// line, Send sends.
  Widget _phoneComposer(Palette palette, TypeScale type) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    mainAxisSize: MainAxisSize.min,
    children: [
      if (_tooLong)
        Padding(
          padding: const EdgeInsets.fromLTRB(Space.l, Space.xs, Space.l, 0),
          child: Text(
            'Too long to send. Make it shorter.',
            style: type.caption.copyWith(color: palette.danger),
          ),
        ),
      Padding(
        padding: const EdgeInsets.all(Space.xs),
        child: PhoneComposerRow(
          canSend: _thread.canSend && _text.text.trim().isNotEmpty && !_tooLong,
          onSend: () => unawaited(_send()),
          field: TextField(
            controller: _text,
            placeholder: 'Message',
            semanticsLabel: 'Message',
            minLines: 1,
            maxLines: 5,
            bordered: false,
          ),
        ),
      ),
    ],
  );

  Widget _composer(Palette palette, TypeScale type) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Expanded(
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): _send,
              },
              child: TextField(
                controller: _text,
                placeholder: 'Message',
                semanticsLabel: 'Message',
                minLines: 1,
                maxLines: 6,
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: 'Send',
            primary: true,
            onPressed: !_thread.canSend || _text.text.trim().isEmpty || _tooLong
                ? null
                : _send,
          ),
        ],
      ),
      if (_tooLong)
        Padding(
          padding: const EdgeInsets.only(top: Space.xs),
          child: Text(
            'Too long to send. Make it shorter.',
            style: type.caption.copyWith(color: palette.danger),
          ),
        ),
    ],
  );

  Widget _endedLine(Palette palette, TypeScale type) => Text(
    "You're no longer friends, so you can't send messages here. You can "
    'still read them.',
    style: type.caption.copyWith(color: palette.textMuted),
  );
}

/// A message from the service: the person's own on the right, filled;
/// the friend's on the left. Plain text, its time at its foot, and who
/// sent it when an agent did.
class _Message extends StatelessWidget {
  const _Message({required this.message, this.sent = false});

  final DmMessage message;

  /// Says Sent under it: the service committed it. Never more than that:
  /// nothing says whether they read it.
  final bool sent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final mine = message.mine;
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Column(
        crossAxisAlignment: mine
            ? CrossAxisAlignment.end
            : CrossAxisAlignment.start,
        children: [
          Bubble(
            color: mine ? palette.signal : palette.selected,
            outgoing: mine,
            sentMs: message.acceptedMs,
            timeColor: mine ? const Color(0xB3FFFFFF) : null,
            child: Text(
              message.text,
              style: type.body.copyWith(
                color: mine ? const Color(0xFFFFFFFF) : palette.text,
              ),
            ),
          ),
          if (message.byAgent)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: 2,
              ),
              child: Text(
                mine ? 'Sent by your agent' : 'Sent by their agent',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          if (sent && !message.byAgent)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: 2,
              ),
              child: Text(
                'Sent',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
        ],
      ),
    );
  }
}

/// A message written here that the service hasn't confirmed: faded,
/// with where it stands under it, and Try again or Remove.
class _Outgoing extends StatelessWidget {
  const _Outgoing({
    required this.friend,
    required this.message,
    required this.onRetry,
    required this.onDiscard,
  });

  final String friend;
  final OutgoingMessage message;
  final VoidCallback onRetry;
  final VoidCallback onDiscard;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final stuck = message.state != OutgoingState.sending;
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Opacity(
            opacity: 0.6,
            child: Bubble(
              color: palette.signal,
              outgoing: true,
              child: Text(
                message.text,
                style: type.body.copyWith(color: const Color(0xFFFFFFFF)),
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(right: Space.m, top: 2),
            child: Text(
              outgoingLine(message, friend: friend),
              textAlign: TextAlign.end,
              style: type.caption.copyWith(
                color: stuck ? palette.danger : palette.textFaint,
              ),
            ),
          ),
          if (stuck)
            Padding(
              padding: const EdgeInsets.only(right: Space.m, top: Space.xs),
              // On a phone, stacked full width when they don't fit, the
              // primary last.
              child: isPhoneLayout(context)
                  ? ControlsLine(
                      children: [
                        Button(label: 'Remove', onPressed: onDiscard),
                        if (message.state == OutgoingState.unknown)
                          Button(label: 'Try again', onPressed: onRetry),
                      ],
                    )
                  : Wrap(
                      spacing: Space.s,
                      children: [
                        if (message.state == OutgoingState.unknown)
                          Button(label: 'Try again', onPressed: onRetry),
                        Button(label: 'Remove', onPressed: onDiscard),
                      ],
                    ),
            ),
        ],
      ),
    );
  }
}

/// A quiet line in the history: who can read the chat changed.
class _QuietLine extends StatelessWidget {
  const _QuietLine(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: Text(
        text,
        textAlign: TextAlign.center,
        style: type.caption.copyWith(color: palette.textMuted),
      ),
    );
  }
}

/// The friend's security key changed: what that means, and OK, which
/// lets sending go on (until then nothing is encrypted to them).
class _KeyChanged extends StatelessWidget {
  const _KeyChanged({required this.name, required this.onSeen});

  final String name;
  final VoidCallback onSeen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final who = name.isEmpty ? 'Their' : "$name's";
    final them = name.isEmpty ? 'they' : name;
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Container(
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '$who security key changed. This happens when $them sets up '
              'messages again or replaces a device. Verify '
              '${name.isEmpty ? 'them' : name} to be sure it\'s still them.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.s),
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: Button(label: 'OK', primary: true, onPressed: onSeen),
            ),
          ],
        ),
      ),
    );
  }
}
