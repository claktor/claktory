import 'dart:async';

import 'package:flutter/widgets.dart';

import 'app_grant_card.dart' show appGrantReadPipes;
import 'apps.dart' show publisherLine, tierLine;
import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'card_answer.dart' show CardAnswer, PublishesAnswer, noComment;
import 'decision.dart' show CommentTyped, DecisionComment, SubmitButton;
import 'extensions.dart' show logTime;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'tool_views.dart' show ShowMore;
import 'visible_codes.dart' show proseCodes;

/// What the card and Needs you ask, in a line: "Let Charts read Activity?",
/// "Let Mike use Charts?", or both: "Let Charts read Activity, and let Mike
/// use it?".
String appAccessSummary(AppAccessProposal access, Workspace workspace) {
  final app = access.appName.isEmpty ? 'this app' : access.appName;
  final sources = _andList([for (final source in access.sources) source.label]);
  final agents = _andList([
    for (final grant in access.agents) _agentName(grant.agentId, workspace),
  ]);
  if (agents.isEmpty && sources.isEmpty) return 'Change what $app can use?';
  if (agents.isEmpty) return 'Let $app read $sources?';
  if (sources.isEmpty) return 'Let $agents use $app?';
  return 'Let $app read $sources, and let $agents use it?';
}

String _agentName(String id, Workspace workspace) =>
    workspace.agents.where((agent) => agent.id == id).firstOrNull?.name ?? id;

/// "A", "A and B", "A, B and C".
String _andList(List<String> items) => items.length < 2
    ? items.join()
    : '${items.take(items.length - 1).join(', ')} and ${items.last}';

/// Where what [app] would read could go: nowhere off this computer, or the
/// [hosts] it reaches, which a warning names when it would read [private]
/// data (Fugo's point 8).
String appAccessReach(String app, List<String> hosts, List<String> private) {
  if (hosts.isEmpty) {
    return "$app can't reach the internet, so what it reads stays on this "
        'computer.';
  }
  final reach = '$app can reach ${_andList(hosts)}';
  if (private.isEmpty) return '$reach.';
  return '$reach, so ${_andList(private)} could leave this computer.';
}

/// Rivet's proposal of what an installed app may read and which agents may
/// use it (AppAccessProposal), shown exactly as it would be given, source
/// by source and agent by agent: the app with who vouches for it, each
/// source with what it holds (the private ones marked), where it could go,
/// each agent with what it could do and, through the app, read; then
/// Rivet's reason, set apart, and Decline or Approve, all or nothing. The
/// approval names what the card showed (its digest), so a proposal changed
/// since is refused and shown again. A sibling of the app grant card, in
/// Rivet's color; the backend sends the entry back decided.
class AppAccessCard extends StatefulWidget {
  const AppAccessCard(
    this.access, {
    super.key,
    required this.agentId,
    required this.seq,
  });

  final AppAccessProposal access;

  /// Whose conversation shows it: Rivet's.
  final String agentId;

  /// The entry's seq in that conversation.
  final int seq;

  @override
  State<AppAccessCard> createState() => _AppAccessCardState();
}

class _AppAccessCardState extends State<AppAccessCard>
    with PublishesAnswer {
  final _comment = TextEditingController();

  /// Which decision is on its way.
  bool? _deciding;
  String? _error;

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  /// [reply]: Submit (#370), the comment going to Rivet as the person's
  /// reply, and nothing allowed; Rivet can suggest it again. Sent with
  /// approve off, so a backend that doesn't know replies declines it.
  /// [reason]: the no's, from focus mode's dock, with the comment.
  /// [rethrowing] for focus mode's keys (#514): refused, it throws too.
  Future<void> _decide({
    required bool approve,
    bool reply = false,
    String? reason,
    bool rethrowing = false,
  }) async {
    if (_deciding != null) return;
    final backend = AppScope.of(context);
    setState(() {
      _deciding = approve;
      _error = null;
    });
    try {
      await backend.decideAppAccess(
        widget.agentId,
        widget.seq,
        approve: approve,
        digest: widget.access.digest,
        comment: noComment(reason, _comment.text),
        reply: reply,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _deciding = null);
    }
  }

  void _submit() => unawaited(_decide(approve: false, reply: true));

  /// Approve and Decline for focus mode's keys (#514), and Submit once a
  /// comment's typed, as the card's buttons.
  @override
  CardAnswer? answerNow() {
    if (widget.access.state != AppGrantState.APP_GRANT_STATE_PENDING) {
      return null;
    }
    final busy = _deciding != null;
    return CardAnswer(
      key: 'agent:${widget.agentId}/${widget.seq}',
      docked: false,
      yes: 'Approve',
      no: 'Decline',
      typed: _comment.text.trim().isNotEmpty,
      busy: busy,
      error: _error,
      onYes: busy ? null : () => _decide(approve: true, rethrowing: true),
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
    final access = widget.access;
    final rivet =
        workspace.agents.where((a) => a.id == widget.agentId).firstOrNull ??
        rivetIn(workspace);
    final color = agentColor(rivet?.color ?? 'blue');
    final rivetName = rivet?.name ?? 'Rivet';
    final pending = access.state == AppGrantState.APP_GRANT_STATE_PENDING;
    final muted = access.state == AppGrantState.APP_GRANT_STATE_DECLINED;
    final app = workspace.apps.where((a) => a.id == access.appId).firstOrNull;
    final name = access.appName.isEmpty ? 'This app' : access.appName;
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
            appAccessSummary(access, workspace),
            style: type.emphasis.copyWith(
              color: muted ? palette.textMuted : palette.text,
            ),
          ),
          const SizedBox(height: 2),
          Text(
            [
              name,
              if (access.publisher.isNotEmpty &&
                  access.tier != AppTier.APP_TIER_CLAKTORY)
                publisherLine(access.tier, access.publisher),
              tierLine(access.tier, ''),
            ].join(' · '),
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          // Why it's a card at all, in Claktory's words, before anything
          // Rivet wrote: a steered Rivet's reason has this to answer to
          // (Fugo, rows 674/675). In the backend's order, which puts
          // Rivet having read another Claktory first.
          if (access.cardRules.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            _CardRules(access.cardRules),
          ],
          if (access.sources.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            ..._sources(palette, type, name),
          ],
          for (final grant in access.agents) ...[
            const SizedBox(height: Space.m),
            _AgentAccess(
              access: access,
              grant: grant,
              app: app,
              workspace: workspace,
            ),
          ],
          // After Claktory's own facts, set apart as Rivet's.
          if (access.reason.trim().isNotEmpty) ...[
            const SizedBox(height: Space.m),
            _Reason(
              rivetName: rivetName,
              // Rivet's words, to decide on: what reorders text shows by
              // its code (proseCodes, Fugo).
              reason: proseCodes(access.reason.trim()),
              color: color,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._pending(palette, type, rivetName)
          else
            ..._decided(palette, type, color),
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

  /// What the app could read, source by source, and where it could go.
  List<Widget> _sources(Palette palette, TypeScale type, String name) {
    final access = widget.access;
    final private = [
      for (final source in access.sources)
        if (source.private) source.label,
    ];
    final reach = appAccessReach(name, access.hosts, private);
    return [
      _Heading('$name could read:'),
      for (final source in access.sources)
        Padding(
          padding: const EdgeInsets.only(left: Space.xs, top: Space.xs),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text.rich(
                TextSpan(
                  children: [
                    TextSpan(text: '•  ${proseCodes(source.label)}'),
                    if (source.private)
                      TextSpan(
                        text: '  Private',
                        style: type.caption.copyWith(
                          color: palette.signal,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                  ],
                ),
                style: type.body.copyWith(color: palette.text),
              ),
              if (source.holds.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(left: Space.m),
                  child: Text(
                    source.holds,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                ),
            ],
          ),
        ),
      const SizedBox(height: Space.s),
      if (access.hosts.isNotEmpty && private.isNotEmpty)
        Container(
          padding: const EdgeInsets.all(Space.s),
          decoration: BoxDecoration(
            color: palette.signal.withValues(alpha: 0.06),
            border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Text(reach, style: type.body.copyWith(color: palette.text)),
        )
      else
        Text(reach, style: type.caption.copyWith(color: palette.textFaint)),
    ];
  }

  List<Widget> _pending(Palette palette, TypeScale type, String rivetName) {
    final busy = _deciding != null;
    return [
      // Once a comment is typed, one Submit: a reply, allowing nothing.
      CommentTyped(
        controller: _comment,
        builder: (context, typed) => ActionLine(
          lead: Text(
            _error ??
                (typed
                    ? 'Nothing is allowed. $rivetName gets your comment and '
                          'can suggest it again.'
                    : _deciding == true
                    ? 'Approving…'
                    : 'Approving allows exactly this, and nothing more.'),
            style: type.caption.copyWith(
              color: _error == null ? palette.textFaint : palette.danger,
            ),
          ),
          action: typed
              ? SubmitButton(onPressed: busy ? null : _submit)
              : ControlsLine(
                  children: [
                    Button(
                      label: 'Decline',
                      onPressed: busy ? null : () => _decide(approve: false),
                    ),
                    Button(
                      label: 'Approve',
                      primary: true,
                      onPressed: busy ? null : () => _decide(approve: true),
                    ),
                  ],
                ),
        ),
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

  List<Widget> _decided(Palette palette, TypeScale type, Color color) {
    final access = widget.access;
    final decided = access.decidedMs.toInt();
    final failed = access.state == AppGrantState.APP_GRANT_STATE_FAILED;
    final (String said, Color tone) = switch (access.state) {
      AppGrantState.APP_GRANT_STATE_GRANTED => ('Approved', color),
      AppGrantState.APP_GRANT_STATE_FAILED => (
        "Approved, but it couldn't be done",
        palette.danger,
      ),
      // Answered with a comment (#370): nothing allowed.
      _ when access.replied => ('Replied', palette.textMuted),
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
      if (failed && access.error.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(access.error, style: type.body.copyWith(color: palette.textMuted)),
      ],
      if (access.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          access.comment,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
    ];
  }
}

/// A part of the card's facts: "Charts could read:".
class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Text(
      text,
      style: TypeScale.of(context).caption
          .copyWith(color: palette.textMuted, fontWeight: FontWeight.w500),
    );
  }
}

/// One agent the proposal would let use the app: its character and name,
/// what it could do (the app's words for each scope), what it could read
/// through the app, whether this replaces what it has, and whether it has
/// read a web page.
class _AgentAccess extends StatelessWidget {
  const _AgentAccess({
    required this.access,
    required this.grant,
    required this.app,
    required this.workspace,
  });

  final AppAccessProposal access;
  final AppGrant grant;

  /// The app as installed now, for its scopes' words; null once it's gone.
  final AppInfo? app;
  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = workspace.agents
        .where((agent) => agent.id == grant.agentId)
        .firstOrNull;
    final name = agent?.name ?? grant.agentId;
    final appName = access.appName.isEmpty ? 'this app' : access.appName;
    final specs = {
      for (final scope in app?.scopes ?? const <AppScopeSpec>[])
        scope.id: scope,
    };
    final readsData = grant.scopes.any((id) => specs[id]?.readsData ?? false);
    // What the app would read once approved: these and what it reads now.
    final read = <String, String>{
      for (final source in app?.sources ?? const <AppDataSource>[])
        if (source.granted) source.id: source.label,
      for (final source in access.sources) source.id: source.label,
    };
    final had = app?.grants
        .where((given) => given.agentId == grant.agentId)
        .firstOrNull;
    final replaces =
        had != null &&
        had.scopes.isNotEmpty &&
        !(had.scopes.toSet().containsAll(grant.scopes) &&
            grant.scopes.toSet().containsAll(had.scopes));
    // The app's events it would hear, in the app's words.
    final labels = {
      for (final event in app?.events ?? const <AppEventKind>[])
        event.kind: event.label,
    };
    final heard = [
      for (final subscription in access.subscriptions)
        if (subscription.agentIds.contains(grant.agentId))
          proseCodes(labels[subscription.eventKind] ?? subscription.eventKind),
    ];
    final line = type.body.copyWith(color: palette.text);
    final caption = type.caption.copyWith(color: palette.textMuted);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            if (agent != null) ...[
              AgentCharacter(agent: agent, size: 16),
              const SizedBox(width: Space.xs + 2),
            ],
            Flexible(child: _Heading('$name could:')),
          ],
        ),
        for (final id in grant.scopes)
          Padding(
            padding: const EdgeInsets.only(left: Space.xs, top: 2),
            child: Text(
              '•  ${proseCodes(specs[id]?.label.isNotEmpty ?? false ? specs[id]!.label : id)}',
              style: line,
            ),
          ),
        if (readsData && read.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(
            'Through $appName, $name could also read '
            '${_andList(read.values.toList())}.',
            style: caption,
          ),
        ],
        if (heard.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text('$name would hear: ${_andList(heard)}.', style: caption),
        ],
        if (replaces) ...[
          const SizedBox(height: Space.xs),
          Text(
            'This replaces what $name can do with $appName now.',
            style: caption,
          ),
        ],
        if (access.agentsReadPipes.contains(grant.agentId)) ...[
          const SizedBox(height: Space.xs),
          _ReadNote(appGrantReadPipes(name, sends: false)),
        ],
      ],
    );
  }
}

/// Why the person is asked rather than Rivet doing it: Claktory's own
/// sentences (AppAccessProposal.card_rules), under its heading, in the
/// card's warning box: never set apart as Rivet's.
class _CardRules extends StatelessWidget {
  const _CardRules(this.rules);

  final List<String> rules;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      padding: const EdgeInsets.all(Space.s),
      decoration: BoxDecoration(
        color: palette.signal.withValues(alpha: 0.06),
        border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Why Claktory asks you',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          for (final rule in rules)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Text(rule, style: type.body.copyWith(color: palette.text)),
            ),
        ],
      ),
    );
  }
}

/// A line about a web page the agent read, after a globe, as cards say
/// where a request may come from.
class _ReadNote extends StatelessWidget {
  const _ReadNote(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Row(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Padding(
        padding: const EdgeInsets.only(top: 2),
        child: ExcludeSemantics(
          child: ExtensionGlyph(
            icon: 'globe',
            name: '',
            color: Palette.of(context).textMuted,
            size: 12,
          ),
        ),
      ),
      const SizedBox(width: Space.xs + 2),
      Expanded(
        child: Text(
          text,
          style: TypeScale.of(context).caption
              .copyWith(color: Palette.of(context).textMuted),
        ),
      ),
    ],
  );
}

/// Rivet's reason, set apart as its words, never as Claktory's: under
/// "Rivet's reason", plain text beside a bar in Rivet's color, four lines
/// and then Show more (Fugo's rule, as the app grant card has it).
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
