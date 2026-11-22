// Needs you: everything that waits for the person's answer, across every
// agent, chat and pipe, in one place, so nothing waits buried in a conversation
// they don't read. Each is a bubble that opens to the card itself, with its
// real controls, so they answer without opening the conversation.
//
// The app keeps nothing of it: what waits is the backend's (each agent's,
// chat's and pipe's waiting_seqs), and each card is fetched as it is now,
// with LoadEntries for an agent's, LoadChatEntries for a chat's and
// LoadPipeMessages for a pipe's.
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'ask_card.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'chats.dart' show ChatGlyph, chatTitle, openChat;
import 'connectors.dart' show ConnectProposalCard, serviceName;
import 'controls.dart';
import 'forms.dart' show archivedProjectOf, rivetIn;
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'move.dart' show BackendMove, MoveConfirmCard, MoveStatus, moveStatusOf;
import 'pairing_card.dart' show DevicePairingCard;
import 'pipe_start.dart' show PipeStartCard, pipeStartSummary;
import 'pane_chrome.dart' show InPane;
import 'pipe_timeline.dart' show pipeItemKey, pipeLiaison;
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'recovery_key.dart' show SaveRecoveryKeyForm;
import 'scope.dart';
import 'secrets.dart'
    show SecretRequestCard, UnlockSecretsForm, secretRequestSummary;
import 'state.dart' show TranscriptStore;
import 'transcript.dart' show ProposalCard;
import 'usage.dart' show formatAge;
import 'waiting.dart' show NeedsYouMark;

/// What a thing that waits for the person is.
enum NeedsYouKind {
  /// A card in an agent's conversation (Agent.waiting_seqs).
  entry,

  /// A card in one of the person's chats (Chat.waiting_seqs): a proposal
  /// to Rivet.
  chat,

  /// A pipe waiting for the person's approval.
  pipeApproval,

  /// A pipe the loop breaker paused, for the person to continue.
  pipeLoop,

  /// A pipe's message held for the person (Pipe.waiting_seqs).
  pipeHeld,

  /// A provider refused to renew an account's sign-in
  /// (Account.sign_in_again): the person signs in to it again.
  signIn,

  /// The secrets are locked and agents have waited for them
  /// (VaultState.agents_waiting): the person unlocks them with the
  /// recovery key.
  vault,

  /// A move of this Claktory an agent started, its digits waiting for the
  /// person's own Confirm here (Workspace.move).
  move,

  /// The recovery key isn't confirmed saved yet
  /// (VaultState.recovery_key_unconfirmed): the person shows it, saves it
  /// and types back one group of it.
  recoveryKey,
}

/// One thing that waits for the person's answer.
class NeedsYouItem {
  const NeedsYouItem.entry(this.agentId, this.seq)
    : kind = NeedsYouKind.entry,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '';

  const NeedsYouItem.chat(this.chatId, this.seq)
    : kind = NeedsYouKind.chat,
      agentId = '',
      pipeId = '',
      providerId = '',
      accountId = '';

  const NeedsYouItem.pipe(this.kind, this.pipeId, {this.seq = 0})
    : agentId = '',
      chatId = '',
      providerId = '',
      accountId = '';

  const NeedsYouItem.signIn(this.providerId, this.accountId)
    : kind = NeedsYouKind.signIn,
      agentId = '',
      chatId = '',
      pipeId = '',
      seq = 0;

  const NeedsYouItem.move()
    : kind = NeedsYouKind.move,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0;

  const NeedsYouItem.vault()
    : kind = NeedsYouKind.vault,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0;

  const NeedsYouItem.recoveryKey()
    : kind = NeedsYouKind.recoveryKey,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0;

  final NeedsYouKind kind;
  final String agentId;
  final String chatId;
  final String pipeId;

  /// The provider and account to sign in to again.
  final String providerId;
  final String accountId;

  /// The entry's seq in the agent's or chat's conversation, or the held
  /// message's in the pipe's.
  final int seq;

  /// Whether it's a card in a conversation, an agent's or a chat's.
  bool get isEntry => kind == NeedsYouKind.entry || kind == NeedsYouKind.chat;

  /// What tells it apart from every other.
  String get key => switch (kind) {
    NeedsYouKind.entry => 'agent:$agentId/$seq',
    NeedsYouKind.chat => 'chat:$chatId/$seq',
    NeedsYouKind.pipeHeld => 'pipe:$pipeId/$seq',
    NeedsYouKind.pipeApproval => 'pipe:$pipeId/approval',
    NeedsYouKind.pipeLoop => 'pipe:$pipeId/loop',
    NeedsYouKind.signIn => 'signin:$providerId/$accountId',
    NeedsYouKind.vault => 'vault',
    NeedsYouKind.move => 'move',
    NeedsYouKind.recoveryKey => 'recoveryKey',
  };

  /// Done in a click, with no card to open: signing in again, unlocking
  /// the secrets, saving the recovery key.
  bool get isAction =>
      kind == NeedsYouKind.signIn ||
      kind == NeedsYouKind.vault ||
      kind == NeedsYouKind.recoveryKey;
}

/// Everything that waits for the person now, agents' cards first in the
/// sidebar's order, then chats', then pipes', then sign-ins to renew and
/// locked secrets agents wait for. Archived projects' wait with them, out
/// of sight, as in the sidebar.
List<NeedsYouItem> needsYouItems(Workspace workspace) {
  final archived = {
    for (final project in workspace.projects)
      if (project.archived) project.id,
  };
  return [
    for (final agent in workspace.agents)
      if (archivedProjectOf(workspace, agent) == null)
        for (final seq in agent.waitingSeqs)
          NeedsYouItem.entry(agent.id, seq.toInt()),
    for (final chat in workspace.chats)
      for (final seq in chat.waitingSeqs)
        NeedsYouItem.chat(chat.id, seq.toInt()),
    for (final pipe in workspace.pipes)
      if (!archived.contains(pipe.projectId)) ...[
        if (pipe.state == PipeState.PIPE_STATE_APPROVAL)
          NeedsYouItem.pipe(NeedsYouKind.pipeApproval, pipe.id),
        if (pipe.state == PipeState.PIPE_STATE_PAUSED &&
            pipe.paused == PipePause.PIPE_PAUSE_LOOP)
          NeedsYouItem.pipe(NeedsYouKind.pipeLoop, pipe.id),
        for (final seq in pipe.waitingSeqs)
          NeedsYouItem.pipe(NeedsYouKind.pipeHeld, pipe.id, seq: seq.toInt()),
      ],
    for (final provider in workspace.providers)
      for (final account in provider.accounts)
        if (account.signInAgain) NeedsYouItem.signIn(provider.id, account.id),
    if (workspace.vault.agentsWaiting) const NeedsYouItem.vault(),
    if (workspace.vault.recoveryKeyUnconfirmed)
      const NeedsYouItem.recoveryKey(),
    // Also floating over the window on every device (MoveGate).
    if (workspace.hasMove() && moveStatusOf(workspace.move).awaitsPerson)
      const NeedsYouItem.move(),
  ];
}

/// The provider and account [item] asks to sign in to again, while it
/// still does.
({Provider provider, Account account})? signInOf(
  Workspace workspace,
  NeedsYouItem item,
) {
  for (final provider in workspace.providers) {
    if (provider.id != item.providerId) continue;
    for (final account in provider.accounts) {
      if (account.id == item.accountId) {
        return (provider: provider, account: account);
      }
    }
  }
  return null;
}

/// The account [item] asks to sign in to again.
Account? signInAccount(Workspace workspace, NeedsYouItem item) =>
    signInOf(workspace, item)?.account;

/// What a sign-in to renew says: whose expired, and what agents do
/// meanwhile, using the provider's other accounts or waiting.
String signInAgainText(Provider provider, Account account) {
  final who = account.label.isEmpty ? 'This account' : account.label;
  final others = provider.accounts.any(
    (other) => other.id != account.id && !other.signInAgain,
  );
  return "$who's sign-in expired. "
      '${others ? 'Agents use your other accounts meanwhile.' : 'Agents on its models wait until you sign in.'}';
}

/// Whether what began waiting at [ms] has waited a day or more by [now]:
/// its age then stands out.
bool waitedADay(int ms, DateTime now) =>
    now.difference(DateTime.fromMillisecondsSinceEpoch(ms)).inDays >= 1;

/// How many things wait for the person, everywhere.
int needsYouCount(Workspace workspace) => needsYouItems(workspace).length;

/// What the count says to a screen reader.
String needsYouLabel(int count) => count == 1
    ? 'Something is waiting for your answer'
    : '$count things are waiting for your answer';

/// The Needs you page: on a Mac a pane beside the conversation (or the
/// agent view's place, with no agent), on a phone a page of its own.
class InboxView extends StatelessWidget {
  const InboxView({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void close() => AppScope.closePage(context);
    final inPane = InPane.of(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              isPhoneLayout(context)
                  ? PhoneHeader(
                      leading: HeaderButton.back(onTap: close),
                      title: Text(
                        'Needs you',
                        style: PhoneType.emphasis.copyWith(color: palette.text),
                      ),
                    )
                  : WindowDrag(
                      child: Container(
                        height: titleBarHeight,
                        padding: const EdgeInsets.only(
                          left: Space.xl,
                          right: Space.m,
                        ),
                        decoration: BoxDecoration(
                          border: Border(
                            bottom: BorderSide(color: palette.hairline),
                          ),
                        ),
                        child: Row(
                          children: [
                            Text(
                              'Needs you',
                              style: TypeScale.of(context).label.copyWith(
                                color: palette.text,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            const Spacer(),
                            Button(label: 'Done', onPressed: close),
                          ],
                        ),
                      ),
                    ),
            const Expanded(child: NeedsYouList()),
          ],
        ),
      ),
    );
  }
}

/// Everything that waits for the person, newest first, each a bubble that
/// opens to its card. What they answer here stays, answered, until they
/// leave, so they see it went through. A phone's drawer can show it too.
class NeedsYouList extends StatefulWidget {
  const NeedsYouList({super.key, this.padding, this.shrinkWrap = false});

  /// Around the list; the page's own by default.
  final EdgeInsets? padding;

  /// As tall as its items, up to what it's given (the chip's drop-down).
  final bool shrinkWrap;

  @override
  State<NeedsYouList> createState() => _NeedsYouListState();
}

class _NeedsYouListState extends State<NeedsYouList> {
  late final Backend _backend = AppScope.of(context);

  /// The cards as fetched, by key.
  final _entries = <String, Entry>{};
  final _messages = <String, PipeMessage>{};

  /// Fetches under way, and those that failed (why).
  final _fetching = <String>{};
  final _failed = <String, String>{};

  /// Open bubbles, and what the person opened here: kept in the list once
  /// answered, until they leave, showing how it ended.
  final _open = <String>{};
  final _kept = <String, NeedsYouItem>{};

  /// What waited the last time the list was built.
  var _waiting = <String>{};

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _backend.workspace.addListener(_changed);
      _backend.transcript.addListener(_rebuild);
      _backend.chatTranscript.addListener(_rebuild);
      _backend.pipeChat.addListener(_rebuild);
      _changed();
    });
  }

  @override
  void dispose() {
    _backend.workspace.removeListener(_changed);
    _backend.transcript.removeListener(_rebuild);
    _backend.chatTranscript.removeListener(_rebuild);
    _backend.pipeChat.removeListener(_rebuild);
    super.dispose();
  }

  void _rebuild() {
    if (mounted) setState(() {});
  }

  /// What waits changed, perhaps: fetch what's new, and once more what the
  /// person answered here, to show how it ended.
  void _changed() {
    if (!mounted) return;
    final items = needsYouItems(_backend.workspace.workspace);
    final now = {for (final item in items) item.key};
    for (final item in items) {
      if (!_has(item)) _fetch(item);
    }
    for (final MapEntry(key: key, value: item) in _kept.entries) {
      if (_waiting.contains(key) && !now.contains(key)) _fetch(item);
    }
    _waiting = now;
    setState(() {});
  }

  bool _has(NeedsYouItem item) => switch (item.kind) {
    NeedsYouKind.entry || NeedsYouKind.chat => _entries.containsKey(item.key),
    NeedsYouKind.pipeHeld => _messages.containsKey(item.key),
    _ => true,
  };

  void _fetch(NeedsYouItem item) {
    final key = item.key;
    if (item.kind == NeedsYouKind.pipeApproval ||
        item.kind == NeedsYouKind.pipeLoop ||
        !_fetching.add(key)) {
      return;
    }
    Future<void> run() async {
      try {
        if (item.kind == NeedsYouKind.entry) {
          final entry = await _backend.fetchEntry(item.agentId, item.seq);
          if (entry != null) _entries[key] = entry;
        } else if (item.kind == NeedsYouKind.chat) {
          final entry = await _backend.fetchChatEntry(item.chatId, item.seq);
          if (entry != null) _entries[key] = entry;
        } else {
          final message = await _backend.fetchPipeMessage(
            item.pipeId,
            item.seq,
          );
          if (message != null) _messages[key] = message;
        }
        _failed.remove(key);
      } on RequestFailure catch (failure) {
        _failed[key] = failure.message;
      } finally {
        _fetching.remove(key);
        if (mounted) setState(() {});
      }
    }

    unawaited(run());
  }

  /// [item]'s card as it is now: the open conversation's own copy when it
  /// holds it (it's live), else what was fetched.
  Entry? _entryOf(NeedsYouItem item) {
    Entry? live(TranscriptStore transcript) {
      for (var i = transcript.length - 1; i >= 0; i--) {
        final entry = transcript[i].entry;
        if (entry.seq.toInt() == item.seq) return entry;
        if (entry.seq.toInt() < item.seq) break;
      }
      return null;
    }

    final chat = _backend.chatTranscript;
    final held = item.kind == NeedsYouKind.chat
        ? (chat.chatId == item.chatId ? live(chat) : null)
        : (_backend.transcript.agentId == item.agentId
              ? live(_backend.transcript)
              : null);
    return held ?? _entries[item.key];
  }

  PipeMessage? _messageOf(NeedsYouItem item) {
    final chat = _backend.pipeChat;
    if (chat.pipeId == item.pipeId) {
      for (final message in chat.messages) {
        if (message.seq.toInt() == item.seq) return message;
      }
    }
    return _messages[item.key];
  }

  /// When it began waiting, to put the newest first; 0 while unknown.
  int _timeOf(NeedsYouItem item, Workspace workspace) => switch (item.kind) {
    NeedsYouKind.entry ||
    NeedsYouKind.chat => _entryOf(item)?.timestampMs.toInt() ?? 0,
    NeedsYouKind.pipeHeld => _messageOf(item)?.timestampMs.toInt() ?? 0,
    _ => 0,
  };

  void _toggle(NeedsYouItem item) {
    final key = item.key;
    setState(() {
      if (!_open.remove(key)) {
        _open.add(key);
        _kept[key] = item;
      }
    });
    // Opened: as it is now.
    if (_open.contains(key)) _fetch(item);
  }

  /// Signs in again, or unlocks the secrets: no card to open first.
  void _act(NeedsYouItem item) {
    switch (item.kind) {
      case NeedsYouKind.signIn:
        final account = signInAccount(_backend.workspace.workspace, item);
        if (account == null) return;
        // Settings → Providers shows how the sign-in goes, at its row.
        fireAndLog(
          AppScope.scope(context).login.start(
            item.providerId,
            account.methodId.isEmpty ? 'oauth' : account.methodId,
            addAccount: true,
          ),
        );
        AppScope.openProviders(context);
      case NeedsYouKind.vault:
        showModal(context, (context, close) => UnlockSecretsForm(close: close));
      case NeedsYouKind.recoveryKey:
        showModal(
          context,
          (context, close) => SaveRecoveryKeyForm(close: close),
        );
      default:
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = _backend.workspace.workspace;
    final waiting = needsYouItems(workspace);
    final keys = {for (final item in waiting) item.key};
    final items = [
      ...waiting,
      // Answered here meanwhile: stays, showing how it ended.
      for (final item in _kept.values)
        if (!keys.contains(item.key) && _stillThere(item, workspace)) item,
    ];
    // Newest first; unknown times (a pipe's approval, its pause) on top.
    final order = {for (var i = 0; i < items.length; i++) items[i].key: i};
    items.sort((a, b) {
      final ta = _timeOf(a, workspace), tb = _timeOf(b, workspace);
      final ka = ta == 0 ? 1 << 62 : ta, kb = tb == 0 ? 1 << 62 : tb;
      if (ka != kb) return kb.compareTo(ka);
      return order[a.key]!.compareTo(order[b.key]!);
    });
    final padding =
        widget.padding ??
        EdgeInsets.all(isPhoneLayout(context) ? Space.l : Space.xl);
    if (items.isEmpty) {
      return Center(
        child: Padding(
          padding: padding,
          child: Text(
            'Nothing is waiting for your answer.',
            textAlign: TextAlign.center,
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
        ),
      );
    }
    return ListView(
      shrinkWrap: widget.shrinkWrap,
      padding: padding,
      children: [
        // Keyed here, on the list's own children: matched by place instead,
        // a question coming in above would take this one's card, and with
        // it what the person was typing and their focus.
        for (final item in items)
          Align(
            key: ValueKey(item.key),
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 640),
              child: Padding(
                padding: const EdgeInsets.only(bottom: Space.s),
                child: _Bubble(
                  item: item,
                  waiting: keys.contains(item.key),
                  open: _open.contains(item.key),
                  onToggle: () => item.isAction ? _act(item) : _toggle(item),
                  entry: item.isEntry ? _entryOf(item) : null,
                  message: item.kind == NeedsYouKind.pipeHeld
                      ? _messageOf(item)
                      : null,
                  failed: _failed[item.key],
                  workspace: workspace,
                ),
              ),
            ),
          ),
      ],
    );
  }

  /// Whether what [item] was about still exists: its agent, chat or pipe.
  static bool _stillThere(NeedsYouItem item, Workspace workspace) =>
      switch (item.kind) {
        NeedsYouKind.entry => workspace.agents.any(
          (agent) => agent.id == item.agentId,
        ),
        NeedsYouKind.chat => workspace.chats.any(
          (chat) => chat.id == item.chatId,
        ),
        NeedsYouKind.move => workspace.hasMove(),
        _ => workspace.pipes.any((pipe) => pipe.id == item.pipeId),
      };
}

/// One thing that waits, as a bubble: who asks and what, folded; opened,
/// the card itself, and a way to its conversation or pipe.
class _Bubble extends StatelessWidget {
  const _Bubble({
    required this.item,
    required this.waiting,
    required this.open,
    required this.onToggle,
    required this.entry,
    required this.message,
    required this.failed,
    required this.workspace,
  });

  final NeedsYouItem item;

  /// Still waiting; else answered here, showing how it ended.
  final bool waiting;
  final bool open;
  final VoidCallback onToggle;
  final Entry? entry;
  final PipeMessage? message;
  final String? failed;
  final Workspace workspace;

  /// Whose it is: a move's is the agent that started it.
  Agent? get _agent {
    final id = item.kind == NeedsYouKind.move
        ? _move.startedByAgentId
        : item.agentId;
    return [
      for (final agent in workspace.agents)
        if (agent.id == id) agent,
    ].firstOrNull;
  }

  MoveStatus get _move =>
      moveStatusOf(workspace.hasMove() ? workspace.move : null);

  Chat? get _chat => [
    for (final chat in workspace.chats)
      if (chat.id == item.chatId) chat,
  ].firstOrNull;

  Pipe? get _pipe => [
    for (final pipe in workspace.pipes)
      if (pipe.id == item.pipeId) pipe,
  ].firstOrNull;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final agent = _agent;
    final chat = _chat;
    final pipe = _pipe;
    // A pipe's liaison asks as the pipe's ("Kai · Acme launch").
    final liaisonOf = agent == null || agent.pipeId.isEmpty
        ? null
        : [
            for (final pipe in workspace.pipes)
              if (pipe.id == agent.pipeId) pipe,
          ].firstOrNull;
    final signIn = signInOf(workspace, item);
    final who = item.kind == NeedsYouKind.signIn
        ? 'Sign in to ${signIn?.provider.name ?? 'your provider'} again'
        : item.kind == NeedsYouKind.vault
        ? 'Unlock your secrets with your recovery key'
        : item.kind == NeedsYouKind.recoveryKey
        ? 'Save your recovery key'
        : liaisonOf != null
        ? '${agent!.name} · ${liaisonOf.name}'
        : agent?.name ??
              (chat == null ? null : chatTitle(chat)) ??
              pipe?.name ??
              '';
    final what = _summary(backend);
    final time = switch (item.kind) {
      NeedsYouKind.entry ||
      NeedsYouKind.chat => entry?.timestampMs.toInt() ?? 0,
      NeedsYouKind.pipeHeld => message?.timestampMs.toInt() ?? 0,
      _ => 0,
    };
    final header = Semantics(
      button: true,
      expanded: open,
      child: Pressable(
        onTap: onToggle,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          curve: Curves.easeOut,
          constraints: const BoxConstraints(minHeight: 48),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s,
          ),
          color: hovered ? palette.hover : const Color(0x00000000),
          child: Row(
            children: [
              SizedBox.square(
                dimension: 24,
                child: Center(
                  child: agent != null
                      ? AgentCharacter(agent: agent, size: 24)
                      : item.isAction
                      ? ExtensionGlyph(
                          icon:
                              item.kind == NeedsYouKind.vault ||
                                  item.kind == NeedsYouKind.recoveryKey
                              ? 'key-round'
                              : 'user',
                          name: '',
                          color: palette.textMuted,
                        )
                      : item.kind == NeedsYouKind.chat
                      ? ChatGlyph(
                          color: waiting
                              ? palette.textMuted
                              : palette.textFaint,
                        )
                      : PortalGlyph(
                          color: waiting ? pipeColor : palette.textFaint,
                        ),
                ),
              ),
              const SizedBox(width: Space.s + 2),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    // How long it has waited, ticking; past a day, while
                    // it still waits, in signal.
                    Ticking(
                      active: time > 0,
                      builder: (context, now) => Text.rich(
                        TextSpan(
                          text: who,
                          children: [
                            if (time > 0)
                              TextSpan(
                                text: ' · ${formatAge(time, now)}',
                                style: TextStyles.caption.copyWith(
                                  color: waiting && waitedADay(time, now)
                                      ? palette.signal
                                      : palette.textFaint,
                                  fontWeight: FontWeight.w400,
                                ),
                              ),
                          ],
                        ),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.label.copyWith(
                          color: waiting ? palette.text : palette.textMuted,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    Text(
                      what,
                      maxLines: open ? 3 : 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyles.body.copyWith(
                        color: waiting ? palette.textMuted : palette.textFaint,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              // A proposal to Rivet goes stale: Dismiss it in one click.
              if (waiting && !open && entry?.whichKind() == Entry_Kind.proposal)
                Padding(
                  padding: const EdgeInsets.only(right: Space.s),
                  child: Button(
                    label: 'Dismiss',
                    onPressed: () => fireAndLog(
                      backend.decideProposal(
                        item.agentId,
                        entry!.proposal.id,
                        approve: false,
                        chatId: item.chatId,
                      ),
                    ),
                  ),
                )
              else if (waiting)
                const SizedBox.shrink()
              else
                Padding(
                  padding: const EdgeInsets.only(right: Space.s),
                  child: Text(
                    'Answered',
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
                ),
              AnimatedRotation(
                turns: open ? 0.25 : 0,
                duration: Motion.fast,
                curve: Curves.easeOut,
                child: Chevron(
                  color: hovered ? palette.textMuted : palette.textFaint,
                  down: false,
                ),
              ),
            ],
          ),
        ),
      ),
    );
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: AnimatedSize(
        duration: Motion.base,
        curve: Curves.easeOut,
        alignment: Alignment.topCenter,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            header,
            if (open)
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  Space.m,
                  Space.xs,
                  Space.m,
                  Space.m,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _card(context, palette),
                    // A move's card is all there is to it.
                    if (item.kind != NeedsYouKind.move) ...[
                      const SizedBox(height: Space.s),
                      Align(
                        alignment: Alignment.centerLeft,
                        child: _OpenLink(
                          label: switch (item.kind) {
                            NeedsYouKind.entry when liaisonOf != null =>
                              'Open pipe',
                            NeedsYouKind.entry => 'Open conversation',
                            NeedsYouKind.chat => 'Open chat',
                            _ => 'Open pipe',
                          },
                          onTap: () => _openWhere(context),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }

  /// The card, with its real controls; until it's here, where it is.
  Widget _card(BuildContext context, Palette palette) {
    final pipe = _pipe;
    Widget line(String text, {Color? color}) => Text(
      text,
      style: TextStyles.caption.copyWith(color: color ?? palette.textFaint),
    );
    switch (item.kind) {
      case NeedsYouKind.pipeApproval:
        if (pipe == null || pipe.state != PipeState.PIPE_STATE_APPROVAL) {
          return line(
            pipe == null ? 'This pipe was deleted.' : pipeStatus(pipe),
          );
        }
        return PipeApprovalCard(pipe: pipe, inline: true);
      case NeedsYouKind.pipeLoop:
        if (pipe == null ||
            pipe.state != PipeState.PIPE_STATE_PAUSED ||
            pipe.paused != PipePause.PIPE_PAUSE_LOOP) {
          return line(
            pipe == null ? 'This pipe was deleted.' : pipeStatus(pipe),
          );
        }
        return PipeLoopCard(pipe: pipe);
      case NeedsYouKind.pipeHeld:
        final message = this.message;
        if (pipe == null) return line('This pipe was deleted.');
        if (message == null) {
          return line(
            failed ?? 'Loading…',
            color: failed == null ? null : palette.danger,
          );
        }
        return PipeMessageView(pipe: pipe, message: message);
      case NeedsYouKind.signIn ||
          NeedsYouKind.vault ||
          NeedsYouKind.recoveryKey:
        // Done in a click; never opened.
        return const SizedBox.shrink();
      case NeedsYouKind.move:
        final move = _move;
        if (!MoveConfirmCard.shows(move)) {
          return line(
            move.active
                ? 'This move is under way. Follow it in Settings → General.'
                : 'This move ended.',
          );
        }
        return MoveConfirmCard(
          move: move,
          actions: BackendMove(AppScope.of(context)),
          agentName: _agent?.name ?? '',
        );
      case NeedsYouKind.entry || NeedsYouKind.chat:
        final entry = this.entry;
        if (item.kind == NeedsYouKind.chat && _chat == null) {
          return line('This chat was deleted.');
        }
        if (entry == null) {
          return line(
            failed ?? 'Loading…',
            color: failed == null ? null : palette.danger,
          );
        }
        return waitingCard(entry, agentId: item.agentId, chatId: item.chatId) ??
            line(
              item.kind == NeedsYouKind.chat
                  ? 'Open the chat to answer this.'
                  : 'Open the conversation to answer this.',
            );
    }
  }

  /// What it asks, in a line.
  String _summary(Backend backend) {
    final pipe = _pipe;
    switch (item.kind) {
      case NeedsYouKind.pipeApproval:
        return pipe == null
            ? 'Waiting for your approval'
            : '${pipePeerName(pipe)} wants to connect';
      case NeedsYouKind.pipeLoop:
        return 'Paused: agents kept talking on their own';
      case NeedsYouKind.pipeHeld:
        final message = this.message;
        if (message == null) return 'A message waits for you';
        final held =
            message.state == PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING;
        return held
            ? 'Let in a message from '
                  '${pipe == null ? 'the other side' : pipePeerClaktory(pipe)}?'
                  '\n${message.text}'
            : 'Waiting for you to send it: ${message.text}';
      case NeedsYouKind.signIn:
        final signIn = signInOf(workspace, item);
        return signIn == null
            ? 'Signed in again'
            : signInAgainText(signIn.provider, signIn.account);
      case NeedsYouKind.move:
        return 'Move this Claktory to ${_move.target}?';
      case NeedsYouKind.vault:
        final why = workspace.vault.lockedWhy;
        return why.isEmpty ? 'Agents are waiting for your secrets.' : why;
      case NeedsYouKind.recoveryKey:
        return "It unlocks your secrets if claktory.com can't. Save it, "
            'then confirm you have it.';
      case NeedsYouKind.entry || NeedsYouKind.chat:
        final entry = this.entry;
        if (entry == null) return 'Waiting for your answer';
        return waitingSummary(entry, backend, agentId: item.agentId);
    }
  }

  /// Opens the conversation at the card, the chat, or the pipe.
  void _openWhere(BuildContext context) {
    final backend = AppScope.of(context);
    final chat = _chat;
    AppScope.closePage(context);
    final agent = _agent;
    final pipe = _pipe;
    if (item.kind == NeedsYouKind.entry) {
      fireAndLog(() async {
        // A pipe's liaison's conversation shows in its pipe.
        if (agent != null && agent.pipeId.isNotEmpty) {
          await backend.selectPipe(agent.pipeId);
        } else {
          await backend.selectAgent(item.agentId);
        }
        backend.reveal.reachSeq(item.agentId, item.seq);
      }());
    } else if (item.kind == NeedsYouKind.chat) {
      if (chat != null) openChat(context, chat);
    } else {
      final liaison = pipe == null
          ? null
          : pipeLiaison(backend.workspace.workspace, pipe);
      fireAndLog(() async {
        await backend.selectPipe(item.pipeId);
        // Among its liaison's conversation, at the message's card.
        if (liaison != null && item.kind == NeedsYouKind.pipeHeld) {
          backend.reveal.reachExtra(liaison.id, pipeItemKey(item.seq));
        }
      }());
    }
    // On a phone, the conversation is pushed over the list.
    PhoneNavigation.of(context)?.value = true;
  }
}

/// A quiet link under an open bubble's card.
class _OpenLink extends StatelessWidget {
  const _OpenLink({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.xs),
          child: Text(
            label,
            style: TextStyles.caption.copyWith(
              color: hovered ? palette.text : palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ),
    );
  }
}

/// What a waiting card in [entry] asks, in a line; [agentId] is whose
/// conversation shows it.
String waitingSummary(Entry entry, Backend backend, {String? agentId}) =>
    switch (entry.whichKind()) {
      Entry_Kind.secretRequest => secretRequestSummary(
        entry.secretRequest,
        backend.workspace.workspace,
        shownIn: agentId,
      ),
      Entry_Kind.connectProposal =>
        'Suggests connecting '
            '${serviceName(backend, entry.connectProposal.connectorId)}',
      Entry_Kind.devicePairing => 'Adds a device: ${entry.devicePairing.name}',
      Entry_Kind.proposal =>
        'Proposes to ${rivetIn(backend.workspace.workspace)?.name ?? 'Rivet'}: '
            '${entry.proposal.text.trim().split('\n').first}',
      Entry_Kind.secretApproval =>
        'Wants to use ${entry.secretApproval.secretNames.join(', ')}',
      Entry_Kind.askPerson => entry.askPerson.title,
      Entry_Kind.pipeStart => pipeStartSummary(entry.pipeStart),
      _ => 'Waiting for your answer',
    };

/// The card for a waiting [entry] of [agentId]'s (or, with [chatId], of
/// that chat's), with its real controls, as its conversation shows it;
/// null for a kind this app can't show.
Widget? waitingCard(
  Entry entry, {
  required String agentId,
  String chatId = '',
}) => switch (entry.whichKind()) {
  Entry_Kind.secretRequest => SecretRequestCard(
    key: ValueKey(entry.secretRequest.id),
    entry.secretRequest,
    shownIn: agentId,
  ),
  Entry_Kind.connectProposal => ConnectProposalCard(
    key: ValueKey(entry.connectProposal.id),
    entry.connectProposal,
  ),
  Entry_Kind.devicePairing => DevicePairingCard(
    key: ValueKey(entry.devicePairing.id),
    entry.devicePairing,
  ),
  Entry_Kind.proposal => ProposalCard(
    key: ValueKey(entry.proposal.id),
    entry.proposal,
    agentId: agentId,
    // Explicit, so a card shown over a chat isn't taken for the chat's.
    chatId: chatId,
  ),
  Entry_Kind.askPerson => AskPersonCard(
    key: ValueKey('$agentId/${entry.seq}'),
    entry.askPerson,
    agentId: agentId,
    seq: entry.seq.toInt(),
  ),
  Entry_Kind.secretApproval => SecretApprovalCard(
    key: ValueKey('$agentId/${entry.seq}'),
    entry.secretApproval,
    seq: entry.seq.toInt(),
  ),
  Entry_Kind.pipeStart => PipeStartCard(
    key: ValueKey(entry.pipeStart.id),
    entry.pipeStart,
  ),
  _ => null,
};

/// How many things need the person, on the chip.
String needsYouChipText(int count) =>
    count == 1 ? '1 needs you' : '$count need you';

/// What the chip says to VoiceOver.
String needsYouChipLabel(int count) => count == 1
    ? '1 thing needs your answer. Show it.'
    : '$count things need your answer. Show them.';

/// Over every conversation, at its top right, while anything anywhere
/// waits for the person's answer: a floating chip saying how many ("2 need
/// you"). A click drops the Needs you bubbles down under it, over the
/// conversation, which stays as it is; another click folds them away. What
/// the person answers there stays, answered, while it's open. With nothing
/// waiting and nothing open, it's nothing at all.
class NeedsYouChip extends StatefulWidget {
  const NeedsYouChip({super.key, this.onOpen});

  /// Opens the bubbles some other way (a phone's sheet) instead of
  /// dropping them down under the chip.
  final VoidCallback? onOpen;

  @override
  State<NeedsYouChip> createState() => _NeedsYouChipState();
}

class _NeedsYouChipState extends State<NeedsYouChip> {
  var _open = false;

  void _toggle() {
    final onOpen = widget.onOpen;
    if (onOpen != null) return onOpen();
    setState(() => _open = !_open);
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final count = needsYouCount(backend.workspace.workspace);
        if (count == 0 && !_open) return const SizedBox.shrink();
        return LayoutBuilder(
          builder: (context, constraints) {
            final chip = _chip(context, count);
            if (!_open) return chip;
            final width = constraints.maxWidth.isFinite
                ? constraints.maxWidth.clamp(0.0, 440.0)
                : 440.0;
            return CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.escape): _toggle,
              },
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  chip,
                  const SizedBox(height: Space.xs),
                  Flexible(child: _panel(context, width)),
                ],
              ),
            );
          },
        );
      },
    );
  }

  Widget _chip(BuildContext context, int count) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      expanded: _open,
      label: count == 0 ? 'Close' : needsYouChipLabel(count),
      excludeSemantics: true,
      child: Pressable(
        onTap: _toggle,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          curve: Curves.easeOut,
          constraints: BoxConstraints(minHeight: Metrics.of(context).target),
          padding: const EdgeInsets.only(
            left: Space.xs + 2,
            right: Space.s + 2,
          ),
          decoration: BoxDecoration(
            color: hovered || _open ? palette.selected : palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.floating),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (count > 0) ...[
                NeedsYouMark(count: count, label: ''),
                const SizedBox(width: Space.xs + 2),
              ],
              Text(
                count == 0 ? 'Close' : needsYouChipText(count),
                style: TypeScale.of(context).label
                    .copyWith(color: palette.text, fontWeight: FontWeight.w500),
              ),
              // It drops down: a sheet from below has no need of it.
              if (widget.onOpen == null) ...[
                const SizedBox(width: Space.xs + 2),
                AnimatedRotation(
                  turns: _open ? 0.5 : 0,
                  duration: Motion.fast,
                  curve: Curves.easeOut,
                  child: Chevron(color: palette.textMuted, down: true),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _panel(BuildContext context, double width) {
    final palette = Palette.of(context);
    return Container(
      width: width,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: palette.canvas,
        borderRadius: BorderRadius.circular(Radii.floating),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: const NeedsYouList(
        shrinkWrap: true,
        padding: EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, 0),
      ),
    );
  }
}
