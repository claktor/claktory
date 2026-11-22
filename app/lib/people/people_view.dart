import 'dart:async';

import 'package:flutter/widgets.dart';

import '../avatar.dart';
import '../controls.dart';
import '../forms.dart';
import '../identity.dart';
import '../message_time.dart' show dayLine;

import '../mobile/metrics.dart';
import '../mobile/navigation.dart' show PhoneRoutes;
import '../mobile/presentation.dart';
import '../protocol/factory/v1/factory.pb.dart' show Workspace;
import '../user_apps/session.dart';
import '../your_claktories.dart' show DeviceSignInDialog;
import 'chat.dart';
import 'chat_view.dart';
import 'contacts.dart';
import 'e2ee.dart' show e2eeOf;
import 'messages_setup.dart';
import 'my_friend_id.dart';
import 'my_identity.dart';
import 'problems.dart';
import 'report_form.dart';
import 'review.dart';

// D3b: People's friend flow, before any messages: friend requests that
// reached the person (Accept, Decline, Block), their friends (Remove,
// Block), the requests they sent (Cancel), whom they blocked (Unblock),
// and Add a friend with a friend ID. Everyone shows with their tag, so
// two of the same name can't pass for each other. It opens from the
// sidebar (the drawer on a phone) only where [peopleAvailable].

/// Whether People shows: only where the backend offers direct messages
/// (its preview, FACTORY_DMS_PREVIEW, until People ships), which it says
/// by listing their extension.
bool peopleAvailable(Workspace workspace) =>
    workspace.extensions.any((extension) => extension.id == 'dm');

/// Opens People.
Future<void> showPeople(BuildContext context) => showModal(
  context,
  (sheet, close) => PeopleView(
    close: close,
    // On a phone a chat is a page, and pages sit under sheets: People
    // closes first, and the chat opens from where People was opened.
    openChat: PhoneRoutes.of(context) == null
        ? null
        : (conversation) {
            close();
            unawaited(showChat(context, conversation));
          },
  ),
);

/// Stands in for the account service in tests.
ContactsApi Function(BuildContext context)? contactsApi;

ContactsApi _contacts(BuildContext context) =>
    contactsApi?.call(context) ?? ContactsApi(UserAppsSession.of(context));

class PeopleView extends StatefulWidget {
  const PeopleView({super.key, required this.close, this.openChat});

  final VoidCallback close;

  /// Opens a chat in People's place (a phone); null: over People.
  final void Function(DmConversation conversation)? openChat;

  @override
  State<PeopleView> createState() => _PeopleViewState();
}

class _PeopleViewState extends State<PeopleView> {
  late final ContactsApi _api = _contacts(context);
  late final ChatApi _chats =
      chatApi?.call(context) ?? ChatApi(UserAppsSession.of(context));
  PeopleLists? _lists;

  /// Conversations with people who are no longer friends, still readable.
  var _ended = const <DmConversation>[];

  /// What agents proposed sending, waiting for the person, and the replies
  /// they drafted (without their text).
  var _proposals = const <DmProposal>[];
  var _drafts = const <DmAgentDraft>[];
  String? _error;
  var _busy = false;

  /// This device isn't signed in to the account: why, and Sign in….
  var _signedOut = false;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final lists = await _api.lists();
      if (mounted) {
        setState(() {
          _lists = lists;
          _signedOut = false;
          _error = null;
        });
      }
    } on UserAppError catch (error) {
      if (mounted) {
        setState(() {
          _signedOut = error.problem == UserAppProblem.notSignedIn;
          _error = _signedOut ? null : problemText(error);
        });
      }
      return;
    }
    try {
      final snapshot = await _chats.snapshot();
      if (mounted) {
        setState(() {
          _ended = [
            for (final conversation in snapshot.conversations)
              if (!conversation.active) conversation,
          ];
          _proposals = [
            for (final proposal in snapshot.proposals)
              if (proposal.pending) proposal,
          ];
          _drafts = snapshot.agentDrafts;
        });
      }
    } on UserAppError {
      // Past conversations and what waits only; the lists above stand on
      // their own.
    }
  }

  /// Signs this device in to the account, the way the rest of the app
  /// does, then reads People again.
  Future<void> _signIn() async {
    await showModal(
      context,
      (context, close) => DeviceSignInDialog(
        title: 'Sign in to use People',
        intro:
            'People uses your Claktory account, so your friends and messages '
            'are the same on all your devices.',
        close: close,
      ),
    );
    if (mounted) await _load();
  }

  /// Opens [conversation]: in People's place on a phone, over it elsewhere
  /// (and, closed, what waits is read again).
  Future<void> _open(DmConversation conversation) async {
    final open = widget.openChat;
    if (open != null) return open(conversation);
    await showChat(context, conversation);
    if (mounted) await _load();
  }

  /// Opens the conversation with [friend].
  Future<void> _message(Friend friend) => _open(
    DmConversation(
      id: friend.conversationId,
      contactId: friend.id,
      generation: friend.generation,
      person: friend.person,
      active: true,
      sinceMs: friend.sinceMs,
      endedMs: 0,
      lastSeq: 0,
    ),
  );

  /// Runs [request], then reads the lists again (what changed, and what
  /// another device changed meanwhile), whether it worked or not.
  Future<void> _run(Future<void> Function() request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      await request();
    } on UserAppError catch (failure) {
      error = problemText(failure);
    }
    await _load();
    if (mounted) {
      setState(() {
        _busy = false;
        if (error != null) _error = error;
      });
    }
  }

  /// Asks [title]'s question, with [body] under it; true for [action].
  Future<bool> _confirm(String title, String body, String action) async {
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: title,
        body: body,
        fields: const [],
        error: null,
        submitLabel: action,
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    return sure && mounted;
  }

  Future<void> _block(
    PeoplePerson person, {
    String? request,
    String? contact,
  }) async {
    if (!await _confirm(
      'Block ${person.label}?',
      "They can't send you friend requests anymore, and if you're friends, "
          "you won't be. Unblocking later doesn't make you friends again.",
      'Block',
    )) {
      return;
    }
    await _run(() => _api.block(requestId: request, contactId: contact));
  }

  Future<void> _remove(Friend friend) async {
    if (!await _confirm(
      'Remove ${friend.person.label}?',
      'You stop being friends. Either of you can send a new request later.',
      'Remove',
    )) {
      return;
    }
    await _run(() => _api.remove(friend.id));
  }

  Future<void> _friendActions(BuildContext context, Friend friend) async {
    final chosen = await showActions<String>(
      context,
      title: friend.person.label,
      actions: const [
        SheetAction('Remove friend…', 'remove'),
        SheetAction('Report…', 'report'),
        SheetAction('Block…', 'block', destructive: true),
      ],
    );
    switch (chosen) {
      case 'remove':
        await _remove(friend);
      case 'report':
        await _report(
          friend.person,
          contact: friend.id,
          conversation: friend.conversationId,
        );
      case 'block':
        await _block(friend.person, contact: friend.id);
    }
  }

  /// Reports [person] to Claktory's team, then reads the lists again (the
  /// report joins Your reports).
  Future<void> _report(
    PeoplePerson person, {
    String? request,
    String? contact,
    String? block,
    String conversation = '',
  }) async {
    await showModal(
      context,
      (context, close) => ReportForm(
        api: _api,
        person: person,
        requestId: request,
        contactId: contact,
        blockId: block,
        conversationId: conversation,
        close: close,
      ),
    );
    if (mounted) await _load();
  }

  Future<void> _add() async {
    await showModal(
      context,
      (context, close) => AddFriendForm(api: _api, close: close),
    );
    if (mounted) await _load();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final lists = _lists;
    final now = DateTime.now();
    return FormFrame(
      title: 'People',
      fields: [
        if (_signedOut) ...[
          Text(
            problemText(const UserAppError(UserAppProblem.notSignedIn)),
            style: type.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(label: 'Sign in…', primary: true, onPressed: _signIn),
          ),
        ] else ...[
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'My friend ID…',
              onPressed: () => showMyFriendId(context),
            ),
          ),
          const SizedBox(height: Space.l),
          MessagesSetup(e2ee: e2eeOf(context)),
        ],
        if (lists == null && _error == null && !_signedOut)
          Text(
            'Getting your friends…',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        if (lists != null) ...[
          if (lists.incoming.isNotEmpty)
            Field(
              label: 'Friend requests',
              child: Column(
                children: [
                  for (final request in lists.incoming)
                    PersonRow(
                      person: request.from,
                      caption:
                          'Wants to be friends · '
                          '${_dayIn(request.createdMs, now)}',
                      trailing: [
                        Button(
                          label: 'Accept',
                          primary: true,
                          onPressed: _busy
                              ? null
                              : () => _run(() => _api.accept(request.id)),
                        ),
                        Button(
                          label: 'Decline',
                          onPressed: _busy
                              ? null
                              : () => _run(() => _api.decline(request.id)),
                        ),
                        _More(
                          label: 'More for ${request.from.label}',
                          onTap: () async {
                            final chosen = await showActions<String>(
                              context,
                              title: request.from.label,
                              actions: const [
                                SheetAction('Report…', 'report'),
                                SheetAction(
                                  'Block…',
                                  'block',
                                  destructive: true,
                                ),
                              ],
                            );
                            switch (chosen) {
                              case 'report':
                                await _report(
                                  request.from,
                                  request: request.id,
                                );
                              case 'block':
                                await _block(request.from, request: request.id);
                            }
                          },
                        ),
                      ],
                    ),
                ],
              ),
            ),
          Field(
            label: 'Friends',
            note: dmDisclosure,
            child: lists.friends.isEmpty
                ? Text(
                    'No friends yet. Give someone your friend ID, or add '
                    'theirs.',
                    style: type.body.copyWith(color: palette.textMuted),
                  )
                : Column(
                    children: [
                      for (final friend in lists.friends)
                        Builder(
                          builder: (context) {
                            final waiting = waitingCaption(
                              _proposals,
                              friend.conversationId,
                              drafts: _drafts,
                            );
                            return PersonRow(
                              person: friend.person,
                              caption:
                                  waiting ??
                                  'Friends since ${_dayIn(friend.sinceMs, now)}',
                              waiting: waiting != null,
                              onSecondaryTap: () =>
                                  _friendActions(context, friend),
                              trailing: [
                                Button(
                                  label: 'Message',
                                  onPressed: friend.conversationId.isEmpty
                                      ? null
                                      : () => _message(friend),
                                ),
                                _More(
                                  label: 'More for ${friend.person.label}',
                                  onTap: () => _friendActions(context, friend),
                                ),
                              ],
                            );
                          },
                        ),
                    ],
                  ),
          ),
          if (lists.sent.isNotEmpty)
            Field(
              label: 'Requests you sent',
              note:
                  "You'll see them among your friends if they accept. "
                  'Requests expire after 30 days.',
              child: Column(
                children: [
                  for (final sent in lists.sent)
                    _SentRow(
                      caption: 'Request sent ${_dayIn(sent.createdMs, now)}',
                      onCancel: _busy
                          ? null
                          : () => _run(() => _api.cancel(sent.id)),
                    ),
                ],
              ),
            ),
          if (lists.declined.isNotEmpty)
            Field(
              label: 'Declined',
              note:
                  "Declining doesn't tell them. Their requests can reach "
                  'you again after 30 days, or now if you reopen.',
              child: Column(
                children: [
                  for (final declined in lists.declined)
                    PersonRow(
                      person: declined.person,
                      caption:
                          'Can send requests again from '
                          '${_dayIn(declined.untilMs, now)}',
                      trailing: [
                        Button(
                          label: 'Reopen',
                          onPressed: _busy
                              ? null
                              : () => _run(() => _api.reopen(declined.id)),
                        ),
                      ],
                    ),
                ],
              ),
            ),
          if (lists.blocked.isNotEmpty)
            Field(
              label: 'Blocked',
              child: Column(
                children: [
                  for (final blocked in lists.blocked)
                    PersonRow(
                      person: blocked.person,
                      caption: 'Blocked ${_dayIn(blocked.createdMs, now)}',
                      trailing: [
                        Button(
                          label: 'Unblock',
                          onPressed: _busy
                              ? null
                              : () => _run(() => _api.unblock(blocked.id)),
                        ),
                        _More(
                          label: 'More for ${blocked.person.label}',
                          onTap: () async {
                            final chosen = await showActions<String>(
                              context,
                              title: blocked.person.label,
                              actions: const [SheetAction('Report…', 'report')],
                            );
                            if (chosen == 'report') {
                              await _report(blocked.person, block: blocked.id);
                            }
                          },
                        ),
                      ],
                    ),
                ],
              ),
            ),
          if (_ended.isNotEmpty)
            Field(
              label: 'Past conversations',
              note:
                  "You're no longer friends, but you can still read what "
                  'you wrote to each other.',
              child: Column(
                children: [
                  for (final conversation in _ended)
                    PersonRow(
                      person: conversation.person,
                      caption: 'Ended ${_dayIn(conversation.endedMs, now)}',
                      trailing: [
                        Button(
                          label: 'Open',
                          onPressed: () => _open(conversation),
                        ),
                      ],
                    ),
                ],
              ),
            ),
          MessagesSetup(e2ee: e2eeOf(context), devices: true),
          if (lists.reports.isNotEmpty)
            Field(
              label: 'Your reports',
              note: 'Kept for 30 days. Only you and Claktory’s team see them.',
              child: Column(
                children: [
                  for (final report in lists.reports)
                    PersonRow(
                      person: report.about,
                      caption: [
                        _reasonLabel(report.reason),
                        'sent ${_dayIn(report.createdMs, now)}',
                        report.reviewed ? 'Reviewed' : 'Received',
                      ].join(' · '),
                    ),
                ],
              ),
            ),
        ],
      ],
      error: _error,
      submitLabel: 'Add a friend…',
      onSubmit: _busy || _signedOut ? null : _add,
      cancelLabel: 'Done',
      onCancel: widget.close,
    );
  }
}

/// What waits for the person in [conversation] (Trish's words): "June
/// wants to send a message", or "2 messages waiting"; with none, an
/// agent's reply drafted there, "June suggests a reply" or "2 replies
/// suggested"; null for nothing.
String? waitingCaption(
  List<DmProposal> proposals,
  String conversation, {
  List<DmAgentDraft> drafts = const [],
}) {
  if (conversation.isEmpty) return null;
  // Its Claktory's own words: never with a character that hides.
  String agent(String name) =>
      name.trim().isEmpty || hasHidden(name) ? 'Your agent' : name;
  final here = [
    for (final proposal in proposals)
      if (proposal.pending && proposal.conversationId == conversation) proposal,
  ];
  if (here.length > 1) return '${here.length} messages waiting';
  if (here.length == 1) {
    return '${agent(here.single.agentName)} wants to send a message';
  }
  final drafted = [
    for (final draft in drafts)
      if (draft.conversationId == conversation) draft,
  ];
  if (drafted.length > 1) return '${drafted.length} replies suggested';
  if (drafted.length == 1) {
    return '${agent(drafted.single.agentName)} suggests a reply';
  }
  return null;
}

/// Someone in People: their avatar, their name with its tag (the tag in
/// `textFaint`), a caption under it, and its actions at the end.
class PersonRow extends StatelessWidget {
  const PersonRow({
    super.key,
    required this.person,
    required this.caption,
    this.trailing = const [],
    this.onSecondaryTap,
    this.waiting = false,
  });

  final PeoplePerson person;
  final String caption;

  /// [caption] says something waits for the person there: in `signal`.
  final bool waiting;
  final List<Widget> trailing;
  final VoidCallback? onSecondaryTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // On a phone, two or more actions go under the words, a finger's
    // size each, rather than squeezing the name out.
    final below = trailing.length > 1 && isPhoneLayout(context);
    final words = Semantics(
      label: '${person.label}, $caption',
      excludeSemantics: true,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: person.name.isEmpty ? 'No name' : person.name,
                  style: TextStyle(
                    color: person.name.isEmpty
                        ? palette.textMuted
                        : palette.text,
                  ),
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
            style: type.body,
          ),
          Text(
            caption,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: type.caption.copyWith(
              color: waiting ? palette.signal : palette.textMuted,
              fontWeight: waiting ? FontWeight.w500 : null,
            ),
          ),
        ],
      ),
    );
    final row = Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      child: Row(
        crossAxisAlignment: below
            ? CrossAxisAlignment.start
            : CrossAxisAlignment.center,
        children: [
          PersonAvatar(
            person: peopleAvatarOf(person.name, person.avatarSeed),
            size: 28,
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: below
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      words,
                      const SizedBox(height: Space.s),
                      Wrap(
                        spacing: Space.s,
                        runSpacing: Space.s,
                        children: trailing,
                      ),
                    ],
                  )
                : words,
          ),
          if (!below)
            for (final action in trailing) ...[
              const SizedBox(width: Space.s),
              action,
            ],
        ],
      ),
    );
    if (onSecondaryTap == null) return row;
    return GestureDetector(onSecondaryTap: onSecondaryTap, child: row);
  }
}

class _SentRow extends StatelessWidget {
  const _SentRow({required this.caption, required this.onCancel});

  final String caption;
  final VoidCallback? onCancel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      child: Row(
        children: [
          Expanded(
            child: Text(
              caption,
              style: TypeScale.of(context).body.copyWith(color: palette.text),
            ),
          ),
          Button(label: 'Cancel', onPressed: onCancel),
        ],
      ),
    );
  }
}

class _More extends StatelessWidget {
  const _More({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => GlyphButton(
    label: label,
    active: false,
    onTap: onTap,
    glyph: (color) => ExtensionGlyph(icon: 'ellipsis', name: '…', color: color),
  );
}

/// Add a friend: their friend ID, pasted or typed (a link works too), and
/// what they'll see of the person with it; then the service's own words
/// for what happened, which say nothing about whose ID it was.
class AddFriendForm extends StatefulWidget {
  const AddFriendForm({super.key, required this.api, required this.close});

  final ContactsApi api;
  final VoidCallback close;

  @override
  State<AddFriendForm> createState() => _AddFriendFormState();
}

class _AddFriendFormState extends State<AddFriendForm> {
  final _code = TextEditingController();
  MyIdentity? _me;
  String? _error;
  String? _sent;
  var _busy = false;

  /// This request's id, kept for its retries; a different friend ID is a
  /// different request.
  String? _actionId;
  String _actionFor = '';

  @override
  void initState() {
    super.initState();
    _code.addListener(() => setState(() {}));
    unawaited(_loadMe());
  }

  Future<void> _loadMe() async {
    try {
      final me = await MyIdentityApi(widget.api.session).me();
      if (mounted) setState(() => _me = me);
    } on UserAppError {
      // The preview is only a courtesy; sending says what's wrong.
    }
  }

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    final code = friendIdFrom(_code.text);
    if (code.isEmpty) return;
    if (_actionId == null || _actionFor != code) {
      _actionId = newActionId();
      _actionFor = code;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final message = await widget.api.send(code, _actionId!);
      if (mounted) {
        setState(
          () => _sent = message.isNotEmpty
              ? message
              : "Request sent. If that's their friend ID, they'll see it.",
        );
      }
    } on UserAppError catch (error) {
      if (mounted) setState(() => _error = problemText(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final sent = _sent;
    if (sent != null) {
      return FormFrame(
        title: 'Add a friend',
        fields: [
          Text(sent, style: type.body.copyWith(color: palette.text)),
          const SizedBox(height: Space.l),
        ],
        error: null,
        submitLabel: 'Done',
        onSubmit: widget.close,
        onCancel: widget.close,
      );
    }
    final me = _me;
    return FormFrame(
      title: 'Add a friend',
      body:
          'Paste or type their friend ID. They see your name and picture '
          'with your request, nothing else. $dmDisclosure',
      dirty: _code.text.isNotEmpty,
      fields: [
        Field(
          label: 'Friend ID',
          child: TextField(
            controller: _code,
            autofocus: true,
            placeholder: 'Paste their friend ID',
            semanticsLabel: 'Friend ID',
            autocorrect: false,
            onSubmitted: (_) => _send(),
          ),
        ),
        if (me != null)
          Field(
            label: 'They see',
            child: Row(
              children: [
                PersonAvatar(person: peopleAvatar(me), size: 28),
                const SizedBox(width: Space.m),
                Expanded(
                  child: Text(
                    me.name.isEmpty ? 'No name yet' : me.name,
                    style: type.body.copyWith(
                      color: me.name.isEmpty ? palette.textFaint : palette.text,
                    ),
                  ),
                ),
              ],
            ),
          ),
      ],
      error: _error,
      submitLabel: _busy ? 'Sending…' : 'Send request',
      onSubmit: _busy || friendIdFrom(_code.text).isEmpty ? null : _send,
      onCancel: widget.close,
    );
  }
}

/// A report's reason as the service names it, in People's words.
String _reasonLabel(String reason) =>
    switch (ReportReason.values.where((r) => r.name == reason).firstOrNull) {
      final known? => reportReasonLabel(known),
      null => 'Reported',
    };

/// [ms]'s day for the middle of a line: "today", "yesterday", "Monday",
/// "Mon 17 Nov".
String _dayIn(int ms, DateTime now) {
  final day = dayLine(ms, now);
  return day == 'Today' || day == 'Yesterday' ? day.toLowerCase() : day;
}
