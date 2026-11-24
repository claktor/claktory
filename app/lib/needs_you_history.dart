import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'app_dock.dart' show openAppsTab;
import 'apps.dart' show undoAppAccess;
import 'backend.dart';
import 'characters/character.dart';
import 'chats.dart' show ChatGlyph, chatTitle, openChat;
import 'controls.dart';
import 'find.dart' show FindScope;
import 'forms.dart' show AgentSettingsForm;
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
import 'visible_codes.dart' show proseCodes;

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

  /// What an Undo said, under the change it undid (by [_undoKey]): what
  /// was left as it was, or why it couldn't.
  final _undoNotes = <String, ({String text, bool failed})>{};

  /// Who gave what in every app, every agent's model and effort, and what
  /// waits for the person in each conversation, as last seen: a change to
  /// any (Rivet's, the person's on another device, a card answered or its
  /// agent taking it back, #407) changes it, and History asks again.
  late String _given;

  @override
  void initState() {
    super.initState();
    _search.addListener(_searchChanged);
    _given = _appsGiven();
    _backend.workspace.addListener(_workspaceChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _load();
    });
  }

  String _appsGiven() => [
    for (final app in _backend.workspace.workspace.apps) ...[
      for (final grant in app.grants)
        '${grant.agentId}:${grant.given.changeId}',
      for (final source in app.sources)
        if (source.granted) '${source.id}:${source.given.changeId}',
      '/',
    ],
    for (final agent in _backend.workspace.workspace.agents)
      '${agent.id}=${agent.model}/${agent.effort}/${agent.waitingSeqs.join('.')}',
    for (final chat in _backend.workspace.workspace.chats)
      '${chat.id}?${chat.waitingSeqs.join('.')}',
  ].join(',');

  void _workspaceChanged() {
    final given = _appsGiven();
    if (given == _given) return;
    _given = given;
    if (mounted) _load();
  }

  /// Undo on an app access or model change: done, then the list again
  /// (it's marked Undone; an app access undo is a row of its own).
  Future<void> _undo(NeedsYouRecord record) async {
    final key = _undoKey(record);
    final changeId = _changeOf(record);
    setState(() => _undoNotes.remove(key));
    try {
      String? note;
      if (_ofModel(record)) {
        await _backend.undoModelChange(changeId);
      } else {
        note = await undoAppAccess(_backend, changeId);
      }
      if (!mounted) return;
      if (note case final said?) {
        setState(() => _undoNotes[key] = (text: said, failed: false));
      }
    } on RequestFailure catch (failure) {
      if (!mounted) return;
      setState(() => _undoNotes[key] = (text: failure.message, failed: true));
    }
    if (mounted) await _load();
  }

  @override
  void dispose() {
    _backend.workspace.removeListener(_workspaceChanged);
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
                    child: HistoryRow(
                      record: record,
                      onUndo: _undo,
                      undoNote: _undoNotes[_undoKey(record)],
                    ),
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
/// Whether [record] is a change to an app's access, or Rivet's card for
/// one.
bool _ofApp(NeedsYouRecord record) =>
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_APP_ACCESS;

/// Whether [record] is Rivet putting an agent on another model or effort.
bool _ofModel(NeedsYouRecord record) =>
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_MODEL;

/// Whether [record] is an agent (Rivet's Conversations tools) taking
/// another agent's conversation back or switching it (friend bug #9):
/// like a model change, with no Undo.
bool _ofConversation(NeedsYouRecord record) =>
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_CONVERSATION;

/// The change Undo undoes: an app access change's or a model change's; ""
/// for none (a declined card, another kind).
String _changeOf(NeedsYouRecord record) => _ofApp(record)
    ? record.appChangeId
    : _ofModel(record)
    ? record.modelChangeId
    : '';

/// What an Undo's note is kept under: the kind and its change.
String _undoKey(NeedsYouRecord record) =>
    '${record.kind.value}/${_changeOf(record)}';

/// Whether [record] opens the app in Marketplace → Apps rather than a
/// conversation: the person's own change, or one with no entry to show.
bool _opensApp(NeedsYouRecord record) =>
    _ofApp(record) &&
    record.appId.isNotEmpty &&
    (record.agentId.isEmpty || record.seq == 0);

bool _ofPipe(NeedsYouRecord record) =>
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_HELD ||
    record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_LOOP ||
    ((record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_FORWARD ||
            // Rivet letting an agent only send to a pipe, or stopping it
            // (#414): Claktory's sentence, and the pipe opens.
            record.kind ==
                NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_MEMBER) &&
        record.pipeId.isNotEmpty);

/// How often an agent changed a card while it waited (#428): "Updated
/// once", "Updated twice", "Updated 3 times".
String updatedTimes(int times) => switch (times) {
  1 => 'Updated once',
  2 => 'Updated twice',
  _ => 'Updated $times times',
};

/// What a record's outcome is called: as its card's buttons said it; ""
/// for one this app doesn't know (a newer Claktory's). [agent] is the name
/// of the agent the card is from, as it is now.
String historyOutcome(
  NeedsYouRecord record, {
  String? agent,
}) => switch (record.outcome) {
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
  // A comment sent back with Submit, rather than a yes or a no.
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_REPLIED => 'Replied',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED => 'Approved',
  // A secret's use as its card says it, the rest Declined (never Denied).
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DENIED
      when record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_SECRET_USE =>
    'Not allowed',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DENIED => 'Declined',
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_DISMISSED => 'Dismissed',
  // Approved, but the grant couldn't be made (Rivet's app access).
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_FAILED => "Approved, but couldn't give it",
  // Taken back by its agent once it no longer applied (#407), never the
  // person's dismissal.
  NeedsYouOutcome.NEEDS_YOU_OUTCOME_WITHDRAWN =>
    'Withdrawn by ${agent ?? 'the agent'}',
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
  const HistoryRow({
    super.key,
    required this.record,
    this.onUndo,
    this.undoNote,
  });

  final NeedsYouRecord record;

  /// Undo, for an app access or model change that can still be undone.
  final ValueChanged<NeedsYouRecord>? onUndo;

  /// What the last Undo of it said.
  final ({String text, bool failed})? undoNote;

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
            record.agentId.isEmpty) ||
        // A change to an app's access the person made, or an undo.
        (_ofApp(record) && record.agentId.isEmpty);
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
    // An app access change undone since says so after how it ended.
    final outcome = [
      historyOutcome(record, agent: agentName),
      if ((_ofApp(record) || _ofModel(record)) && record.undone) 'Undone',
      // Changed again since, so Undo would be refused (row 684).
      if (_ofModel(record) && record.superseded && !record.undone)
        'Changed since',
      // Its agent changed the card while it waited (#428).
      if (record.updates > 0) updatedTimes(record.updates),
    ].where((part) => part.isNotEmpty).join(' · ');
    // Rivet's reason, or why an agent withdrew its card, as the agent's
    // own words, never as Claktory's.
    final reason =
        (_ofApp(record) ||
                _ofModel(record) ||
                _ofConversation(record) ||
                record.outcome ==
                    NeedsYouOutcome.NEEDS_YOU_OUTCOME_WITHDRAWN) &&
            record.agentReason.isNotEmpty
        ? '${agentName ?? 'An agent'}: “${proseCodes(_shortened(record.agentReason, 300))}”'
        : '';
    final undo =
        _changeOf(record).isNotEmpty &&
            !record.undone &&
            !record.superseded &&
            onUndo != null
        ? () => onUndo!(record)
        : null;
    final decided = record.decidedMs.toInt();
    final where = record.chatId.isNotEmpty
        ? 'Open chat'
        : record.kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_REPORT_TYPE
        ? 'Show what crosses'
        : _opensApp(record)
        ? 'Show in Apps'
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
                    if (reason.isNotEmpty)
                      Text(
                        reason,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.caption.copyWith(
                          color: palette.textMuted,
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
                    if (undoNote case final note?)
                      Text(
                        note.text,
                        style: TextStyles.caption.copyWith(
                          color: note.failed
                              ? palette.danger
                              : palette.textMuted,
                        ),
                      ),
                  ],
                ),
              ),
              if (undo != null) ...[
                const SizedBox(width: Space.s),
                Unselectable(
                  child: Button(label: 'Undo', onPressed: undo),
                ),
              ],
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
  // Needs you comes back on History, where this was opened from.
  backend.needsYouHistoryNext = true;
  final workspace = backend.workspace.workspace;
  // The person's own app access change: the app, in Marketplace → Apps.
  if (_opensApp(record)) {
    openAppsTab(context);
    backend.marketplaceApp.value = record.appId;
    return;
  }
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
  // Another agent's conversation taken back or switched, with no call to
  // show (its agent gone): that conversation, as it is now.
  if (_ofConversation(record) && (agent == null || record.seq == 0)) {
    final subject = record.subjectAgentId;
    if (!workspace.agents.any((agent) => agent.id == subject)) return;
    AppScope.closePage(context);
    fireAndLog(backend.selectAgent(subject));
    PhoneNavigation.of(context)?.value = true;
    return;
  }
  // A model change with no entry to show (Rivet gone, or no call): the
  // agent whose model it was, in its settings.
  if (_ofModel(record) && (agent == null || record.seq == 0)) {
    final subject = workspace.agents
        .where((agent) => agent.id == record.subjectAgentId)
        .firstOrNull;
    if (subject != null) {
      showModal(
        context,
        (context, close) => AgentSettingsForm(agent: subject, close: close),
      );
    }
    return;
  }
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
                            // Room at its ends, so the largest text's word
                            // shrinks inside the segment rather than
                            // touching its edges (Zucchero's SE at AX5).
                            padding: const EdgeInsets.symmetric(
                              horizontal: Space.s,
                            ),
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
