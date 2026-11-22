import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show agentExtension, logTime;
import 'identity.dart';
import 'markdown.dart';
import 'mobile/platform_policy.dart' show PhonePlatformPolicy;
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'tool_views.dart' show ShowMore;

/// Why an agent's message was sent, as its sender said (`Event.intent`,
/// `QuietCopy.intent`), in the app's words; empty when it didn't say or
/// said something this app doesn't know.
String intentLabel(String intent) => switch (intent) {
  'handoff' => 'Handoff',
  'decision_request' => 'Asks for a decision',
  'decision' => 'Decision',
  'status' => 'Status update',
  _ => '',
};

/// What a message from another agent stands on, quietly under its bubble:
/// why it was sent, and that it replies to an earlier message. The message
/// it replies to is one the sender received, so it's in the sender's
/// conversation, not this one, and never named by its number: the person
/// never sees message numbers. Nothing at all for a message that says
/// neither (older ones).
class MessageProvenance extends StatelessWidget {
  const MessageProvenance({super.key, required this.event});

  final Event event;

  @override
  Widget build(BuildContext context) {
    final line = provenanceLine(
      event.intent,
      event.replyToIds.length,
      quiet: event.quiet,
    );
    if (line.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(left: Space.m, top: 2),
      child: Text(
        line,
        style: TypeScale.of(context).caption
            .copyWith(color: Palette.of(context).textFaint),
      ),
    );
  }
}

/// Whether a message came quietly, why it was sent and whether it replies,
/// in one line: "Quiet · Handoff · Replying to an earlier message"; empty
/// when it says none of them.
String provenanceLine(String intent, int replies, {bool quiet = false}) => [
  if (quiet) 'Quiet',
  if (intentLabel(intent) case final label when label.isNotEmpty) label,
  if (replies == 1) 'Replying to an earlier message',
  if (replies > 1) 'Replying to earlier messages',
].join(' · ');

/// The words for how many quiet copies [agent] hasn't read yet.
String newCopiesLabel(int count) =>
    count == 1 ? '1 new copy' : '$count new copies';

/// How many copies, shown where there's little room: "1 copy".
String copiesLabel(int count) => count == 1 ? '1 copy' : '$count copies';

/// Whether [agent] has quiet copies it hasn't read, and Messaging on to
/// show them in.
bool showsNewCopies(Agent agent) =>
    agent.unreadCopies > 0 &&
    (agentExtension(agent, 'agents')?.enabled ?? false);

/// In an agent's header while it has quiet copies it hasn't read: how many,
/// in a quiet hairline capsule that opens its Messaging panel, where they
/// are. Nothing otherwise. On a phone, the number alone in a header
/// button.
class QuietCopiesChip extends StatelessWidget {
  const QuietCopiesChip({super.key, required this.agent, this.phone = false});

  final Agent agent;

  /// In the phone's header bar: a header button.
  final bool phone;

  @override
  Widget build(BuildContext context) {
    if (!showsNewCopies(agent)) return const SizedBox.shrink();
    final count = agent.unreadCopies;
    final label = newCopiesLabel(count);
    void open() =>
        fireAndLog(AppScope.of(context).openPanel(agent.id, 'agents'));
    if (phone) {
      // How many, with a word, so it isn't read as another count of what
      // needs the person: "1 copy". A header's 44pt tall target, as wide as
      // its words, at a glyph's size whatever the text size (VoiceOver
      // reads "1 new copy").
      final shown = count > 99 ? '99+ copies' : copiesLabel(count);
      return Unselectable(
        child: Semantics(
          container: true,
          button: true,
          label: label,
          onTap: open,
          excludeSemantics: true,
          child: Pressable(
            onTap: open,
            builder: (context, pressed) => SizedBox(
              height: PhonePlatformPolicy.of(context).touchTarget,
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                child: Center(
                  child: MediaQuery.withNoTextScaling(
                    child: _Capsule(text: shown, hovered: pressed),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }
    return Unselectable(
      child: Semantics(
        button: true,
        label: label,
        hint: 'Opens Messaging',
        excludeSemantics: true,
        child: Pressable(
          onTap: open,
          builder: (context, hovered) =>
              _Capsule(text: label, hovered: hovered),
        ),
      ),
    );
  }
}

class _Capsule extends StatelessWidget {
  const _Capsule({required this.text, this.hovered = false});

  final String text;
  final bool hovered;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: Space.s, vertical: 2),
      decoration: BoxDecoration(
        color: hovered ? palette.hover : null,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Text(
        text,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TypeScale.of(context).caption.copyWith(
          color: hovered ? palette.text : palette.textMuted,
          fontWeight: FontWeight.w500,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
    );
  }
}

/// The words for how many quiet messages wait for an agent: "1 quiet
/// message", "4 quiet messages".
String quietWaitingLabel(int count) =>
    count == 1 ? '1 quiet message' : '$count quiet messages';

/// The same where there's little room: "4 quiet".
String quietWaitingShort(int count) => '$count quiet';

/// What waiting means, for hover and VoiceOver.
String quietWaitingAbout(Agent agent) {
  final count = agent.quietWaiting;
  final them = count == 1 ? 'it' : 'them';
  return '${quietWaitingLabel(count)} waiting. ${agent.name} reads $them '
      'the next time it starts working.';
}

/// Under a quiet message still waiting for [name], where others say
/// "Queued".
String quietQueuedLabel(String name) =>
    'Quiet · read when $name next starts working';

/// In an agent's header while quiet messages wait for it: how many, in a
/// quiet hairline capsule, saying on hover what that means. Not a button:
/// they're at the conversation's end already. Nothing while none wait. On
/// a phone, shorter ("4 quiet"), its words read by VoiceOver.
class QuietWaitingChip extends StatelessWidget {
  const QuietWaitingChip({super.key, required this.agent, this.phone = false});

  final Agent agent;

  /// In the phone's header bar.
  final bool phone;

  @override
  Widget build(BuildContext context) {
    final count = agent.quietWaiting;
    if (count == 0) return const SizedBox.shrink();
    final about = quietWaitingAbout(agent);
    final faint = TypeScale.of(context).caption
        .copyWith(color: Palette.of(context).textMuted);
    if (phone) {
      return Unselectable(
        child: Semantics(
          container: true,
          label: about,
          excludeSemantics: true,
          child: HoverCard(
            end: true,
            holdToShow: true,
            card: (context) => Text(about, style: faint),
            child: SizedBox(
              height: PhonePlatformPolicy.of(context).touchTarget,
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                child: Center(
                  child: MediaQuery.withNoTextScaling(
                    child: _Capsule(
                      text: count > 99 ? '99+ quiet' : quietWaitingShort(count),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }
    return Unselectable(
      child: Semantics(
        container: true,
        label: about,
        excludeSemantics: true,
        child: HoverCard(
          end: true,
          card: (context) => ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 280),
            child: Text(about, style: faint),
          ),
          child: _Capsule(text: quietWaitingLabel(count)),
        ),
      ),
    );
  }
}

/// How many copies a page asks for.
const quietCopiesPage = 20;

/// The quiet copies [agentId] was sent (its team's messages to each other,
/// copied to it without waking it), newest first, in its Messaging panel:
/// a heading, what they are, each copy, and Show earlier copies while
/// there are older ones. Nothing while it has none. Asked for when it
/// shows, and again when how many it hasn't read changes; nothing is kept.
/// Listing them doesn't mark them read: only the agent reading them does.
class QuietCopiesSection extends StatefulWidget {
  const QuietCopiesSection({super.key, required this.agentId});

  final String agentId;

  @override
  State<QuietCopiesSection> createState() => _QuietCopiesSectionState();
}

class _QuietCopiesSectionState extends State<QuietCopiesSection> {
  late Backend _backend;
  var _copies = <QuietCopy>[];
  var _more = false;
  var _loading = false;
  String? _failed;

  /// Counts requests, so only the latest one's answer shows.
  var _asked = 0;

  /// How many it hadn't read when last asked; a change asks again.
  int? _unread;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (_unread != null && identical(backend, _backend)) return;
    if (_unread != null) _backend.workspace.removeListener(_changed);
    _backend = backend;
    _backend.workspace.addListener(_changed);
    _unread = _unreadNow();
    _load();
  }

  @override
  void didUpdateWidget(QuietCopiesSection old) {
    super.didUpdateWidget(old);
    if (old.agentId != widget.agentId) {
      _copies = [];
      _more = false;
      _unread = _unreadNow();
      _load();
    }
  }

  @override
  void dispose() {
    _backend.workspace.removeListener(_changed);
    super.dispose();
  }

  int _unreadNow() =>
      _backend.workspace.agent(widget.agentId)?.unreadCopies ?? 0;

  void _changed() {
    final unread = _unreadNow();
    if (unread == _unread) return;
    _unread = unread;
    _load();
  }

  /// The newest page; [earlier], the page before what's shown.
  Future<void> _load({bool earlier = false}) async {
    final asked = ++_asked;
    if (mounted) {
      setState(() {
        _loading = true;
        _failed = null;
      });
    }
    try {
      final page = await _backend.listQuietCopies(
        widget.agentId,
        beforeId: earlier && _copies.isNotEmpty ? _copies.last.id.toInt() : 0,
        limit: quietCopiesPage,
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        _copies = earlier ? [..._copies, ...page.copies] : page.copies;
        _more = page.more;
        _loading = false;
      });
    } on RequestFailure catch (error) {
      if (!mounted || asked != _asked) return;
      setState(() {
        _failed = error.message;
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    if (_copies.isEmpty && _failed == null) return const SizedBox.shrink();
    final store = _backend.workspace;
    final lead = store.agent(widget.agentId)?.name ?? widget.agentId;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.s,
              Space.m,
              Space.s,
              Space.xs,
            ),
            child: Text(
              'Quiet copies',
              style: type.caption.copyWith(
                color: palette.textFaint,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.xs),
            child: Text(
              'Messages its team sent each other, copied to it without '
              "interrupting it. It reads them when it's ready.",
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ),
          for (final copy in _copies)
            KeyedSubtree(
              key: ValueKey('copy/${copy.id}'),
              child: QuietCopyRow(copy: copy, lead: lead),
            ),
          if (_failed case final why?)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.s,
                vertical: Space.xs,
              ),
              child: Wrap(
                spacing: Space.s,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Text(
                    why,
                    style: type.caption.copyWith(color: palette.danger),
                  ),
                  Button(
                    label: 'Try again',
                    onPressed: () => _load(earlier: _copies.isNotEmpty),
                  ),
                ],
              ),
            )
          else if (_more)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.s,
                vertical: Space.xs,
              ),
              child: Align(
                alignment: Alignment.centerLeft,
                child: _loading
                    ? Text(
                        'Loading…',
                        style: type.caption.copyWith(color: palette.textFaint),
                      )
                    : Button(
                        label: 'Show earlier copies',
                        onPressed: () => _load(earlier: true),
                      ),
              ),
            ),
        ],
      ),
    );
  }
}

/// One quiet copy: its sender's character and name, whom the message went
/// to, when, a dot while [lead] hasn't read it, why it was sent and what it
/// replies to, then its words, cut to four lines with Show more.
class QuietCopyRow extends StatelessWidget {
  const QuietCopyRow({super.key, required this.copy, required this.lead});

  final QuietCopy copy;

  /// The name of the agent it was copied to.
  final String lead;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final store = AppScope.of(context).workspace;
    final sender = store.agent(copy.fromId);
    final to = [for (final id in copy.toIds) store.agent(id)?.name ?? id];
    final faint = type.caption.copyWith(color: palette.textFaint);
    final about = provenanceLine(copy.intent, copy.replyToIds.length);
    final when = logTime(
      DateTime.fromMillisecondsSinceEpoch(copy.createdMs.toInt()),
    );
    return Padding(
      padding: EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: isPhoneLayout(context) ? Space.s : Space.xs,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Padding(
                padding: const EdgeInsets.only(right: Space.xs + 2),
                child: sender == null
                    ? const UnknownCharacter(size: 18)
                    : AgentCharacter(agent: sender, size: 18),
              ),
              Expanded(
                child: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: sender?.name ?? copy.from,
                        style: TextStyle(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      if (to.isNotEmpty)
                        TextSpan(
                          text: '  to ${to.join(', ')}',
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                    ],
                  ),
                  style: type.body,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              const SizedBox(width: Space.s),
              Text(when, style: faint),
              if (!copy.read)
                Semantics(
                  container: true,
                  label: 'Not read yet by $lead',
                  child: Padding(
                    padding: const EdgeInsets.only(left: Space.xs + 2),
                    child: HoverCard(
                      end: true,
                      card: (context) =>
                          Text('Not read yet by $lead', style: faint),
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: BoxDecoration(
                          color: palette.textMuted,
                          shape: BoxShape.circle,
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
          if (about.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(about, style: faint),
            ),
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: ShowMore(
              child: Markdown(copy.text, color: palette.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}
