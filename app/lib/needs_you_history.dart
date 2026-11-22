import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'chats.dart' show ChatGlyph, chatTitle, openChat;
import 'controls.dart';
import 'find.dart' show FindScope;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'pipe_timeline.dart' show pipeItemKey, pipeLiaison;
import 'pipes.dart' show PortalGlyph, pipeChatRevealId, pipeColor;
import 'protocol/factory/v1/factory.pb.dart';
import 'report_types.dart' show recordHasReportTypes, showReportTypeRecord;
import 'scope.dart';
import 'selection.dart' show CopyShape, Unselectable;
import 'taint_label.dart';
import 'usage.dart' show formatAge;

/// How many records a page of history asks for.
const historyPage = 50;

/// What the person already answered, approved, declined or dismissed in the
/// last 30 days (Needs you's History), newest first: a search field that
/// filters by agent and words, a row per card, and Show earlier while
/// there's more. A row opens the conversation at its card. Asked for each
/// time it shows, and again as the search changes; nothing is kept.
class NeedsYouHistoryList extends StatefulWidget {
  const NeedsYouHistoryList({super.key, this.padding});

  /// Around the list; the page's own by default.
  final EdgeInsets? padding;

  @override
  State<NeedsYouHistoryList> createState() => _NeedsYouHistoryListState();
}

class _NeedsYouHistoryListState extends State<NeedsYouHistoryList> {
  late final Backend _backend = AppScope.of(context);
  final _search = TextEditingController();

  /// The search field's focus: ⌘F goes to it.
  final _field = FocusNode(debugLabel: 'Search history');
  Timer? _typing;

  /// What's shown, and whether there's more before it.
  var _records = <NeedsYouRecord>[];
  var _more = false;

  /// Asking now, and why the last ask failed.
  var _loading = true;
  String? _failed;

  /// Which ask is the latest: an answer to an older one is dropped.
  var _asked = 0;

  /// The search the records answer.
  var _query = '';

  @override
  void initState() {
    super.initState();
    _search.addListener(_searchChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _load();
    });
  }

  @override
  void dispose() {
    _typing?.cancel();
    _search.dispose();
    _field.dispose();
    super.dispose();
  }

  /// Asks again once typing pauses.
  void _searchChanged() {
    final query = _search.text.trim();
    if (query == _query) return;
    _typing?.cancel();
    _typing = Timer(const Duration(milliseconds: 300), () {
      if (mounted) _load();
    });
  }

  /// The newest page for the search; [earlier], the page before what's
  /// shown.
  Future<void> _load({bool earlier = false}) async {
    final query = _search.text.trim();
    final asked = ++_asked;
    setState(() {
      _loading = true;
      _failed = null;
    });
    try {
      final page = await _backend.listNeedsYouHistory(
        ListNeedsYouHistory(
          query: query,
          beforeMs: earlier && _records.isNotEmpty
              ? _records.last.decidedMs
              : Int64.ZERO,
          limit: historyPage,
        ),
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        _query = query;
        _records = earlier ? [..._records, ...page.records] : page.records;
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
    final phone = isPhoneLayout(context);
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    final padding =
        widget.padding ??
        EdgeInsets.symmetric(
          horizontal: phone ? Space.l : Space.xl,
          vertical: Space.l,
        );
    final query = _search.text.trim();
    final Widget below;
    if (_failed case final why? when _records.isEmpty) {
      below = Wrap(
        spacing: Space.s,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(why, style: TextStyles.caption.copyWith(color: palette.danger)),
          Button(label: 'Try again', onPressed: _load),
        ],
      );
    } else if (_records.isEmpty) {
      below = Text(
        _loading
            ? 'Loading…'
            : query.isEmpty
            ? 'Nothing in the last 30 days.'
            : 'Nothing matches “$query”.',
        style: TextStyles.body.copyWith(color: palette.textFaint),
      );
    } else {
      below = const SizedBox.shrink();
    }
    // In a column as wide as Waiting's at most, the whole pane scrolling.
    Widget column(Widget child) => Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 640),
        child: child,
      ),
    );
    // ⌘F (and a phone's Search) goes to its own search field.
    return FindScope.field(
      field: _field,
      words: _search,
      child: ListView(
        padding: padding,
        children: [
          // Kept out of what's selected: its placeholder isn't history.
          column(
            Unselectable(
              child: TextField(
                controller: _search,
                focusNode: _field,
                placeholder: 'Search by agent or words',
                semanticsLabel: 'Search history',
              ),
            ),
          ),
          const SizedBox(height: Space.m),
          if (_records.isEmpty) column(below),
          // Keyed on the list's own children, so a row keeps its place.
          for (final record in _records)
            KeyedSubtree(
              key: ValueKey(
                '${record.kind}/${record.agentId}${record.chatId}'
                '/${record.seq}/${record.decidedMs}',
              ),
              // Copied, its rows on lines of their own and a blank line
              // after it, apart from the next; selected where the pointer is
              // at any zoom, however far it's scrolled.
              child: column(
                ScrolledSelection(
                  child: CopyShape(
                    lines: true,
                    trail: '\n\n',
                    child: HistoryRow(record: record),
                  ),
                ),
              ),
            ),
          if (_records.isNotEmpty && _more)
            column(
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: _loading
                      ? Text('Loading…', style: caption)
                      : Button(
                          label: 'Show earlier',
                          onPressed: () => _load(earlier: true),
                        ),
                ),
              ),
            ),
          if (_records.isNotEmpty && _failed != null)
            column(
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: Text(
                  _failed!,
                  style: TextStyles.caption.copyWith(color: palette.danger),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// Whether [record] is a pipe's own decision (a held message, a loop
/// pause, where its reports go) rather than an agent's or chat's card.
bool _ofPipe(NeedsYouRecord record) =>
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_HELD ||
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_LOOP ||
    (record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_FORWARD &&
        record.pipeId.isNotEmpty);

/// What a record's outcome is called: as its card's buttons said it; ""
/// for one this app doesn't know (a newer Claktory's).
String historyOutcome(NeedsYouRecord record) => switch (record.outcome) {
  // A held message: an agent's was sent, the other side's let in.
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED
      when record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_HELD =>
    record.agentId.isEmpty ? 'Let in' : 'Sent',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DENIED
      when record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_HELD =>
    'Discarded',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED
      when record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_LOOP =>
    'Continued',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_ANSWERED => 'Answered',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED => 'Approved',
  // A secret's use as its card says it, the rest Declined (never Denied).
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DENIED
      when record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_SECRET_USE =>
    'Not allowed',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DENIED => 'Declined',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DISMISSED => 'Dismissed',
  // Approved, but the grant couldn't be made (Rivet's app access).
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_FAILED => "Approved, but couldn't give it",
  _ => '',
};

/// [text] on one line, cut at a word within [max] characters, with "…".
String _shortened(String text, int max) {
  final line = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  if (line.length <= max) return line;
  final cut = line.lastIndexOf(' ', max);
  return '${line.substring(0, cut > 0 ? cut : max).trimRight()}…';
}

/// One card the person decided: who asked (the agent's character and
/// name, a chat's bubble, a pipe's portal) with when they decided, what it
/// asked, and how they answered ("Answered · “Use the EU bucket”"), with
/// the card's taint. A click opens the conversation at the card.
class HistoryRow extends StatelessWidget {
  const HistoryRow({super.key, required this.record});

  final NeedsYouRecord record;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final agent = workspace.agents
        .where((agent) => agent.id == record.agentId)
        .firstOrNull;
    final chat = workspace.chats
        .where((chat) => chat.id == record.chatId)
        .firstOrNull;
    final pipe = workspace.pipes
        .where((pipe) => pipe.id == record.pipeId)
        .firstOrNull;
    // A pipe by this side's name for it, never one the other side chose;
    // an agent with the pipe it's about, or the liaison's pipe, after it.
    final aboutPipe = _ofPipe(record)
        ? pipe
        : agent == null || agent.pipeId.isEmpty
        ? null
        : workspace.pipes.where((pipe) => pipe.id == agent.pipeId).firstOrNull;
    final agentName =
        agent?.name ?? (record.agentName.isEmpty ? null : record.agentName);
    // Where a project's new pipes' reports go, changed by the person: the
    // title says it all.
    final yours =
        (record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_FORWARD &&
            record.agentId.isEmpty &&
            record.pipeId.isEmpty) ||
        // A kind of report the person made, changed or deleted.
        (record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_REPORT_TYPE &&
            record.agentId.isEmpty);
    final who = yours
        ? 'You'
        : record.chatId.isNotEmpty
        ? (chat == null ? 'A chat' : chatTitle(chat))
        : _ofPipe(record) && agentName == null
        ? pipe?.name ?? 'A pipe'
        : aboutPipe != null
        ? '${agentName ?? 'An agent'} · ${aboutPipe.name}'
        : agentName ?? 'An agent';
    // The start of a held message from the other side, only under the
    // title that says where it's from, as plain text.
    final excerpt =
        record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_HELD &&
            record.agentId.isEmpty
        ? _shortened(record.excerpt, 200)
        : '';
    final reply = _shortened(record.reply, 120);
    final outcome = historyOutcome(record);
    final decided = record.decidedMs.toInt();
    final where = record.chatId.isNotEmpty
        ? 'Open chat'
        : record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_REPORT_TYPE
        ? 'Show what crosses'
        : _ofPipe(record)
        ? 'Open pipe'
        : 'Open conversation';
    return Semantics(
      button: true,
      hint: where,
      child: Pressable(
        onTap: () => openHistoryRecord(context, record),
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(
            minHeight: isPhoneLayout(context) ? Touch.target : 48,
          ),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox.square(
                dimension: 24,
                child: Center(
                  child: agent != null
                      ? AgentCharacter(agent: agent, size: 24)
                      : record.chatId.isNotEmpty
                      ? ChatGlyph(color: palette.textMuted)
                      : pipe != null || _ofPipe(record)
                      ? const PortalGlyph(color: pipeColor)
                      : ExtensionGlyph(
                          icon: 'user',
                          name: '',
                          color: palette.textFaint,
                        ),
                ),
              ),
              const SizedBox(width: Space.s + 2),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Ticking(
                      active: decided > 0,
                      builder: (context, now) => Text.rich(
                        TextSpan(
                          text: who,
                          children: [
                            if (decided > 0)
                              TextSpan(
                                text: ' · ${formatAge(decided, now)}',
                                style: TextStyles.caption.copyWith(
                                  color: palette.textFaint,
                                  fontWeight: FontWeight.w400,
                                ),
                              ),
                          ],
                        ),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    if (record.title.isNotEmpty)
                      Text(
                        record.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.body.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    // Kinds of report: a change that lets more cross says
                    // so; the kinds a pipe was set to forward open apart
                    // from the pipe.
                    if (record.widens)
                      Text(
                        'Lets more of their words through',
                        style: TextStyles.caption.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    if (record.kind ==
                            NeedsYouRecordKind
                                .NEEDS_YOU_RECORD_KIND_PIPE_FORWARD &&
                        record.reportTypes.isNotEmpty)
                      Pressable(
                        onTap: () => showReportTypeRecord(context, record),
                        builder: (context, hovered) => Text(
                          'See what comes through',
                          style: TextStyles.caption.copyWith(
                            color: hovered ? palette.text : palette.signal,
                          ),
                        ),
                      ),
                    if (excerpt.isNotEmpty)
                      Text(
                        '“$excerpt”',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.body.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    if (outcome.isNotEmpty || reply.isNotEmpty)
                      Text.rich(
                        TextSpan(
                          text: outcome,
                          children: [
                            if (reply.isNotEmpty)
                              TextSpan(
                                text: outcome.isEmpty
                                    ? '“$reply”'
                                    : ' · “$reply”',
                                style: TextStyles.caption.copyWith(
                                  color: palette.textFaint,
                                  fontWeight: FontWeight.w400,
                                ),
                              ),
                          ],
                        ),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.caption.copyWith(
                          color: palette.textMuted,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    TaintLabel(taint: record.taint, agent: who),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Opens the conversation [record]'s card is in, at the card when it's
/// still there (seq 0: at its end), or the chat.
void openHistoryRecord(BuildContext context, NeedsYouRecord record) {
  final backend = AppScope.of(context);
  final workspace = backend.workspace.workspace;
  // A kind of report made, changed or deleted: what crosses with it.
  if (record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_REPORT_TYPE) {
    if (recordHasReportTypes(record)) showReportTypeRecord(context, record);
    return;
  }
  if (record.chatId.isNotEmpty) {
    final chat = workspace.chats
        .where((chat) => chat.id == record.chatId)
        .firstOrNull;
    AppScope.closePage(context);
    if (chat != null) openChat(context, chat);
    // On a phone, the conversation is pushed over the list.
    PhoneNavigation.of(context)?.value = true;
    return;
  }
  if (_ofPipe(record)) {
    final pipe = workspace.pipes
        .where((pipe) => pipe.id == record.pipeId)
        .firstOrNull;
    if (pipe == null) return;
    final liaison = pipeLiaison(workspace, pipe);
    AppScope.closePage(context);
    fireAndLog(() async {
      await backend.selectPipe(pipe.id);
      // Among its liaison's conversation, at the message's card.
      // At the message's card: among its liaison's conversation, or in
      // its chat when it has none.
      if (record.seq > 0) {
        backend.reveal.reachExtra(
          liaison?.id ?? pipeChatRevealId(pipe.id),
          pipeItemKey(record.seq.toInt()),
        );
      }
    }());
    PhoneNavigation.of(context)?.value = true;
    return;
  }
  final agent = workspace.agents
      .where((agent) => agent.id == record.agentId)
      .firstOrNull;
  if (agent == null) return;
  AppScope.closePage(context);
  fireAndLog(() async {
    // A pipe's liaison's conversation shows in its pipe.
    if (agent.pipeId.isNotEmpty) {
      await backend.selectPipe(agent.pipeId);
    } else {
      await backend.selectAgent(agent.id);
    }
    if (record.seq > 0) {
      backend.reveal.reachSeq(agent.id, record.seq.toInt());
    }
  }());
  PhoneNavigation.of(context)?.value = true;
}

/// Needs you's two views, Waiting and History: on a Mac tabs as the Usage
/// page's ranges (body, `textMuted`, the chosen one `text` over a 2px
/// `signal` underline), on a phone a segmented control as its ranges.
class NeedsYouTabs extends StatelessWidget {
  const NeedsYouTabs({super.key, required this.history, required this.choose});

  /// Whether History is the one shown.
  final bool history;
  final ValueChanged<bool> choose;

  static const _labels = ['Waiting', 'History'];

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    if (phone) {
      final type = TypeScale.of(context);
      const inset = 2.0;
      return Container(
        decoration: BoxDecoration(
          color: palette.hover,
          borderRadius: BorderRadius.circular(Radii.panel),
        ),
        child: IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var i = 0; i < _labels.length; i++)
                Expanded(
                  child: Semantics(
                    button: true,
                    selected: history == (i == 1),
                    inMutuallyExclusiveGroup: true,
                    label: _labels[i],
                    excludeSemantics: true,
                    onTap: () => choose(i == 1),
                    child: Pressable(
                      onTap: () => choose(i == 1),
                      builder: (context, pressed) {
                        final chosen = history == (i == 1);
                        return Padding(
                          padding: const EdgeInsets.all(inset),
                          child: AnimatedContainer(
                            duration: Motion.fast,
                            curve: Curves.easeOut,
                            constraints: const BoxConstraints(
                              minHeight: Touch.target - inset * 2,
                            ),
                            alignment: Alignment.center,
                            decoration: BoxDecoration(
                              color: chosen
                                  ? palette.canvas
                                  : const Color(0x00000000),
                              borderRadius: BorderRadius.circular(
                                Radii.panel - inset,
                              ),
                              boxShadow: chosen ? floatingShadow : null,
                            ),
                            // At the largest text a word shrinks to fit
                            // rather than break.
                            child: FittedBox(
                              fit: BoxFit.scaleDown,
                              child: Text(
                              _labels[i],
                              maxLines: 1,
                              style: type.secondary.copyWith(
                                fontWeight: chosen
                                    ? FontWeight.w600
                                    : FontWeight.w500,
                                color: chosen || pressed
                                    ? palette.text
                                    : palette.textMuted,
                              ),
                            ),
                            ),
                          ),
                        );
                      },
                    ),
                  ),
                ),
            ],
          ),
        ),
      );
    }
    // At the largest text in a narrow window the second tab goes under
    // the first rather than past the edge.
    return Wrap(
      spacing: Space.m,
      children: [
        for (var i = 0; i < _labels.length; i++)
          Semantics(
            button: true,
            selected: history == (i == 1),
            child: Pressable(
              onTap: () => choose(i == 1),
              builder: (context, hovered) {
                final chosen = history == (i == 1);
                return Container(
                  constraints: const BoxConstraints(minHeight: 28),
                  padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                  decoration: BoxDecoration(
                    border: Border(
                      bottom: BorderSide(
                        width: 2,
                        color: chosen
                            ? palette.signal
                            : const Color(0x00000000),
                      ),
                    ),
                  ),
                  child: Align(
                    widthFactor: 1,
                    child: Text(
                      _labels[i],
                      style: TypeScale.of(context).body.copyWith(
                        color: chosen || hovered
                            ? palette.text
                            : palette.textMuted,
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
      ],
    );
  }
}
