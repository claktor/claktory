import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../avatar.dart';
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart';
import '../extensions.dart' show logTime;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../message_time.dart' show clockTime, dayLine, sameDay;
import '../mobile/composer.dart' show PhoneComposerRow;
import '../mobile/header.dart';
import '../mobile/lines.dart' show ControlsLine;
import '../mobile/metrics.dart';
import '../mobile/navigation.dart';
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../scope.dart';
import '../transcript.dart' show Bubble;
import '../user_apps/session.dart';
import '../visible_codes.dart' show nameInText, proseCodes;
import '../tunnel/api/dm.dart' as dm;
import 'chat.dart';
import 'e2ee.dart';
import 'my_identity.dart';
import 'problems.dart';
import 'review_view.dart';
import 'verify.dart';

// D3c: the chat screen: a friend's name and tag over the history, newest
// at the bottom, the person's messages on the right and theirs on the
// left, as plain text (never Markdown: it's another person's words); then
// what the person wrote here that isn't confirmed yet, each with where it
// stands; then the box to write in, whose draft is the person's on all
// their devices. An ended conversation reads the same with no box. People
// opens it: Message on a friend, Open on a past conversation. On a phone
// it's a page of its own, Back at its top and the
// phone's composer above the keyboard.

/// Who can read direct messages (dms-e2ee §7), in one
/// place: the chat's top, Add a friend and People's Friends say it.
const dmDisclosure =
    'End-to-end encrypted: only you and your friends can read these '
    'messages, on your devices and any Claktory either of you gives a chat '
    'to. '
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
/// who it's to, and [claktory] the name of the Claktory the app shows.
String outgoingLine(
  OutgoingMessage message, {
  String friend = '',
  String claktory = 'this Claktory',
}) => switch (message.state) {
  OutgoingState.sending => 'Sending…',
  OutgoingState.unknown =>
    "Couldn't confirm it was sent. Try again: it won't be sent twice.",
  OutgoingState.refused => switch (message.problem) {
    'closed' => "Not sent: you're no longer friends.",
    'stale' => 'Not sent: this conversation changed. Open it again.',
    'waitingForFriend' =>
      "Not sent: ${friend.isEmpty ? 'they' : nameInText(friend)} can't get messages "
          'in this chat yet. Send it again once they can.',
    'rateLimited' => 'Not sent: too many messages for now. Try again later.',
    'storageFull' =>
      "Not sent: you're out of space for messages. Delete some "
          'conversations to make room.',
    'unencryptable' =>
      'Not sent: ${_whose(friend)} security key changed. Read the note '
          'above, then send it again.',
    _ => 'Not sent.',
  },
  OutgoingState.waiting => switch (message.problem) {
    'homeFull' => 'Waiting for room on your Claktory. It goes once there is.',
    'rateLimited' => 'Waiting: too many messages for now. It goes soon.',
    'behind' => 'Sending again in a moment…',
    'otherAccount' =>
      'Waiting: this Claktory is signed in to another account. It goes '
          'once you connect to one of yours.',
    'unconfirmable' =>
      'Waiting: $claktory was added with an older code. Add it again with '
          'a new code.',
    _ => 'Waiting for your Claktory',
  },
};

/// The name of the Claktory the app shows; "this Claktory" where there's
/// none to say.
String shownClaktoryName(BuildContext context) {
  final backend = context.getInheritedWidgetOfExactType<AppScope>()?.backend;
  return backend == null ? 'this Claktory' : claktoryName(backend);
}

String _whose(String name) => name.isEmpty ? 'their' : "${nameInText(name)}'s";

/// What's said under the person's own message of where it stands past
/// their home; null when nothing is ([DmDelivered.sent] on any but
/// the newest, and [DmDelivered.askResend], which asks instead).
String? deliveredLine(
  ({DmDelivered state, int sinceMs}) delivered, {
  String friend = '',
  bool newest = false,
  bool byAgent = false,
  DateTime? now,
}) {
  final name = nameInText(friend);
  final claktory = "${friend.isEmpty ? 'Their' : "$name's"} Claktory";
  final to = "${friend.isEmpty ? 'their' : "$name's"} Claktory";
  return switch (delivered.state) {
    // An agent's says Sent by your agent instead.
    DmDelivered.sent => newest && !byAgent ? 'Sent' : null,
    DmDelivered.delivered => newest ? 'Delivered to $to' : null,
    DmDelivered.notYet || DmDelivered.notTaking when !newest => null,
    DmDelivered.notTaking =>
      "Not delivered yet: $to isn't taking messages from yours right now.",
    DmDelivered.notYet =>
      delivered.sinceMs > 0
          ? 'Not delivered yet: $to has been offline since '
                '${logTime(DateTime.fromMillisecondsSinceEpoch(delivered.sinceMs), now: now)}'
          : 'Not delivered yet: $to is offline.',
    DmDelivered.askResend => null,
    DmDelivered.resending => 'Sending again…',
    DmDelivered.resent => "$claktory couldn't read this one. Sent again.",
    DmDelivered.bounced => 'Not delivered to $to.',
  };
}

/// The question under the person's message the friend's Claktory
/// couldn't read, their conversation having gained someone since (never
/// sent again without asking).
String resendQuestion(String friend) {
  if (friend.isEmpty) return 'Resend? Their conversation has changed since.';
  final name = nameInText(friend);
  return "Resend to $name? $name's conversation has changed since.";
}

/// What a change in who can read a chat says, one line per leaf: [me] is
/// the person's account, [friend] the friend's name.
List<String> memberLines(
  dm.DmMembers members, {
  required String me,
  required String friend,
  String here = '',
}) {
  final who = friend.isEmpty ? 'They' : nameInText(friend);
  String line(dm.DmMember member, bool added) {
    final mine = member.account == me;
    // This device itself, removed by another of the person's:
    // never "You removed a device" with its own name. Its account and id
    // both: a friend's leaf under the same id is the friend's.
    if (!added && mine && here.isNotEmpty && member.leafId == here) {
      return 'This device was removed from your account';
    }
    if (member.kind == dm.DmLeafKind.claktory) {
      final whose = mine ? 'Your' : "$who's";
      return '$whose Claktory ${nameInText(member.name)} can '
          '${added ? 'now' : 'no longer'} read this chat';
    }
    return '${mine ? 'You' : who} ${added ? 'added' : 'removed'} a device: '
        '${nameInText(member.name)}';
  }

  return [
    // Before anything they said under it: only the friend's, in a
    // chat of two.
    for (final account in members.keyChanged)
      if (account != me)
        "${friend.isEmpty ? 'Their' : "$who's"} security key changed",
    // A friend's first device here: they joined, said for
    // each leaf all the same.
    for (final member in members.joined)
      member.account == me
          ? line(member, true)
          : '$who joined on ${nameInText(member.name)}',
    for (final member in members.added) line(member, true),
    // Back with a new key: the device installed again.
    for (final member in members.setUpAgain)
      '${member.account == me ? 'You' : who} set up a device again: '
          '${nameInText(member.name)}',
    for (final member in members.removed) line(member, false),
    // This device joined again past messages it can't read.
    if (members.skipped case final because?)
      switch (because) {
        '' =>
          "Some messages sent while this device was away can't be shown "
              'on it.',
        // Sent while a device they've since taken off their list was in
        // the chat.
        _ when because.startsWith('device:') =>
          "Some earlier messages can't be shown here: they were sent while "
              '${because == 'device:$me'
                  ? "a device you've since removed"
                  : friend.isEmpty
                  ? "a device that's since been removed"
                  : 'a device ${nameInText(friend)} has since removed'} '
              'was in this chat.',
        _ =>
          'Some messages sent when '
              '${because == me
                  ? 'your'
                  : friend.isEmpty
                  ? 'their'
                  : "$who's"} '
              "security key changed can't be shown on this device.",
      },
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
    final name = nameInText(_thread.conversation.person.name);
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
              // Listed by the service, not here yet from the agent's
              // Claktory: never its text until it is.
              if (_thread.arriving > 0)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs),
                  child: Text(
                    _thread.arriving == 1
                        ? "An agent's suggestion is on its way."
                        : '${_thread.arriving} suggestions from agents are on their way.',
                    style: type.caption.copyWith(color: palette.textFaint),
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
              if (_thread.held)
                _Held(onRejoin: _thread.heldLong ? _thread.rejoin : null)
              else if (_thread.behind)
                const _Behind(),
              if (_thread.waitingFor case final account?)
                _WaitingForKey(
                  name: account == _thread.e2ee.accountId
                      ? null
                      : _thread.conversation.person.name,
                  device: _thread.reinstalling,
                ),
              // Their key changed: said as such, never as waiting for them
              // to join, which their devices under it can't (E11-3).
              if (_thread.waitingFor == null &&
                  _thread.joining != null &&
                  !_thread.keyChanged &&
                  !_thread.keyRefused)
                _WaitingToJoin(name: _thread.conversation.person.name),
              if (_thread.keyChanged)
                _KeyChanged(
                  name: _thread.conversation.person.name,
                  onSeen: _thread.keySeen,
                  onVerify: _verify,
                  mismatched: _thread.keyMismatched,
                  wasVerified: _thread.e2ee.wasVerified.contains(
                    _thread.friend,
                  ),
                )
              else if (_thread.keyRefused)
                _KeyChanged(
                  name: _thread.conversation.person.name,
                  onVerify: _verify,
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
                    here: _thread.e2ee.leafId,
                  ))
                    _QuietLine(line)
                else
                  _Message(
                    message: message,
                    friend: _thread.conversation.person.name,
                    // The newest, the person's own: it came back.
                    newest:
                        message.mine &&
                        i == _thread.messages.length - 1 &&
                        _thread.outgoing.isEmpty,
                    delivered: message.mine ? _thread.delivered(message) : null,
                    onResend: () => _thread.resend(message),
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
        // Signed out (removed by another of the person's devices, say):
        // why nothing can be sent, over what was typed.
        if (_thread.writable &&
            (_thread.e2ee.setup == DmSetup.signedOut ||
                problem?.problem == UserAppProblem.notSignedIn))
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.l),
            child: ListenableBuilder(
              listenable: AppScope.scope(context).connections.account,
              builder: (context, _) => Text(
                AppScope.scope(context).connections.account.removed
                    ? deviceRemovedText
                    : problemText(
                        const UserAppError(UserAppProblem.notSignedIn),
                      ),
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          )
        else if (problem != null)
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
  /// Verify, for the friend: their security code, to compare or scan.
  Future<void> _verify() async {
    await showVerify(
      context,
      _thread.e2ee,
      account: _thread.friend,
      name: _thread.conversation.person.name,
    );
    if (mounted) setState(() {});
  }

  /// Whether Verify can open: the friend's key is pinned here.
  bool get _canVerify =>
      _thread.friend.isNotEmpty && _thread.e2ee.pin(_thread.friend) != null;

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
          if (_thread.verified)
            TextSpan(
              text: ' · Verified',
              style: TextStyle(color: palette.signal),
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
          if (_canVerify && !_thread.verified) ...[
            Button(label: 'Verify…', onPressed: _verify),
            const SizedBox(width: Space.s),
          ],
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
                if (person.tag.isNotEmpty || _thread.verified)
                  Text(
                    [
                      if (person.tag.isNotEmpty) person.tag,
                      if (_thread.verified) 'Verified',
                    ].join(' · '),
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
              ],
            ),
          ),
        ],
      ),
      trailing: [
        if (_thread.messages.isNotEmpty || _canVerify)
          HeaderButton.more(
            onTap: () async {
              final chosen = await showActions<String>(
                context,
                title: person.label,
                actions: [
                  if (_canVerify) const SheetAction('Verify…', 'verify'),
                  if (_thread.messages.isNotEmpty)
                    const SheetAction(
                      'Delete conversation…',
                      'delete',
                      destructive: true,
                    ),
                ],
              );
              if (chosen == 'delete') await _delete();
              if (chosen == 'verify') await _verify();
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
  const _Message({
    required this.message,
    this.friend = '',
    this.newest = false,
    this.delivered,
    this.onResend,
  });

  final DmMessage message;
  final String friend;

  /// The person's newest, which came back: says Sent, or Delivered to
  /// their friend's Claktory. Never more than that: nothing says
  /// whether they read it.
  final bool newest;

  /// Where the person's own stands past their home; null for the friend's.
  final ({DmDelivered state, int sinceMs})? delivered;
  final VoidCallback? onResend;

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
            sentMs: message.shownMs,
            timeColor: mine ? const Color(0xB3FFFFFF) : null,
            // Theirs, or an agent's: what reorders text shows by its code
            // (proseCodes). The person's own, as they wrote it.
            child: Text(
              mine && !message.byAgent
                  ? message.text
                  : proseCodes(message.text),
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
          // From a device since taken off its person's list, read because
          // one of theirs that was there all along took it out.
          if (message.departed)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: 2,
              ),
              child: Text(
                departedLine(mine: mine, friend: friend),
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          // Held up on the way (a Claktory offline, say): when it came too
          // .
          if (message.arrivedLate case final arrived?)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: 2,
              ),
              child: Text(
                arrivedLine(
                  message.shownMs,
                  arrived,
                  DateTime.now(),
                  h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
                ),
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          if (delivered case final delivered?)
            if (deliveredLine(
                  delivered,
                  friend: friend,
                  newest: newest,
                  byAgent: message.byAgent,
                )
                case final line?)
              Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.m,
                  vertical: 2,
                ),
                child: Text(
                  line,
                  textAlign: TextAlign.end,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ),
          if (delivered?.state == DmDelivered.askResend) ...[
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: 2,
              ),
              child: Text(
                resendQuestion(friend),
                textAlign: TextAlign.end,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
            Padding(
              padding: const EdgeInsets.only(right: Space.m, top: Space.xs),
              child: Button(label: 'Resend', onPressed: onResend),
            ),
          ],
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
    // Waiting for the person's home goes by itself: said quietly, and
    // only Remove.
    final waiting = message.state == OutgoingState.waiting;
    final stuck = message.state != OutgoingState.sending && !waiting;
    final actions = stuck || waiting;
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
          if (message.agentName.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(right: Space.m, top: 2),
              child: Text(
                'Written by your agent',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          Padding(
            padding: const EdgeInsets.only(right: Space.m, top: 2),
            child: Text(
              outgoingLine(
                message,
                friend: friend,
                claktory: shownClaktoryName(context),
              ),
              textAlign: TextAlign.end,
              style: type.caption.copyWith(
                color: stuck ? palette.danger : palette.textFaint,
              ),
            ),
          ),
          if (actions)
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
/// The feed waits at a change made under a security key this device
/// doesn't have yet (held, never dropped); after a day,
/// Join again.
class _Held extends StatelessWidget {
  const _Held({this.onRejoin});

  final Future<void> Function()? onRejoin;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
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
              'Waiting for the new security key to reach this device.',
              style: type.body.copyWith(color: palette.text),
            ),
            if (onRejoin case final rejoin?) ...[
              const SizedBox(height: Space.xs),
              Text(
                'Still waiting after a day. Join again to carry on; messages '
                'sent meanwhile may not show here.',
                style: type.caption.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.s),
              Align(
                alignment: AlignmentDirectional.centerEnd,
                child: Button(
                  label: 'Join again',
                  primary: true,
                  onPressed: () => unawaited(rejoin()),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// When a message held up on the way arrived, under it:
/// "Arrived 06:56", or with its day when that isn't the day it was sent
/// ("Arrived yesterday at 23:10", "Arrived Monday at 09:02").
String arrivedLine(int sentMs, int arrivedMs, DateTime now, {bool h24 = true}) {
  final time = clockTime(arrivedMs, h24: h24);
  if (sameDay(sentMs, arrivedMs)) return 'Arrived $time';
  final day = switch (dayLine(arrivedMs, now)) {
    'Today' => 'today',
    'Yesterday' => 'yesterday',
    final other => other,
  };
  return 'Arrived $day at $time';
}

/// Under a message from a device its person has since removed:
/// [mine] when that's the person, [friend] the friend's name.
String departedLine({required bool mine, required String friend}) => mine
    ? "From a device you've since removed"
    : friend.isEmpty
    ? "From a device they've since removed"
    : 'From a device ${nameInText(friend)} has since removed';

/// A message waits until this device catches up with the chat:
/// Claktory tries again by itself.
class _Behind extends StatelessWidget {
  const _Behind();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
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
              'A message is on its way.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.xs),
            Text(
              'It shows here once this device catches up. Claktory keeps '
              'trying.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ),
      ),
    );
  }
}

/// The friend isn't in the conversation yet: nothing is sent
/// until a device of theirs is. [name] is the friend's.
class _WaitingToJoin extends StatelessWidget {
  const _WaitingToJoin({required this.name});

  final String name;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final who = name.isEmpty ? 'your friend' : nameInText(name);
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
              'Waiting for $who to join this chat.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.xs),
            Text(
              "You can send messages here once they have. It happens when "
              'Claktory is open on one of their devices.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ),
      ),
    );
  }
}

/// Someone started over on a new security key: the
/// group waits until their new device takes the devices under the old
/// one out, and sending waits with it. [name] is the friend's; null for
/// the person's own.
class _WaitingForKey extends StatelessWidget {
  const _WaitingForKey({this.name, this.device});

  final String? name;

  /// A reinstalled device's name, when that's what it waits for (row
  /// 202); null for a new security key.
  final String? device;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final whose = switch (name) {
      null => 'your',
      '' => 'their',
      final name => "${nameInText(name)}'s",
    };
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
            Text(switch (device) {
              null => 'Waiting for $whose new security key to finish.',
              final device =>
                'Waiting for $whose ${nameInText(device)} to finish setting '
                    'up again.',
            }, style: type.body.copyWith(color: palette.text)),
            const SizedBox(height: Space.xs),
            Text(
              'You can send messages here again once it has.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ),
      ),
    );
  }
}

/// The friend's key changed: one to see ([onSeen], OK), or one this device
/// refused to take ([onSeen] null, E11-3), which only Verify can look into.
class _KeyChanged extends StatelessWidget {
  const _KeyChanged({
    required this.name,
    this.onSeen,
    required this.onVerify,
    this.wasVerified = false,
    this.mismatched = false,
  });

  final String name;
  final VoidCallback? onSeen;
  final VoidCallback onVerify;

  /// They were verified before: they no longer are.
  final bool wasVerified;

  /// It's a key the person said didn't match before (E11-3).
  final bool mismatched;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // Their name inside these sentences (nameInText).
    final name = nameInText(this.name);
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
              mismatched && onSeen != null
                  ? '$who security key changed back to the one that didn\'t '
                        'match when you compared numbers. Verify '
                        '${name.isEmpty ? 'them' : name} before you send '
                        'anything.'
                  : onSeen == null
                  ? '$who security key changed to one this device can\'t '
                        'accept, so nothing is sent to '
                        '${name.isEmpty ? 'them' : name}. Verify '
                        '${name.isEmpty ? 'them' : name} to be sure it\'s '
                        'still them.'
                  : '$who security key changed. This happens when $them '
                        '${name.isEmpty ? 'set up' : 'sets up'} messages again '
                        'or ${name.isEmpty ? 'replace' : 'replaces'} a device. '
                        'Verify ${name.isEmpty ? 'them' : name} to be sure '
                        'it\'s still them.',
              style: type.body.copyWith(color: palette.text),
            ),
            if (wasVerified && !mismatched) ...[
              const SizedBox(height: Space.xs),
              Text(
                '$who security key changed, so ${name.isEmpty ? 'they are' : '$name is'} '
                'no longer verified.',
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ],
            const SizedBox(height: Space.s),
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: Wrap(
                spacing: Space.s,
                children: [
                  Button(label: 'Verify…', onPressed: onVerify),
                  if (onSeen case final seen?)
                    Button(label: 'OK', primary: true, onPressed: seen),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
