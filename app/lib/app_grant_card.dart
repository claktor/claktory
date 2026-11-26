import 'dart:async';

import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'card_answer.dart' show CardAnswer, PublishesAnswer, noComment;
import 'decision.dart' show CommentTyped, DecisionComment, SubmitButton;
import 'extensions.dart' show dayLabel, logTime;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'people/contacts.dart';
import 'people/people_view.dart' show contactsApi;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'tool_views.dart' show ShowMore;
import 'user_apps/session.dart';
import 'visible_codes.dart' show proseCodes;

/// What the card and Needs you ask, in a line: "Give Mike access to Direct
/// messages?".
String appGrantSummary(AppGrantProposal grant, Workspace workspace) =>
    'Give ${_agentName(grant, workspace)} access to ${grant.appName}?';

String _agentName(AppGrantProposal grant, Workspace workspace) =>
    workspace.agents
        .where((agent) => agent.id == grant.agentId)
        .firstOrNull
        ?.name ??
    grant.agentId;

/// The scope that lets the agent send messages as the person.
const appGrantSend = 'dm.send';

/// What the warning says when the agent it would give access to has read
/// a web page (grantee_read_pipes, agents_read_pipes: since #346 only a
/// page read through browser control marks an agent); more when it could
/// send as the person.
String appGrantReadPipes(String name, {required bool sends}) => sends
    ? '$name has read a web page, which may hold someone else\'s '
          'instructions. Whoever wrote it could try to get $name to send '
          'messages as you. Approve only if you expected this.'
    : '$name has read a web page, which may hold someone else\'s '
          'instructions.';

/// Whether the app gave [resources] names of their own: until it does,
/// each label is only its id, which the person never sees.
bool appGrantNamed(List<AppGrantResource> resources) => resources.every(
  (resource) => resource.label.isNotEmpty && resource.label != resource.id,
);

/// A resource that is one of the person's conversations, named on this
/// device from People's friends, never from the card (Fugo F1).
const appGrantConversation = 'conversation';

/// A conversation the card would reach that isn't one of the person's
/// friends' on this device.
const appGrantUnknownConversation = "A conversation this device can't find";

/// A conversation the card would reach, while People's friends can't be
/// read: maybe the person's, maybe not, so neither named nor called
/// unknown (Abbacchio).
const appGrantUncheckedConversation =
    "A conversation that can't be checked right now";

/// Each conversation [resources] reach, by whose it is on this device
/// ([friends]: conversation id to "Ana · K7QF2M"), and whether every one
/// is known: until then, Approve is off.
({List<String> lines, bool known}) appGrantConversations(
  List<AppGrantResource> resources,
  Map<String, String> friends,
) {
  final lines = [
    for (final resource in resources)
      if (resource.kind == appGrantConversation)
        friends[resource.id] ?? appGrantUnknownConversation,
  ];
  return (lines: lines, known: !lines.contains(appGrantUnknownConversation));
}

/// What a grant reaches, counted when the app gives no names: "In 2
/// conversations".
String appGrantReach(List<AppGrantResource> resources) {
  final counts = <String, int>{};
  for (final resource in resources) {
    final kind = resource.kind.isEmpty ? 'item' : resource.kind;
    counts[kind] = (counts[kind] ?? 0) + 1;
  }
  final parts = [
    for (final MapEntry(key: kind, value: n) in counts.entries)
      '$n $kind${n == 1 ? '' : 's'}',
  ];
  return 'In ${parts.join(' and ')}';
}

/// When the access would end: "Until you take it back." or "Until Mon 17
/// Nov, 14:32." ("Until today, 14:32." on the day).
String appGrantUntil(int expiresMs, {DateTime? now}) {
  if (expiresMs <= 0) return 'Until you take it back.';
  now ??= DateTime.now();
  final at = DateTime.fromMillisecondsSinceEpoch(expiresMs);
  final day = dayLabel(at, now);
  final clock =
      '${at.hour.toString().padLeft(2, '0')}:'
      '${at.minute.toString().padLeft(2, '0')}';
  final named = day == 'Today' || day == 'Yesterday' ? day.toLowerCase() : day;
  return 'Until $named, $clock.';
}

/// Rivet's proposal to give an agent access to an app kept at the person's
/// claktory.com account, such as their direct messages
/// (AppGrantProposal): what the agent could do there and reach, for how
/// long and why (in Rivet's words), with Decline (and a comment for Rivet,
/// if they like) and Approve. Approved, the account makes the grant before
/// the backend answers, so the card says it's on its way meanwhile; while
/// the account can't be reached, the refusal shows and it can be approved
/// again. A sibling of the proposal card, in Rivet's color. The backend
/// sends the entry back decided; the card keeps nothing but the comment
/// being typed.
class AppGrantCard extends StatefulWidget {
  const AppGrantCard(
    this.grant, {
    super.key,
    required this.agentId,
    required this.seq,
  });

  final AppGrantProposal grant;

  /// Whose conversation shows it: Rivet's.
  final String agentId;

  /// The entry's seq in that conversation.
  final int seq;

  @override
  State<AppGrantCard> createState() => _AppGrantCardState();
}

class _AppGrantCardState extends State<AppGrantCard> with PublishesAnswer {
  final _comment = TextEditingController();

  /// Whose each conversation it reaches is, from People's friends on this
  /// device (Fugo F1): null while they load; [_friendsFailed] when they
  /// couldn't be read, so none is known.
  Map<String, String>? _friends;
  var _friendsFailed = false;
  var _friendsAsked = false;

  bool get _reachesConversations => widget.grant.resources.any(
    (resource) => resource.kind == appGrantConversation,
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_friendsAsked && _reachesConversations) {
      _friendsAsked = true;
      unawaited(_loadFriends());
    }
  }

  Future<void> _loadFriends() async {
    setState(() {
      _friends = null;
      _friendsFailed = false;
    });
    try {
      final api =
          contactsApi?.call(context) ??
          ContactsApi(UserAppsSession.of(context));
      final lists = await api.lists();
      if (!mounted) return;
      setState(
        () => _friends = {
          for (final friend in lists.friends)
            if (friend.conversationId.isNotEmpty)
              friend.conversationId: friend.person.label,
        },
      );
    } catch (_) {
      if (mounted) {
        setState(() {
          _friends = const {};
          _friendsFailed = true;
        });
      }
    }
  }

  /// Every conversation it reaches is known here (or it reaches none).
  bool get _known =>
      !_reachesConversations ||
      (_friends != null &&
          appGrantConversations(widget.grant.resources, _friends!).known);

  /// Which decision is on its way: approving waits for the account.
  bool? _approving;
  String? _error;

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  /// [reply]: Submit (#370), the comment going to Rivet as the person's
  /// reply, and nothing given; Rivet can suggest it again. Sent with
  /// approve off, so a backend that doesn't know replies declines it.
  /// [reason]: the no's, from focus mode's dock, with the comment.
  /// [rethrowing] for focus mode's keys (#514): refused, it throws too.
  Future<void> _decide({
    required bool approve,
    bool reply = false,
    String? reason,
    bool rethrowing = false,
  }) async {
    if (_approving != null || (approve && !_known)) return;
    final backend = AppScope.of(context);
    setState(() {
      _approving = approve;
      _error = null;
    });
    try {
      await backend.decideAppGrant(
        widget.agentId,
        widget.seq,
        approve: approve,
        comment: noComment(reason, _comment.text),
        reply: reply,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _approving = null);
    }
  }

  void _submit() => unawaited(_decide(approve: false, reply: true));

  /// Approve and Decline for focus mode's keys (#514), and Submit once a
  /// comment's typed, as the card's buttons.
  @override
  CardAnswer? answerNow() {
    if (widget.grant.state != AppGrantState.APP_GRANT_STATE_PENDING) {
      return null;
    }
    final busy = _approving != null;
    return CardAnswer(
      key: 'agent:${widget.agentId}/${widget.seq}',
      docked: false,
      yes: 'Approve',
      no: 'Decline',
      typed: _comment.text.trim().isNotEmpty,
      busy: busy,
      error: _error,
      onYes: busy || !_known
          ? null
          : () => _decide(approve: true, rethrowing: true),
      onSubmit: busy
          ? null
          : () => _decide(approve: false, reply: true, rethrowing: true),
      onNo: busy
          ? null
          : (reason) =>
                _decide(approve: false, reason: reason, rethrowing: true),
    );
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      // What's typed turns Submit on in the dock.
      listenable: Listenable.merge([store, _comment]),
      builder: (context, _) {
        publishAnswer();
        return _card(context, store.workspace);
      },
    );
  }

  Widget _card(BuildContext context, Workspace workspace) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final grant = widget.grant;
    final rivet =
        workspace.agents.where((a) => a.id == widget.agentId).firstOrNull ??
        rivetIn(workspace);
    final color = agentColor(rivet?.color ?? 'blue');
    final rivetName = rivet?.name ?? 'Rivet';
    final name = _agentName(grant, workspace);
    final pending = grant.state == AppGrantState.APP_GRANT_STATE_PENDING;
    final muted = grant.state == AppGrantState.APP_GRANT_STATE_DECLINED;
    final card = Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (rivet != null) ...[
                AgentCharacter(agent: rivet, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$rivetName suggests',
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.s),
          Text(
            appGrantSummary(grant, workspace),
            style: type.emphasis.copyWith(
              color: muted ? palette.textMuted : palette.text,
            ),
          ),
          // Before anything else it gives: whom it would give it to has
          // read a web page (Fugo's condition; a pipe before #346).
          if (grant.granteeReadPipes) ...[
            const SizedBox(height: Space.s),
            _ReadPipes(
              name: name,
              sends: grant.scopes.any((scope) => scope.id == appGrantSend),
            ),
          ],
          const SizedBox(height: Space.s),
          ..._access(palette, type, name),
          // After Claktory's own facts, set apart as Rivet's.
          if (grant.reason.trim().isNotEmpty) ...[
            const SizedBox(height: Space.m),
            _Reason(
              rivetName: rivetName,
              // Rivet's words, to decide on: what reorders text shows by
              // its code (proseCodes, Fugo).
              reason: proseCodes(grant.reason.trim()),
              color: color,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._pending(palette, type, name, rivetName)
          else
            ..._decided(palette, type, color, name, rivetName),
        ],
      ),
    );
    // As the ask card: the conversation's to select in it, its own in
    // Needs you.
    if (SelectionContainer.maybeOf(context) != null ||
        Overlay.maybeOf(context) == null) {
      return card;
    }
    return SelectableArea(child: card);
  }

  /// What the agent could do, where, and until when: the backend's words.
  List<Widget> _access(Palette palette, TypeScale type, String name) {
    final grant = widget.grant;
    final line = type.body.copyWith(color: palette.text);
    final faint = type.caption.copyWith(color: palette.textFaint);
    return [
      if (grant.scopes.isNotEmpty) ...[
        Text(
          '$name could:',
          style: type.caption.copyWith(
            color: palette.textMuted,
            fontWeight: FontWeight.w500,
          ),
        ),
        const SizedBox(height: 2),
        for (final scope in grant.scopes)
          Padding(
            padding: const EdgeInsets.only(left: Space.xs, top: 2),
            child: Text(
              '•  ${scope.label.isEmpty ? scope.id : scope.label}',
              style: line,
            ),
          ),
      ],
      if (_reachesConversations) ...[
        const SizedBox(height: Space.s),
        if (_friendsFailed) ...[
          Text(
            'Only in:',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          for (final resource in grant.resources)
            if (resource.kind == appGrantConversation)
              Padding(
                padding: const EdgeInsets.only(left: Space.xs, top: 2),
                child: Text(
                  '•  $appGrantUncheckedConversation',
                  style: line.copyWith(color: palette.textMuted),
                ),
              ),
        ] else if (_friends case final friends?) ...[
          Text(
            'Only in:',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          for (final each in appGrantConversations(
            grant.resources,
            friends,
          ).lines)
            Padding(
              padding: const EdgeInsets.only(left: Space.xs, top: 2),
              child: Text(
                '•  $each',
                style: each == appGrantUnknownConversation
                    ? line.copyWith(
                        color: palette.danger,
                        fontWeight: FontWeight.w500,
                      )
                    : line,
              ),
            ),
        ] else
          Text(appGrantReach(grant.resources), style: line),
      ] else if (grant.resources.isNotEmpty) ...[
        const SizedBox(height: Space.s),
        if (appGrantNamed(grant.resources)) ...[
          Text(
            'Only in:',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          for (final resource in grant.resources)
            Padding(
              padding: const EdgeInsets.only(left: Space.xs, top: 2),
              child: Text('•  ${resource.label}', style: line),
            ),
        ] else
          Text(appGrantReach(grant.resources), style: line),
      ],
      const SizedBox(height: Space.s),
      Text(appGrantUntil(grant.expiresMs.toInt()), style: faint),
    ];
  }

  List<Widget> _pending(
    Palette palette,
    TypeScale type,
    String name,
    String rivetName,
  ) {
    final busy = _approving != null;
    final loading = _reachesConversations && _friends == null;
    final unknown = !loading && !_known;
    return [
      // Once a comment is typed, one Submit: a reply, giving nothing.
      CommentTyped(
        controller: _comment,
        builder: (context, typed) => typed
            ? ActionLine(
                lead: Text(
                  _error ??
                      'Nothing is given. $rivetName gets your comment and '
                          'can suggest it again.',
                  style: type.caption.copyWith(
                    color: _error == null ? palette.textFaint : palette.danger,
                  ),
                ),
                action: SubmitButton(onPressed: busy ? null : _submit),
              )
            : _buttons(palette, type, name, busy: busy, unknown: unknown),
      ),
      // The shared Add a comment: typed, it's Submit's, Return too.
      const SizedBox(height: Space.xs),
      DecisionComment(
        controller: _comment,
        agent: rivetName,
        enabled: !busy,
        onSubmit: _submit,
      ),
    ];
  }

  /// Decline and Approve, while no comment is typed.
  Widget _buttons(
    Palette palette,
    TypeScale type,
    String name, {
    required bool busy,
    required bool unknown,
  }) {
    return ActionLine(
      lead: Text(
        _error ??
            (_approving == true
                ? 'Giving $name access…'
                : _friendsFailed
                ? "Can't check these conversations right now, so you "
                      "can't approve yet."
                : unknown
                ? "You can't approve this: one of these conversations "
                      "isn't among yours on this device."
                : 'Approving gives $name this access through your '
                      'claktory.com account.'),
        style: type.caption.copyWith(
          color: _error == null && !unknown
              ? palette.textFaint
              : palette.danger,
        ),
      ),
      action: ControlsLine(
        children: [
          if (_friendsFailed)
            Button(
              label: 'Try again',
              onPressed: busy ? null : () => unawaited(_loadFriends()),
            ),
          Button(
            label: 'Decline',
            onPressed: busy ? null : () => _decide(approve: false),
          ),
          Button(
            label: 'Approve',
            primary: true,
            // Only once every conversation it reaches is known here.
            onPressed: busy || !_known ? null : () => _decide(approve: true),
          ),
        ],
      ),
    );
  }

  List<Widget> _decided(
    Palette palette,
    TypeScale type,
    Color color,
    String name,
    String rivetName,
  ) {
    final grant = widget.grant;
    final decided = grant.decidedMs.toInt();
    final failed = grant.state == AppGrantState.APP_GRANT_STATE_FAILED;
    final (String said, Color tone) = switch (grant.state) {
      AppGrantState.APP_GRANT_STATE_GRANTED => (
        'Approved · $name has access',
        color,
      ),
      AppGrantState.APP_GRANT_STATE_FAILED => (
        "Approved, but couldn't give $name access",
        palette.danger,
      ),
      // Answered with a comment (#370): nothing given.
      _ when grant.replied => ('Replied', palette.textMuted),
      _ => ('Declined', palette.textMuted),
    };
    return [
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const PersonAvatar(size: 16),
          const SizedBox(width: Space.xs + 2),
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: said,
                    style: TextStyle(color: tone, fontWeight: FontWeight.w500),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text:
                          ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(decided))}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ],
      ),
      if (failed && grant.error.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(grant.error, style: type.body.copyWith(color: palette.textMuted)),
      ],
      if (grant.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          grant.comment,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
    ];
  }
}

/// Rivet's reason, set apart as its words, never as Claktory's: under
/// "Rivet's reason", plain text (no Markdown or links, so nothing in it
/// can look like the card's own lines or controls) beside a bar in
/// Rivet's color, four lines and then Show more (Fugo's rule).
class _Reason extends StatelessWidget {
  const _Reason({
    required this.rivetName,
    required this.reason,
    required this.color,
  });

  final String rivetName;
  final String reason;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          "$rivetName's reason",
          style: type.caption.copyWith(color: palette.textFaint),
        ),
        const SizedBox(height: 2),
        Container(
          padding: const EdgeInsets.only(left: Space.s),
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(color: color.withValues(alpha: 0.5), width: 2),
            ),
          ),
          child: ShowMore(
            child: Text(
              reason,
              style: type.body.copyWith(color: palette.textMuted),
            ),
          ),
        ),
      ],
    );
  }
}

/// The agent it would give access to has read a web page: a line after a
/// globe, as cards say where a request may come from; when the grant would
/// let it send as the person, a `danger` box of its own instead.
class _ReadPipes extends StatelessWidget {
  const _ReadPipes({required this.name, required this.sends});

  final String name;
  final bool sends;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final words = appGrantReadPipes(name, sends: sends);
    if (!sends) {
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: ExcludeSemantics(
              child: ExtensionGlyph(
                icon: 'globe',
                name: '',
                color: palette.textMuted,
                size: 12,
              ),
            ),
          ),
          const SizedBox(width: Space.xs + 2),
          Expanded(
            child: Text(
              words,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ),
        ],
      );
    }
    return Container(
      padding: const EdgeInsets.all(Space.s),
      decoration: BoxDecoration(
        color: palette.danger.withValues(alpha: 0.08),
        border: Border.all(color: palette.danger.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 3),
            child: ExcludeSemantics(
              child: ExtensionGlyph(
                icon: 'globe',
                name: '',
                color: palette.danger,
                size: 14,
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          Expanded(
            child: Text(
              words,
              style: type.body.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
